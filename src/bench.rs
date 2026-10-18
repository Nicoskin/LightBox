//! Startup stage timing for `--bench`.
//!
//! Times are measured from the moment the kernel created the process
//! (`GetProcessTimes`), so they include loader and CRT start-up, not just
//! the time since `main`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

static ENABLED: AtomicBool = AtomicBool::new(false);
static STAGES: Mutex<Vec<(&'static str, f64)>> = Mutex::new(Vec::new());
static mut PROCESS_START: u64 = 0;

fn filetime_u64(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// Enables timing. Must be called once, first thing in `main`.
pub fn init(enabled: bool) {
    if !enabled {
        return;
    }
    let mut creation = FILETIME::default();
    let mut dummy = FILETIME::default();
    // SAFETY: plain out-parameters; the pseudo-handle needs no closing.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut dummy,
            &mut FILETIME::default(),
            &mut FILETIME::default(),
        )
    };
    // SAFETY: written once before ENABLED is published, read only afterwards.
    unsafe {
        PROCESS_START = if ok.is_ok() {
            filetime_u64(creation)
        } else {
            now_100ns()
        };
    }
    ENABLED.store(true, Ordering::Release);
    mark("main");
}

fn now_100ns() -> u64 {
    // SAFETY: no preconditions.
    filetime_u64(unsafe { GetSystemTimePreciseAsFileTime() })
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// Milliseconds since process creation.
pub fn elapsed_ms() -> f64 {
    // SAFETY: see `init`.
    let start = unsafe { PROCESS_START };
    now_100ns().saturating_sub(start) as f64 / 10_000.0
}

/// Records a named stage (no-op unless `--bench`).
pub fn mark(stage: &'static str) {
    if !enabled() {
        return;
    }
    let t = elapsed_ms();
    if let Ok(mut s) = STAGES.lock() {
        s.push((stage, t));
    }
}

/// Prints all stages to the parent console (if any) and appends them to
/// `%TEMP%\lightbox-bench.log`.
pub fn report(file: &str) {
    if !enabled() {
        return;
    }
    let mut stages = match STAGES.lock() {
        Ok(s) => s.clone(),
        Err(_) => return,
    };
    stages.sort_by(|a, b| a.1.total_cmp(&b.1));

    let mut text = format!("lightbox --bench {file}\n");
    let mut prev = 0.0;
    for (name, t) in &stages {
        let _ = writeln!(text, "  {t:8.2} ms  (+{:7.2})  {name}", t - prev);
        prev = *t;
    }

    // SAFETY: attaching to the parent console has no memory-safety preconditions.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok()
        && let Ok(mut con) = std::fs::OpenOptions::new().write(true).open("CONOUT$")
    {
        let _ = con.write_all(format!("\n{text}").as_bytes());
    }
    let log = std::env::temp_dir().join("lightbox-bench.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let _ = f.write_all(text.as_bytes());
    }
}
