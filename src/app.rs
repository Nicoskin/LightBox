//! Application state and window message handling (UI thread only).

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, TryRecvError};

use anyhow::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, PostMessageW, PostQuitMessage, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
    WM_APP, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN,
    WM_PAINT, WM_SIZE,
};

use crate::decode::{DecodeError, DecodedImage};
use crate::gfx::{Adapter, Background, Compositor, DrawParams, Filter, Gpu, GpuImage};
use crate::view::{self, View};
use crate::win::monitor::Monitor;

/// Backdrop opacity in fullscreen mode.
const DIM: f32 = 0.8;
/// Gap between a fitted image and the screen edge, in 96-DPI pixels.
const FIT_MARGIN_DIP: f64 = 24.0;
/// Checkerboard cell size, in 96-DPI pixels.
const CHECKER_DIP: f64 = 8.0;

struct Loaded {
    texture: GpuImage,
    /// Full-resolution size (the texture may be a reduced preview).
    source: (f64, f64),
    has_alpha: bool,
}

/// Posted by the hardware-GPU init thread when its device is ready.
pub const WM_APP_GPU_READY: u32 = WM_APP + 1;

/// Result of the background hardware-device creation.
pub type PendingGpu = Receiver<Result<Gpu>>;

pub struct App {
    hwnd: HWND,
    gpu: Gpu,
    compositor: Compositor,
    /// Hardware device still being created while `gpu` is WARP.
    pending_hw: Option<PendingGpu>,
    image: Option<Loaded>,
    /// CPU copy of the image, kept only until the hardware device takes
    /// over (the texture must be re-uploaded to it).
    cpu_image: Option<DecodedImage>,
    error: Option<String>,
    view: View,
    monitor: Monitor,
    background: Background,
    filter: Filter,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

impl App {
    pub fn new(
        hwnd: HWND,
        gpu: Gpu,
        pending_hw: Option<PendingGpu>,
        decoded: Result<DecodedImage, DecodeError>,
        monitor: Monitor,
    ) -> Result<Self> {
        let compositor = Compositor::new(hwnd)?;
        let mut cpu_image = None;
        let (image, error) = match decoded {
            Ok(img) => {
                let texture = GpuImage::upload(&gpu, &img)?;
                crate::bench::mark("texture upload");
                let loaded = Loaded {
                    texture,
                    source: (f64::from(img.source_width), f64::from(img.source_height)),
                    has_alpha: img.has_alpha,
                };
                if pending_hw.is_some() {
                    cpu_image = Some(img);
                }
                (Some(loaded), None)
            }
            Err(e) => {
                tracing::warn!("decode failed: {e}");
                (None, Some(e.to_string()))
            }
        };
        let mut app = Self {
            hwnd,
            gpu,
            compositor,
            pending_hw,
            image,
            cpu_image,
            error,
            view: View {
                scale: 1.0,
                center: (0.0, 0.0),
            },
            monitor,
            background: Background::Checker(monitor.px(CHECKER_DIP) as f32),
            filter: Filter::Smooth,
        };
        app.fit();
        Ok(app)
    }

    /// Installs the app as the window's message handler.
    pub fn install(self) {
        APP.with(|a| *a.borrow_mut() = Some(self));
    }

    /// Draws, presents and commits the first frame. The window must already
    /// be shown (see `main`).
    pub fn first_frame(&mut self) -> Result<()> {
        self.render()?;
        crate::bench::mark("present");
        self.compositor.set_content(&self.gpu.swap_chain)?;
        self.compositor.commit()?;
        if crate::bench::enabled() {
            self.compositor.wait()?;
            crate::bench::mark(match self.gpu.adapter() {
                Adapter::Hardware => "first frame on screen (hardware)",
                Adapter::Warp => "first frame on screen (warp)",
            });
        }
        Ok(())
    }

    /// Switches to the hardware device if it has become available. With
    /// `block`, waits for it (used by `--bench-exit`).
    pub fn adopt_hardware(&mut self, block: bool) -> Result<()> {
        let Some(rx) = &self.pending_hw else {
            return Ok(());
        };
        let result = if block {
            rx.recv().map_err(|_| TryRecvError::Disconnected)
        } else {
            rx.try_recv()
        };
        let gpu = match result {
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) => {
                self.pending_hw = None;
                self.cpu_image = None;
                return Ok(());
            }
            Ok(Err(e)) => {
                tracing::warn!("hardware GPU unavailable, staying on WARP: {e:#}");
                self.pending_hw = None;
                self.cpu_image = None;
                return Ok(());
            }
            Ok(Ok(gpu)) => gpu,
        };
        self.pending_hw = None;
        self.switch_gpu(gpu)
    }

    /// Moves rendering to `gpu` without a visible glitch: the new swap chain
    /// gets a complete frame before the visual switches to it.
    fn switch_gpu(&mut self, mut gpu: Gpu) -> Result<()> {
        let (w, h) = self.gpu.size();
        gpu.resize(w, h)?;
        if let (Some(img), Some(cpu)) = (&mut self.image, &self.cpu_image) {
            img.texture = GpuImage::upload(&gpu, cpu)?;
        }
        self.cpu_image = None;
        let _old = std::mem::replace(&mut self.gpu, gpu);
        self.render()?;
        self.compositor.set_content(&self.gpu.swap_chain)?;
        self.compositor.commit()?;
        if crate::bench::enabled() {
            self.compositor.wait()?;
        }
        crate::bench::mark("switched to hardware gpu");
        Ok(())
    }

    fn viewport(&self) -> (f64, f64) {
        let (w, h) = self.gpu.size();
        (f64::from(w), f64::from(h))
    }

    fn fit(&mut self) {
        if let Some(img) = &self.image {
            self.view = View::fit(img.source, self.viewport(), self.monitor.px(FIT_MARGIN_DIP));
        }
    }

    fn render(&mut self) -> Result<()> {
        let vp = self.viewport();
        let mut origin = [0.0f32; 2];
        let image = self.image.as_ref().and_then(|img| {
            let rect = self.view.image_rect(img.source, vp);
            origin = [rect.x0 as f32, rect.y0 as f32];
            view::visible_quad(rect, vp).map(|q| (&img.texture, q))
        });
        let has_alpha = self.image.as_ref().is_some_and(|i| i.has_alpha);
        let params = DrawParams {
            clear: [0.0, 0.0, 0.0, DIM],
            image,
            image_origin: origin,
            orientation: 1,
            // Skip the background pass entirely for opaque images.
            background: if has_alpha {
                self.background
            } else {
                Background::None
            },
            filter: self.filter,
        };
        // TODO(M1): draw `self.error` as text (Direct2D).
        let _ = &self.error;
        self.gpu.renderer.draw(&self.gpu, &params)?;
        self.gpu.present()
    }

    /// Re-covers the monitor the window is on (resolution or DPI changed).
    fn cover_monitor(&mut self) -> Result<()> {
        self.monitor = Monitor::of_window(self.hwnd);
        let r = self.monitor.rect;
        // SAFETY: own window. WM_SIZE sent re-entrantly from here falls back
        // to DefWindowProc (the app is borrowed), so resize explicitly below.
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
        }
        self.background = Background::Checker(self.monitor.px(CHECKER_DIP) as f32);
        let (w, h) = self.monitor.size();
        self.on_size(w, h)
    }

    fn on_size(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(()); // minimised
        }
        self.gpu.resize(width, height)?;
        self.fit();
        self.render()
    }

    fn on_key(&mut self, vk: u16) -> Result<Option<LRESULT>> {
        match vk {
            k if k == VK_ESCAPE.0 => {
                // SAFETY: posting to our own window.
                unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))? };
            }
            // T: checkerboard ↔ nothing behind transparent areas.
            k if k == u16::from(b'T') => {
                self.background = match self.background {
                    Background::Checker(_) => Background::None,
                    _ => Background::Checker(self.monitor.px(CHECKER_DIP) as f32),
                };
                self.render()?;
            }
            // N: smoothing on/off (nearest-neighbour when magnifying).
            k if k == u16::from(b'N') => {
                self.filter = match self.filter {
                    Filter::Smooth => Filter::Pixelated,
                    _ => Filter::Smooth,
                };
                self.render()?;
            }
            _ => return Ok(None),
        }
        Ok(Some(LRESULT(0)))
    }

    fn handle(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Result<Option<LRESULT>> {
        match msg {
            WM_KEYDOWN => self.on_key(wparam.0 as u16),
            WM_PAINT => {
                self.render()?;
                // SAFETY: own window.
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
                Ok(Some(LRESULT(0)))
            }
            WM_ERASEBKGND => Ok(Some(LRESULT(1))),
            WM_SIZE => {
                let (w, h) = (lparam.0 & 0xffff, (lparam.0 >> 16) & 0xffff);
                self.on_size(w as u32, h as u32)?;
                Ok(Some(LRESULT(0)))
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED => {
                self.cover_monitor()?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_GPU_READY => {
                self.adopt_hardware(false)?;
                Ok(Some(LRESULT(0)))
            }
            _ => Ok(None),
        }
    }
}

/// Window procedure. Dispatches to the thread-local [`App`]; messages that
/// arrive re-entrantly (while the app is already borrowed) or before the app
/// is installed go to `DefWindowProcW`.
pub extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        // SAFETY: no preconditions.
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    let handled = APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        let app = guard.as_mut()?;
        match app.handle(msg, wparam, lparam) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("message {msg:#x}: {e:#}");
                None
            }
        }
    });
    // SAFETY: standard default processing.
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}
