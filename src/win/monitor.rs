use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

/// Geometry of one display, in physical pixels (the process is per-monitor
/// DPI aware via its manifest).
#[derive(Clone, Copy, Debug)]
pub struct Monitor {
    /// Whole monitor, including the taskbar.
    pub rect: RECT,
    /// Work area (without the taskbar).
    pub work: RECT,
    pub dpi: u32,
}

impl Monitor {
    /// The monitor under the mouse cursor (falls back to the nearest one).
    pub fn under_cursor() -> Self {
        let mut pt = POINT::default();
        // SAFETY: valid out-pointer. On failure (0,0) is used — still a valid point.
        let _ = unsafe { GetCursorPos(&mut pt) };
        // SAFETY: no preconditions.
        Self::from_handle(unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) })
    }

    /// The monitor a window is (mostly) on.
    pub fn of_window(hwnd: HWND) -> Self {
        // SAFETY: an invalid hwnd simply yields the nearest/primary monitor.
        Self::from_handle(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) })
    }

    fn from_handle(h: HMONITOR) -> Self {
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: cbSize is set; out-pointer is valid.
        let ok = unsafe { GetMonitorInfoW(h, &mut mi) }.as_bool();
        let (mut dx, mut dy) = (96, 96);
        // SAFETY: valid out-pointers.
        let dpi = match unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) } {
            Ok(()) => dx,
            Err(_) => 96,
        };
        if !ok {
            let fallback = RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            };
            return Self {
                rect: fallback,
                work: fallback,
                dpi,
            };
        }
        Self {
            rect: mi.rcMonitor,
            work: mi.rcWork,
            dpi,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (
            (self.rect.right - self.rect.left).max(1) as u32,
            (self.rect.bottom - self.rect.top).max(1) as u32,
        )
    }

    /// Scales a length given in 96-DPI units to this monitor's pixels.
    pub fn px(&self, dips: f64) -> f64 {
        (dips * f64::from(self.dpi) / 96.0).round()
    }
}
