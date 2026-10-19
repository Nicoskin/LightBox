//! Folder watching with ReadDirectoryChangesW on a background thread:
//! files added, removed, renamed or rewritten in the folder being browsed.

use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::core::HSTRING;

pub struct Watcher {
    pub dir: PathBuf,
    /// Files that changed (any kind of change), in batches.
    pub rx: Receiver<Vec<PathBuf>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Watches `dir`; `notify` wakes the UI thread after each batch.
    pub fn new(dir: &Path, notify: impl Fn() + Send + 'static) -> Option<Self> {
        // SAFETY: opening a directory handle for change notifications.
        let handle = unsafe {
            CreateFileW(
                &HSTRING::from(dir.as_os_str()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .ok()?;
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let root = dir.to_path_buf();
        let raw = handle.0 as isize;
        let thread = thread::Builder::new()
            .name("watch".into())
            .spawn(move || {
                let h = HANDLE(raw as *mut _);
                // DWORD-aligned buffer, as the API requires.
                let mut buf = vec![0u32; 16 * 1024];
                while !flag.load(Ordering::Acquire) {
                    let mut len = 0u32;
                    // SAFETY: valid handle and buffer; synchronous call that
                    // returns on changes or when cancelled by `Drop`.
                    let ok = unsafe {
                        ReadDirectoryChangesW(
                            h,
                            buf.as_mut_ptr().cast(),
                            (buf.len() * 4) as u32,
                            false,
                            FILE_NOTIFY_CHANGE_FILE_NAME
                                | FILE_NOTIFY_CHANGE_LAST_WRITE
                                | FILE_NOTIFY_CHANGE_SIZE,
                            Some(&mut len),
                            None,
                            None,
                        )
                    }
                    .is_ok();
                    if !ok || flag.load(Ordering::Acquire) {
                        break;
                    }
                    let names = parse(&buf, len as usize, &root);
                    if !names.is_empty() {
                        if tx.send(names).is_err() {
                            break;
                        }
                        notify();
                    }
                }
                // SAFETY: own handle, closed once.
                unsafe {
                    let _ = CloseHandle(h);
                }
            })
            .ok()?;
        Some(Self {
            dir: dir.to_path_buf(),
            rx,
            stop,
            thread: Some(thread),
        })
    }
}

/// Paths named by the FILE_NOTIFY_INFORMATION records in `buf`.
fn parse(buf: &[u32], len: usize, root: &Path) -> Vec<PathBuf> {
    let bytes = len.min(buf.len() * 4);
    let base = buf.as_ptr().cast::<u8>();
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + std::mem::size_of::<FILE_NOTIFY_INFORMATION>() <= bytes {
        // SAFETY: the record lies inside the filled part of the buffer and is
        // DWORD-aligned (offsets are multiples of 4 by the API's contract).
        let info = unsafe { &*base.add(offset).cast::<FILE_NOTIFY_INFORMATION>() };
        let name_len = info.FileNameLength as usize / 2;
        // SAFETY: the name follows the header inside the same record.
        let name = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), name_len) };
        out.push(root.join(String::from_utf16_lossy(name)));
        if info.NextEntryOffset == 0 {
            break;
        }
        offset += info.NextEntryOffset as usize;
    }
    out
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            // SAFETY: the thread handle is valid while `t` lives; cancelling
            // its blocking read makes it exit.
            unsafe {
                let _ = CancelSynchronousIo(HANDLE(t.as_raw_handle()));
            }
            // Not joined: it ends by itself right after the cancellation.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn reports_new_and_removed_files() {
        let dir = std::env::temp_dir().join(format!("lightbox-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = Watcher::new(&dir, || {}).expect("watcher");
        let f = dir.join("a.png");
        std::fs::write(&f, b"x").unwrap();
        let got = w.rx.recv_timeout(Duration::from_secs(5)).expect("change");
        assert!(got.iter().any(|p| p.ends_with("a.png")), "{got:?}");
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
