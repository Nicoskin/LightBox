//! File and shell operations on a COM worker thread: recycle bin, copy /
//! move to a folder (Explorer's own progress and conflict dialogs), image
//! to the clipboard, "show in Explorer", external editor, wallpaper.
//!
//! None of these may block the UI thread: shell calls can take long (network
//! folders, confirmation dialogs) and need a single-threaded apartment.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Shell::{
    DesktopWallpaper, FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOCONFIRMMKDIR,
    FOFX_RECYCLEONDELETE, FileOperation, IDesktopWallpaper, IFileOperation,
    IFileOperationProgressSink, ILCreateFromPathW, ILFree, IShellItem, SHCreateItemFromParsingName,
    SHOpenFolderAndSelectItems, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR};

use crate::decode;
use crate::encode;
use crate::i18n::tr;
use crate::orientation::Orientation;
use crate::win::clipboard;

pub enum Job {
    /// To the recycle bin.
    Delete {
        path: PathBuf,
        confirm: bool,
    },
    Copy {
        path: PathBuf,
        to: PathBuf,
    },
    Move {
        path: PathBuf,
        to: PathBuf,
    },
    /// The picture as shown (with `edit` applied) to the clipboard; the
    /// file itself too when it has no unsaved edit.
    CopyImage {
        path: PathBuf,
        edit: Orientation,
    },
    ShowInExplorer {
        path: PathBuf,
    },
    /// `editor`: an editor's exe, or the system "edit" verb.
    OpenInEditor {
        path: PathBuf,
        editor: Option<PathBuf>,
    },
    Wallpaper {
        path: PathBuf,
        edit: Orientation,
    },
}

pub enum Outcome {
    Deleted {
        path: PathBuf,
    },
    Copied {
        to: PathBuf,
    },
    Moved {
        path: PathBuf,
        to: PathBuf,
    },
    ImageCopied,
    WallpaperSet,
    /// Cancelled by the user in a shell dialog.
    Cancelled,
    /// Done, nothing to report.
    Done,
    Failed {
        what: &'static str,
        error: String,
    },
}

pub struct FileOps {
    tx: Sender<Job>,
    rx: Receiver<Outcome>,
}

impl FileOps {
    /// `notify` wakes the UI thread after each job.
    pub fn new(hwnd: isize, notify: impl Fn() + Send + 'static) -> Self {
        let (tx, jobs) = channel::<Job>();
        let (done, rx) = channel();
        let spawned = thread::Builder::new()
            .name("fileops".into())
            .spawn(move || {
                // SAFETY: initialises COM for this thread only.
                let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                let owner = HWND(hwnd as *mut _);
                for job in jobs {
                    if done.send(run(job, owner)).is_err() {
                        break;
                    }
                    notify();
                }
            });
        if let Err(e) = spawned {
            tracing::error!("cannot start the file thread: {e}");
        }
        Self { tx, rx }
    }

    pub fn submit(&self, job: Job) {
        let _ = self.tx.send(job);
    }

    pub fn try_recv(&self) -> Option<Outcome> {
        self.rx.try_recv().ok()
    }
}

fn run(job: Job, owner: HWND) -> Outcome {
    let fail = |what: &'static str| {
        move |e: anyhow::Error| Outcome::Failed {
            what,
            error: format!("{e:#}"),
        }
    };
    match job {
        Job::Delete { path, confirm } => {
            let mut flags = FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE;
            if !confirm {
                flags |= FOF_NOCONFIRMATION;
            }
            match file_op(owner, flags, |op| unsafe {
                op.DeleteItem(&shell_item(&path)?, None::<&IFileOperationProgressSink>)
                    .context("DeleteItem")
            }) {
                Ok(true) => Outcome::Deleted { path },
                Ok(false) => Outcome::Cancelled,
                Err(e) => fail(tr("Не удалось удалить"))(e),
            }
        }
        Job::Copy { path, to } => {
            let flags = FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR;
            match file_op(owner, flags, |op| unsafe {
                op.CopyItem(
                    &shell_item(&path)?,
                    &shell_item(&to)?,
                    PCWSTR::null(),
                    None::<&IFileOperationProgressSink>,
                )
                .context("CopyItem")
            }) {
                Ok(true) => Outcome::Copied { to },
                Ok(false) => Outcome::Cancelled,
                Err(e) => fail(tr("Не удалось скопировать"))(e),
            }
        }
        Job::Move { path, to } => {
            let flags = FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR;
            match file_op(owner, flags, |op| unsafe {
                op.MoveItem(
                    &shell_item(&path)?,
                    &shell_item(&to)?,
                    PCWSTR::null(),
                    None::<&IFileOperationProgressSink>,
                )
                .context("MoveItem")
            }) {
                Ok(true) => Outcome::Moved { path, to },
                Ok(false) => Outcome::Cancelled,
                Err(e) => fail(tr("Не удалось переместить"))(e),
            }
        }
        Job::CopyImage { path, edit } => copy_image(owner, &path, edit)
            .map(|()| Outcome::ImageCopied)
            .unwrap_or_else(fail(tr("Не удалось скопировать изображение"))),
        Job::ShowInExplorer { path } => show_in_explorer(&path)
            .map(|()| Outcome::Done)
            .unwrap_or_else(fail(tr("Не удалось открыть проводник"))),
        Job::OpenInEditor { path, editor } => open_in_editor(owner, &path, editor.as_deref())
            .map(|()| Outcome::Done)
            .unwrap_or_else(fail(tr("Не удалось открыть редактор"))),
        Job::Wallpaper { path, edit } => set_wallpaper(&path, edit)
            .map(|()| Outcome::WallpaperSet)
            .unwrap_or_else(fail(tr("Не удалось установить обои"))),
    }
}

fn shell_item(path: &Path) -> Result<IShellItem> {
    // SAFETY: plain COM call with a NUL-terminated path.
    // The shell parser wants backslashes ("C:/x" fails).
    let p = path.to_string_lossy().replace('/', "\\");
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(p), None) }
        .with_context(|| format!("{}", path.display()))
}

/// Runs one IFileOperation; `Ok(false)` if the user cancelled it.
fn file_op(
    owner: HWND,
    flags: FILEOPERATION_FLAGS,
    queue: impl FnOnce(&IFileOperation) -> Result<()>,
) -> Result<bool> {
    // SAFETY: COM is initialised on this thread; all objects live for the
    // duration of the calls.
    unsafe {
        let op: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).context("IFileOperation")?;
        op.SetOwnerWindow(owner)?;
        op.SetOperationFlags(flags)?;
        queue(&op)?;
        if let Err(e) = op.PerformOperations() {
            // ERROR_CANCELLED / COPYENGINE_E_USER_CANCELLED.
            if e.code().0 as u32 == 0x8007_04C7 || e.code().0 as u32 == 0x8027_0000 {
                return Ok(false);
            }
            return Err(e).context("PerformOperations");
        }
        Ok(!op.GetAnyOperationsAborted()?.as_bool())
    }
}

/// Decodes the file at full resolution and turns it as shown.
fn oriented(path: &Path, edit: Orientation) -> Result<(Vec<u8>, usize, usize)> {
    let data = std::fs::read(path)?;
    let img = decode::decode_for_export(&data, path)?;
    let display = img.orientation.then(edit);
    let straight = img.to_straight_rgba8();
    Ok(encode::orient_pixels(
        &straight,
        img.width as usize,
        img.height as usize,
        4,
        display,
    ))
}

fn copy_image(owner: HWND, path: &Path, edit: Orientation) -> Result<()> {
    let (px, w, h) = oriented(path, edit)?;
    // PNG keeps the alpha channel for applications that read it.
    let png = encode::encode_png(&px, w, h).ok();
    let file = (edit == Orientation::IDENTITY).then_some(path);
    clipboard::set_image(owner, &px, w, h, png.as_deref(), file)
}

fn show_in_explorer(path: &Path) -> Result<()> {
    // SAFETY: the item id list is created and freed here.
    unsafe {
        let pidl = ILCreateFromPathW(&HSTRING::from(path.as_os_str()));
        if pidl.is_null() {
            bail!("{}: not found", path.display());
        }
        let r = SHOpenFolderAndSelectItems(pidl, None, 0);
        ILFree(Some(pidl));
        r.context("SHOpenFolderAndSelectItems")
    }
}

fn open_in_editor(owner: HWND, path: &Path, editor: Option<&Path>) -> Result<()> {
    let file = HSTRING::from(path.as_os_str());
    // SAFETY: ShellExecuteW with NUL-terminated strings alive for the call.
    let ok = |r: windows::Win32::Foundation::HINSTANCE| r.0 as isize > 32;
    unsafe {
        if let Some(exe) = editor {
            let args = HSTRING::from(format!("\"{}\"", path.display()));
            let r = ShellExecuteW(
                Some(owner),
                &HSTRING::from("open"),
                &HSTRING::from(exe.as_os_str()),
                &args,
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
            if ok(r) {
                return Ok(());
            }
            bail!("{}", exe.display());
        }
        // The system editor for this type, else "Open with…".
        for verb in ["edit", "openas"] {
            let r = ShellExecuteW(
                Some(owner),
                &HSTRING::from(verb),
                &file,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
            if ok(r) {
                return Ok(());
            }
        }
    }
    bail!(tr("нет программы для редактирования этого формата"))
}

/// Formats the desktop shows directly; others (and turned photos) are
/// converted to a JPEG first.
const WALLPAPER_FORMATS: [&str; 5] = ["jpg", "jpeg", "png", "bmp", "jfif"];

fn set_wallpaper(path: &Path, edit: Orientation) -> Result<()> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let file = if edit == Orientation::IDENTITY && WALLPAPER_FORMATS.contains(&ext.as_str()) {
        path.to_path_buf()
    } else {
        let (px, w, h) = oriented(path, edit)?;
        let jpeg = encode::encode_jpeg(&px, w, h, 95)?;
        let dir =
            crate::paths::local_dir().unwrap_or_else(|| std::env::temp_dir().join(crate::APP_NAME));
        std::fs::create_dir_all(&dir)?;
        let out = dir.join("wallpaper.jpg");
        std::fs::write(&out, jpeg)?;
        out
    };
    // SAFETY: COM is initialised on this thread.
    unsafe {
        let wp: IDesktopWallpaper =
            CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL).context("IDesktopWallpaper")?;
        wp.SetWallpaper(PCWSTR::null(), &HSTRING::from(file.as_os_str()))
            .context("SetWallpaper")
    }
}
