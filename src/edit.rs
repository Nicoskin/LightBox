//! Writing edits to files on a background thread.
//!
//! Every write goes to a temporary file in the same folder and then
//! atomically replaces the original with `ReplaceFileW` (which also keeps the
//! creation time, attributes and ACLs), so a crash or a full disk never
//! leaves a damaged photo. The original bytes are kept in a temp folder for
//! the rest of the session so that Ctrl+Z can restore them exactly.

use std::fs::{self, FileTimes, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::SystemTime;

use windows::Win32::Storage::FileSystem::{REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW};
use windows::Win32::UI::Shell::{SHCNE_UPDATEITEM, SHCNF_PATHW, SHChangeNotify};
use windows::core::HSTRING;

use crate::decode;
use crate::encode::{self, EncodeError};
use crate::i18n::tr;
use crate::orientation::Orientation;
use crate::settings::ImperfectJpeg;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Jpeg,
    Png,
}

pub enum Job {
    /// Bake `display` (the orientation the photo is shown with) into the file.
    Rotate {
        path: PathBuf,
        display: Orientation,
        /// The user's edit this write covers (echoed back).
        edit: Orientation,
        keep_dates: bool,
        imperfect: Option<ImperfectJpeg>,
        /// Then keep only this part (x, y, width, height of the turned image).
        crop: Option<[u32; 4]>,
    },
    /// Write the photo as shown (`edit` on top of its own orientation) to `dst`.
    Export {
        src: PathBuf,
        dst: PathBuf,
        edit: Orientation,
        format: ExportFormat,
        quality: u8,
    },
    /// Bake tonal adjustments and a straightening angle into the file.
    Adjust {
        path: PathBuf,
        display: Orientation,
        edit: Orientation,
        adjust: crate::adjust::Adjust,
        angle: f64,
        quality: u8,
        keep_dates: bool,
    },
    /// Put back the bytes saved before an earlier rotation (undo).
    Restore { undo: Undo },
}

/// What is needed to undo one write.
#[derive(Clone, Debug)]
pub struct Undo {
    pub path: PathBuf,
    pub backup: PathBuf,
    pub modified: Option<SystemTime>,
}

pub enum Outcome {
    Rotated {
        path: PathBuf,
        edit: Orientation,
        undo: Undo,
        /// What was saved, for the notice.
        note: &'static str,
    },
    /// The JPEG cannot be rotated exactly; ask the user and resubmit.
    NeedsJpegChoice {
        path: PathBuf,
        edit: Orientation,
        display: Orientation,
    },
    Exported {
        dst: PathBuf,
    },
    Restored {
        path: PathBuf,
    },
    Failed {
        path: PathBuf,
        error: String,
    },
}

pub struct Writer {
    tx: Option<Sender<Job>>,
    rx: Receiver<Outcome>,
    thread: Option<JoinHandle<()>>,
    backups: PathBuf,
}

impl Writer {
    /// `notify` wakes the UI thread after each finished job.
    pub fn new(notify: impl Fn() + Send + 'static) -> Self {
        let backups = std::env::temp_dir()
            .join("Lightbox")
            .join(format!("undo-{}", std::process::id()));
        let (tx, jobs) = channel::<Job>();
        let (done, rx) = channel();
        let dir = backups.clone();
        let thread = thread::Builder::new()
            .name("writer".into())
            .spawn(move || {
                let mut n = 0u32;
                for job in jobs {
                    n += 1;
                    if done.send(run(job, &dir, n)).is_err() {
                        break;
                    }
                    notify();
                }
            })
            .map_err(|e| tracing::error!("cannot start writer thread: {e}"))
            .ok();
        Self {
            tx: Some(tx),
            rx,
            thread,
            backups,
        }
    }

    pub fn submit(&self, job: Job) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(job);
        }
    }

    pub fn try_recv(&self) -> Option<Outcome> {
        self.rx.try_recv().ok()
    }

    /// Finishes queued writes (called on exit) and removes the backups.
    pub fn finish(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = fs::remove_dir_all(&self.backups);
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.finish();
    }
}

fn run(job: Job, backups: &Path, n: u32) -> Outcome {
    match job {
        Job::Rotate {
            path,
            display,
            edit,
            keep_dates,
            imperfect,
            crop,
        } => match rotate_file(&path, display, keep_dates, imperfect, crop, backups, n) {
            Ok(Some(undo)) => Outcome::Rotated {
                path,
                edit,
                undo,
                note: if crop.is_some() {
                    tr("Обрезка сохранена · Ctrl+Z — вернуть")
                } else {
                    tr("Поворот сохранён")
                },
            },
            Ok(None) => Outcome::NeedsJpegChoice {
                path,
                edit,
                display,
            },
            Err(e) => Outcome::Failed {
                path,
                error: e.to_string(),
            },
        },
        Job::Export {
            src,
            dst,
            edit,
            format,
            quality,
        } => match export(&src, &dst, edit, format, quality) {
            Ok(()) => Outcome::Exported { dst },
            Err(e) => Outcome::Failed {
                path: dst,
                error: e.to_string(),
            },
        },
        Job::Adjust {
            path,
            display,
            edit,
            adjust,
            angle,
            quality,
            keep_dates,
        } => match adjust_file(
            &path,
            display,
            (adjust, angle),
            quality,
            keep_dates,
            backups,
            n,
        ) {
            Ok(undo) => Outcome::Rotated {
                path,
                edit,
                undo,
                note: tr("Коррекция сохранена · Ctrl+Z — вернуть"),
            },
            Err(e) => Outcome::Failed {
                path,
                error: e.to_string(),
            },
        },
        Job::Restore { undo } => match restore(&undo) {
            Ok(()) => Outcome::Restored { path: undo.path },
            Err(e) => Outcome::Failed {
                path: undo.path,
                error: e.to_string(),
            },
        },
    }
}

#[derive(Debug, thiserror::Error)]
enum WriteError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Encode(#[from] EncodeError),
    #[error("{0}")]
    Decode(#[from] decode::DecodeError),
}

/// `Ok(None)`: a JPEG that needs the user's choice (see `ImperfectJpeg`).
fn rotate_file(
    path: &Path,
    display: Orientation,
    keep_dates: bool,
    imperfect: Option<ImperfectJpeg>,
    crop: Option<[u32; 4]>,
    backups: &Path,
    n: u32,
) -> Result<Option<Undo>, WriteError> {
    let data = fs::read(path)?;
    let out = if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        // A crop removes the edges anyway: partial blocks may go.
        let imperfect = imperfect.or(crop.map(|_| ImperfectJpeg::Trim));
        let turned = match encode::jpeg::rotate(&data, display, imperfect)? {
            Some(v) => v,
            None => return Ok(None),
        };
        match crop {
            Some(c) => encode::jpeg::crop(&turned, c)?.0,
            None => turned,
        }
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        encode::png::transform(&data, display, crop)?
    } else {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        encode::other::transform(&data, &ext, display, crop)?
    };
    write_with_backup(path, &data, &out, keep_dates, backups, n).map(Some)
}

/// Keeps `old` (the file's bytes) for undo, then replaces the file with
/// `new`.
fn write_with_backup(
    path: &Path,
    old: &[u8],
    new: &[u8],
    keep_dates: bool,
    backups: &Path,
    n: u32,
) -> Result<Undo, WriteError> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok();
    fs::create_dir_all(backups)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let backup = backups.join(format!("{n:04}-{name}"));
    fs::write(&backup, old)?;
    replace_file(path, new, keep_dates.then_some(modified).flatten())?;
    Ok(Undo {
        path: path.to_path_buf(),
        backup,
        modified,
    })
}

/// Tonal adjustments and straightening baked into the file (re-encoded in
/// its format; JPEG at `quality`).
/// `look`: the adjustments and the straightening angle (degrees).
fn adjust_file(
    path: &Path,
    display: Orientation,
    (adjust, angle): (crate::adjust::Adjust, f64),
    quality: u8,
    keep_dates: bool,
    backups: &Path,
    n: u32,
) -> Result<Undo, WriteError> {
    let data = fs::read(path)?;
    let img = decode::decode_data(&data, path, None)?;
    let rgba = img.to_straight_rgba8();
    let (mut px, w, h) =
        encode::orient_pixels(&rgba, img.width as usize, img.height as usize, 4, display);
    adjust.apply_srgb8(&mut px);
    let (px, w, h) = crate::adjust::straighten(&px, w, h, angle);
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let out = encode::reencode_like(&data, &ext, &px, w, h, quality)?;
    write_with_backup(path, &data, &out, keep_dates, backups, n)
}

fn restore(undo: &Undo) -> Result<(), WriteError> {
    let data = fs::read(&undo.backup)?;
    replace_file(&undo.path, &data, undo.modified)?;
    Ok(())
}

fn export(
    src: &Path,
    dst: &Path,
    edit: Orientation,
    format: ExportFormat,
    quality: u8,
) -> Result<(), WriteError> {
    let data = fs::read(src)?;
    let img = decode::decode_for_export(&data, src)?;
    let display = img.orientation.then(edit);
    let straight = img.to_straight_rgba8();
    let (px, w, h) = encode::orient_pixels(
        &straight,
        img.width as usize,
        img.height as usize,
        4,
        display,
    );
    let bytes = match format {
        ExportFormat::Jpeg => encode::encode_jpeg(&px, w, h, quality)?,
        ExportFormat::Png => encode::encode_png(&px, w, h)?,
    };
    if dst.exists() {
        replace_file(dst, &bytes, None)?;
    } else {
        let tmp = temp_name(dst);
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, dst)?;
        notify_shell(dst);
    }
    Ok(())
}

/// `.name.lbtmp` next to the target (same volume, as ReplaceFileW requires).
fn temp_name(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.lbtmp"))
}

/// Atomically replaces `path` with `bytes`; restores the modification time
/// to `keep_modified` if given.
pub fn replace_file(
    path: &Path,
    bytes: &[u8],
    keep_modified: Option<SystemTime>,
) -> io::Result<()> {
    let tmp = temp_name(path);
    fs::write(&tmp, bytes)?;
    // SAFETY: plain call with NUL-terminated paths.
    let replaced = unsafe {
        ReplaceFileW(
            &HSTRING::from(path.as_os_str()),
            &HSTRING::from(tmp.as_os_str()),
            None,
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            None,
            None,
        )
    };
    if let Err(e) = replaced {
        let _ = fs::remove_file(&tmp);
        return Err(io::Error::other(e));
    }
    if let Some(t) = keep_modified {
        OpenOptions::new()
            .write(true)
            .open(path)?
            .set_times(FileTimes::new().set_modified(t))?;
    }
    notify_shell(path);
    Ok(())
}

/// Tells Explorer the file changed (refreshes its thumbnail).
fn notify_shell(path: &Path) {
    let wide = HSTRING::from(path.as_os_str());
    // SAFETY: the string outlives the call.
    unsafe {
        SHChangeNotify(
            SHCNE_UPDATEITEM,
            SHCNF_PATHW,
            Some(wide.as_ptr().cast()),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lightbox-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample_jpeg() -> Vec<u8> {
        let px = vec![200u8; 32 * 16 * 4];
        encode::encode_jpeg(&px, 32, 16, 90).unwrap()
    }

    #[test]
    fn replace_keeps_modification_time() {
        let d = temp_dir("replace");
        let p = d.join("a.bin");
        fs::write(&p, b"old").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600 * 24 * 30);
        OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old))
            .unwrap();
        let before = fs::metadata(&p).unwrap().modified().unwrap();
        replace_file(&p, b"new bytes", Some(before)).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new bytes");
        assert_eq!(fs::metadata(&p).unwrap().modified().unwrap(), before);
        assert!(!temp_name(&p).exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn rotate_then_undo_restores_bytes_exactly() {
        let d = temp_dir("undo");
        let p = d.join("photo.jpg");
        let original = sample_jpeg();
        fs::write(&p, &original).unwrap();
        let backups = d.join("backups");
        let undo = rotate_file(&p, Orientation::ROTATE_CW, true, None, None, &backups, 1)
            .unwrap()
            .unwrap();
        let rotated = fs::read(&p).unwrap();
        assert_ne!(rotated, original);
        let img = decode::decode_bytes(&rotated, None).unwrap();
        assert_eq!((img.width, img.height), (16, 32));
        restore(&undo).unwrap();
        assert_eq!(fs::read(&p).unwrap(), original);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn writer_thread_reports_outcomes() {
        let d = temp_dir("writer");
        let p = d.join("x.jpg");
        fs::write(&p, sample_jpeg()).unwrap();
        let mut w = Writer::new(|| {});
        w.submit(Job::Rotate {
            path: p.clone(),
            display: Orientation::ROTATE_CCW,
            edit: Orientation::ROTATE_CCW,
            keep_dates: true,
            imperfect: None,
            crop: None,
        });
        w.submit(Job::Export {
            src: p.clone(),
            dst: d.join("copy.png"),
            edit: Orientation::IDENTITY,
            format: ExportFormat::Png,
            quality: 90,
        });
        w.finish(); // waits for both jobs
        let mut got = Vec::new();
        while let Some(o) = w.try_recv() {
            got.push(match o {
                Outcome::Rotated { .. } => "rotated",
                Outcome::Exported { .. } => "exported",
                _ => "other",
            });
        }
        assert_eq!(got, ["rotated", "exported"]);
        let png = decode::decode_file(&d.join("copy.png"), None).unwrap();
        assert_eq!((png.width, png.height), (16, 32));
        fs::remove_dir_all(&d).unwrap();
    }
}
