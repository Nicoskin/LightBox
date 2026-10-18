//! Single instance: a running viewer listens on a named pipe; a new launch
//! hands it the file path and exits at once (no window, no GPU set-up), so
//! opening the next photo takes milliseconds.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_INBOUND,
    ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::HSTRING;

use crate::APP_NAME;

/// Pipe of this user's session (pipes are machine-wide).
fn pipe_name() -> HSTRING {
    let mut session = 0;
    // SAFETY: valid out-pointer.
    let _ = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    let user = std::env::var("USERNAME").unwrap_or_default();
    // Per program copy: an installed viewer and a development build run
    // side by side without handing files to each other.
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let hash = exe.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    HSTRING::from(format!(
        r"\\.\pipe\{APP_NAME}-{session}-{user}-{:08x}",
        hash as u32
    ))
}

/// `true` if a viewer of this user is listening.
pub fn is_running() -> bool {
    // SAFETY: NUL-terminated name.
    unsafe { WaitNamedPipeW(&pipe_name(), 1) }.as_bool()
}

/// Hands `path` to a running viewer. `false` if none is running.
pub fn send_to_running(path: &Path) -> bool {
    let name = pipe_name();
    for _ in 0..2 {
        // SAFETY: plain file API on a pipe name; the handle is closed below.
        let handle = unsafe {
            CreateFileW(
                &name,
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };
        let h = match handle {
            Ok(h) => h,
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                // All instances busy: wait briefly for the next one.
                // SAFETY: NUL-terminated name.
                let _ = unsafe { WaitNamedPipeW(&name, 500) };
                continue;
            }
            Err(_) => return false,
        };
        let ok = write_path(h, path);
        // SAFETY: own handle.
        unsafe {
            let _ = CloseHandle(h);
        }
        return ok;
    }
    false
}

fn write_path(h: HANDLE, path: &Path) -> bool {
    // The running viewer may take the foreground (Windows only allows that
    // to the process the user launched, i.e. this one, to pass on).
    let mut pid = 0;
    // SAFETY: valid handle and out-pointer.
    if unsafe { GetNamedPipeServerProcessId(h, &mut pid) }.is_ok() {
        // SAFETY: no preconditions.
        let _ = unsafe { AllowSetForegroundWindow(pid) };
    }
    let bytes: Vec<u8> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut written = 0;
    // SAFETY: the buffer is valid for the call.
    unsafe { WriteFile(h, Some(&bytes), Some(&mut written), None) }.is_ok()
        && written as usize == bytes.len()
}

/// Starts listening; every received path arrives on the returned channel
/// and `notify` is called to wake the UI thread.
pub fn serve(notify: impl Fn() + Send + 'static) -> Receiver<PathBuf> {
    let (tx, rx) = channel();
    let spawned = thread::Builder::new().name("ipc".into()).spawn(move || {
        let name = pipe_name();
        loop {
            // SAFETY: plain pipe creation; the handle is closed each round.
            let pipe = unsafe {
                CreateNamedPipeW(
                    &name,
                    PIPE_ACCESS_INBOUND,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    0,
                    64 * 1024,
                    0,
                    None,
                )
            };
            if pipe.is_invalid() {
                tracing::error!("cannot create the pipe");
                return;
            }
            // SAFETY: valid pipe handle.
            let connected = unsafe { ConnectNamedPipe(pipe, None) };
            let ok = connected.is_ok()
                || connected
                    .as_ref()
                    .is_err_and(|e| e.code() == ERROR_PIPE_CONNECTED.to_hresult());
            if ok && let Some(path) = read_path(pipe) {
                if tx.send(path).is_err() {
                    return;
                }
                notify();
            }
            // SAFETY: own handle.
            unsafe {
                let _ = DisconnectNamedPipe(pipe);
                let _ = CloseHandle(pipe);
            }
        }
    });
    if let Err(e) = spawned {
        tracing::error!("cannot start the pipe thread: {e}");
    }
    rx
}

fn read_path(pipe: HANDLE) -> Option<PathBuf> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut n = 0;
        // SAFETY: the buffer is valid for the call.
        let r = unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut n), None) };
        if n > 0 {
            data.extend_from_slice(&buf[..n as usize]);
        }
        if r.is_err() || n == 0 || data.len() > 64 * 1024 {
            break;
        }
    }
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let s = String::from_utf16(&units).ok()?;
    (!s.is_empty()).then(|| PathBuf::from(s))
}
