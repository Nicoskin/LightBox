//! Lightbox — a fast image viewer for Windows.
//!
//! Start-up is organised for minimum time-to-first-frame: decoding of the
//! file from the command line starts first thing in `main`, the D3D11 devices
//! (hardware and a WARP stand-in) are created on worker threads, and the
//! window is created on the UI thread meanwhile. The first frame uses
//! whichever device is ready; the hardware one takes over later if needed.

#![windows_subsystem = "windows"]

mod app;
mod bench;
mod cache;
mod color;
mod decode;
mod edit;
mod encode;
mod fileops;
mod gfx;
mod inspect;
mod install;
mod ipc;
mod keymap;
mod library;
mod loader;
mod metadata;
mod model;
mod orientation;
mod settings;
mod ui;
mod update;
mod view;
mod win;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW, PostMessageW};
use windows::core::{HSTRING, w};

use crate::decode::SizeHint;
use crate::win::monitor::Monitor;

/// Product name: window class, settings folder, pipe and registry keys.
pub const APP_NAME: &str = "Lightbox";

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    /// Show the image given (the default).
    View,
    /// `--register` / `--unregister`: file types for "Open with" (HKCU).
    Register,
    Unregister,
    /// `--background`: start in the tray without a window (resident mode).
    Background,
    /// `--uninstall`: remove the installed program (from "Installed apps").
    Uninstall,
    /// `--enable-tray`: tray mode, Space preview and start with Windows on
    /// (the installer's check box).
    EnableTray,
}

struct Args {
    path: Option<PathBuf>,
    /// `--pinned l,t,r,b`: a standalone preview window at that rectangle.
    pinned: Option<windows::Win32::Foundation::RECT>,
    /// `--peek`: a Space preview in its own window (see `App::start_peek`).
    peek: bool,
    command: Command,
    bench: bool,
    /// Exit right after the first frame (for repeated timing runs).
    bench_exit: bool,
}

impl Args {
    fn parse() -> Self {
        let mut args = Args {
            path: None,
            pinned: None,
            peek: false,
            command: Command::View,
            bench: false,
            bench_exit: false,
        };
        let mut it = std::env::args_os().skip(1);
        while let Some(a) = it.next() {
            match a.to_str() {
                Some("--pinned") => {
                    let v: Vec<i32> = it
                        .next()
                        .and_then(|r| r.to_str().map(str::to_string))
                        .unwrap_or_default()
                        .split(',')
                        .filter_map(|n| n.trim().parse().ok())
                        .collect();
                    if let [left, top, right, bottom] = v[..] {
                        args.pinned = Some(windows::Win32::Foundation::RECT {
                            left,
                            top,
                            right,
                            bottom,
                        });
                    }
                }
                Some("--peek") => args.peek = true,
                Some("--bench") => args.bench = true,
                Some("--bench-exit") => {
                    args.bench = true;
                    args.bench_exit = true;
                }
                Some("--register") => args.command = Command::Register,
                Some("--unregister") => args.command = Command::Unregister,
                Some("--background") => args.command = Command::Background,
                Some("--uninstall") => args.command = Command::Uninstall,
                Some("--enable-tray") => args.command = Command::EnableTray,
                _ if args.path.is_none() => args.path = Some(PathBuf::from(a)),
                _ => {}
            }
        }
        args
    }
}

fn main() {
    let args = Args::parse();
    bench::init(args.bench);
    init_logging();

    let exe = std::env::current_exe().unwrap_or_default();
    let result = match args.command {
        Command::Register => win::registry::register(&exe),
        Command::Unregister => win::registry::unregister(&exe),
        Command::Uninstall => install::uninstall(),
        Command::EnableTray => {
            let mut s = settings::Settings::load();
            s.resident = true;
            s.quick_look = true;
            s.autostart = true;
            s.save();
            win::registry::set_autostart(&exe, true)
        }
        // Already running: nothing to do.
        Command::Background if ipc::is_running() => Ok(()),
        Command::Background => run_background(false),
        // No file: the start screen (in the running viewer if there is one).
        Command::View if args.path.is_none() && args.pinned.is_none() && !args.bench => {
            if ipc::send_to_running(std::path::Path::new(ipc::SHOW_HOME)) {
                Ok(())
            } else {
                run_background(true)
            }
        }
        Command::View => {
            // A running viewer takes the file: no second window.
            let handed = !args.bench
                && args.pinned.is_none()
                && !args.peek
                && args.path.as_deref().is_some_and(|p| {
                    ipc::send_to_running(&std::path::absolute(p).unwrap_or(p.into()))
                });
            if handed { Ok(()) } else { run(&args) }
        }
    };
    if let Err(e) = result {
        tracing::error!("{e:#}");
        let text = HSTRING::from(format!("{e:#}"));
        // SAFETY: plain message box without an owner.
        unsafe { MessageBoxW(None, &text, w!("Lightbox"), MB_OK | MB_ICONERROR) };
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<()> {
    let Some(arg) = args.path.clone() else {
        anyhow::bail!("Usage: lightbox <image file>");
    };
    let path = std::path::absolute(&arg).unwrap_or(arg);
    let decode_path = path.clone();

    // The photo opens on the monitor under the mouse cursor.
    let monitor = Monitor::under_cursor();
    let (mw, mh) = monitor.size();

    // 1. Decode — first, it is the longest stage.
    let hint = SizeHint {
        box_width: mw,
        box_height: mh,
    };
    let hwnd_slot = Arc::new(AtomicIsize::new(0));
    let (dec_tx, dec_rx) = mpsc::channel();
    let dec_slot = Arc::clone(&hwnd_slot);
    thread::Builder::new()
        .name("decode".into())
        .spawn(move || {
            let result = std::fs::read(&decode_path).map_err(decode::DecodeError::from);
            bench::mark("file read");
            let result = result.and_then(|data| {
                // The EXIF thumbnail first: a picture within milliseconds.
                if let Some(t) = decode::decode_thumbnail_bytes(&data) {
                    let _ = dec_tx.send(app::Startup::Thumb(t));
                }
                decode::decode_data(&data, &decode_path, Some(hint))
            });
            let _ = dec_tx.send(app::Startup::Final(result));
            let h = dec_slot.load(Ordering::Acquire);
            if h != 0 {
                app::post(h, app::WM_APP_DECODED);
            }
        })
        .context("spawn decoder")?;

    // 2. GPU stacks in parallel with decoding: first WARP (ready in ~10 ms),
    //    then the hardware device (~140 ms on some drivers). Sequential on
    //    purpose: concurrent D3D11CreateDevice calls serialise inside the
    //    runtime, which would delay WARP until the hardware device is done.
    let (gpu_tx, gpu_rx) = mpsc::channel();
    let slot = Arc::clone(&hwnd_slot);
    thread::Builder::new()
        .name("gpu-init".into())
        .spawn(move || {
            let _ = gpu_tx.send(gfx::Gpu::new(gfx::Adapter::Warp, mw, mh));
            let _ = gpu_tx.send(gfx::Gpu::new(gfx::Adapter::Hardware, mw, mh));
            // If the window exists already, wake the UI thread. Otherwise the
            // UI thread finds the result itself right after creating it.
            let h = slot.load(Ordering::Acquire);
            if h != 0 {
                // SAFETY: posting to a window handle; a stale handle just fails.
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(h as *mut _)),
                        app::WM_APP_GPU_READY,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        })
        .context("spawn gpu init")?;

    // 3. Window on this (UI) thread meanwhile.
    let title = args
        .path
        .as_deref()
        .and_then(|p| p.file_name())
        .map_or_else(|| "Lightbox".into(), |n| n.to_string_lossy().into_owned());
    let hwnd = win::window::create(monitor.rect, &title, Some(app::wndproc))?;
    hwnd_slot.store(hwnd.0 as isize, Ordering::Release);
    bench::mark("window created");
    // Show it right away (without activation): until the first
    // DirectComposition commit the window is fully transparent (it has no
    // redirection surface), so this is invisible and overlaps with decoding.
    win::window::show(hwnd);
    bench::mark("window shown");

    // The decoded preview if it is ready, otherwise the EXIF thumbnail (the
    // preview then follows through WM_APP_DECODED).
    let (decoded, initial) = match dec_rx.recv() {
        Ok(app::Startup::Thumb(t)) => match dec_rx.try_recv() {
            Ok(app::Startup::Final(r)) => (r, None),
            _ => (Ok(t), Some(dec_rx)),
        },
        Ok(app::Startup::Final(r)) => (r, None),
        Err(_) => anyhow::bail!("decoder thread died"),
    };

    // Draw with the hardware device if it is already there, otherwise WARP.
    let warp = gpu_rx.recv().context("GPU init thread died")?;
    let (gpu, pending_hw) = match (warp, gpu_rx.try_recv()) {
        (_, Ok(Ok(hw))) => (hw, None),
        (Ok(warp), Err(TryRecvError::Empty)) => (warp, Some(gpu_rx)),
        (Ok(warp), _) => (warp, None), // hardware failed: stay on WARP
        (Err(_), Err(TryRecvError::Empty)) => {
            (gpu_rx.recv().context("GPU init thread died")??, None)
        }
        (Err(e), _) => return Err(e.context("no Direct3D 11 device available")),
    };
    bench::mark("joined");

    let settings = settings::Settings::load();
    let mut app = app::App::new(
        hwnd,
        gpu,
        pending_hw,
        app::Opening {
            path,
            decoded,
            initial,
        },
        monitor,
        settings,
    )?;
    if let Some(r) = args.pinned {
        app.start_pinned(r)?;
    }
    if args.peek {
        app.start_peek()?;
    }
    app.first_frame()?;
    // Activation (focus, text services) is slow; do it once the photo is up.
    win::window::activate(hwnd);
    bench::mark("activated");
    if args.bench_exit {
        app.adopt_hardware(true)?;
    } else {
        app.adopt_hardware(false)?;
    }
    let file = args
        .path
        .as_deref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    bench::report(&file);
    if args.bench_exit {
        return Ok(());
    }
    app.start_background();
    app.restart_anim_clock();
    app.install();

    win::window::run_message_loop(app::frame);
    finish()
}

/// Logging is off unless `LIGHTBOX_LOG` is set (e.g. `info`, `debug`);
/// output goes to `%TEMP%\lightbox.log`.
fn init_logging() {
    let Some(level) = std::env::var_os("LIGHTBOX_LOG") else {
        return;
    };
    let level = level
        .to_str()
        .and_then(|s| s.parse::<tracing::Level>().ok())
        .unwrap_or(tracing::Level::INFO);
    let path = std::env::temp_dir().join("lightbox.log");
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        tracing_subscriber::fmt()
            .with_max_level(level)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init();
    }
}

/// Resident start (`--background`, at sign-in): window, GPU and app are
/// ready but hidden until a file arrives through the pipe or the tray.
/// Starts hidden (tray), or with the start screen when `home`.
fn run_background(home: bool) -> Result<()> {
    let monitor = Monitor::under_cursor();
    let (mw, mh) = monitor.size();
    let gpu = gfx::Gpu::new(gfx::Adapter::Hardware, mw, mh)
        .or_else(|_| gfx::Gpu::new(gfx::Adapter::Warp, mw, mh))?;
    let hwnd = win::window::create(monitor.rect, APP_NAME, Some(app::wndproc))?;
    let mut app = app::App::new(
        hwnd,
        gpu,
        None,
        app::Opening {
            path: PathBuf::new(),
            decoded: Err(decode::DecodeError::Unsupported),
            initial: None,
        },
        monitor,
        settings::Settings::load(),
    )?;
    app.start_hidden()?;
    app.start_background();
    if home {
        app.show_home()?;
    }
    app.install();
    win::window::run_message_loop(app::frame);
    finish()
}

/// Ends the process right after the window has closed. Pending writes are
/// already finished (WM_CLOSE waits for them); tearing down the GPU device
/// and the worker threads would only keep a dead process around for a
/// second, during which a new launch could hand its file to it.
fn finish() -> ! {
    tracing::debug!("exit");
    // ExitProcess would spend ~1.5 s unloading the graphics driver; the
    // files and settings are written and the clipboard belongs to the
    // system, so nothing is lost by skipping it.
    // SAFETY: terminates this process.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    std::process::exit(0)
}
