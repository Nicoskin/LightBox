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
mod decode;
mod gfx;
mod library;
mod loader;
mod metadata;
mod orientation;
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

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct Args {
    path: Option<PathBuf>,
    bench: bool,
    /// Exit right after the first frame (for repeated timing runs).
    bench_exit: bool,
}

impl Args {
    fn parse() -> Self {
        let mut args = Args {
            path: None,
            bench: false,
            bench_exit: false,
        };
        for a in std::env::args_os().skip(1) {
            match a.to_str() {
                Some("--bench") => args.bench = true,
                Some("--bench-exit") => {
                    args.bench = true;
                    args.bench_exit = true;
                }
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

    if let Err(e) = run(&args) {
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
    let decoder = thread::Builder::new()
        .name("decode".into())
        .spawn(move || decode::decode_file(&decode_path, Some(hint)))
        .context("spawn decoder")?;

    // 2. GPU stacks in parallel with decoding: first WARP (ready in ~10 ms),
    //    then the hardware device (~140 ms on some drivers). Sequential on
    //    purpose: concurrent D3D11CreateDevice calls serialise inside the
    //    runtime, which would delay WARP until the hardware device is done.
    let (gpu_tx, gpu_rx) = mpsc::channel();
    let hwnd_slot = Arc::new(AtomicIsize::new(0));
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

    let decoded = decoder
        .join()
        .map_err(|_| anyhow::anyhow!("decoder thread panicked"))?;

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

    let mut app = app::App::new(hwnd, gpu, pending_hw, path, decoded, monitor)?;
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
    Ok(())
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
