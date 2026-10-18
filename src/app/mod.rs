//! Application state and window message handling (UI thread only).
//!
//! - `nav`: folder library, decode scheduling, showing the current file;
//! - `input`: keyboard and mouse;
//! - `mode`: fullscreen ↔ windowed switching and window geometry.

mod input;
mod mode;
mod nav;
mod textures;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, KillTimer, MINMAXINFO, PostQuitMessage, SetTimer, WM_APP, WM_DESTROY,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_NCCALCSIZE, WM_PAINT, WM_TIMER,
};

use crate::cache::{self, Cache};
use crate::decode::{DecodeError, DecodedImage};
use crate::gfx::{Adapter, Background, Compositor, DrawParams, Filter, Gpu, GpuImage, Layer, Ui};
use crate::loader::Loader;
use crate::view::{self, Rect, View};
use crate::win::monitor::Monitor;
use textures::Textures;

/// Posted by the hardware-GPU init thread when its device is ready.
pub const WM_APP_GPU_READY: u32 = WM_APP + 1;
/// Posted by decoder threads after each finished decode.
const WM_APP_DECODED: u32 = WM_APP + 2;
/// Posted by the folder-scan thread.
const WM_APP_LIBRARY: u32 = WM_APP + 3;

/// Backdrop opacity in fullscreen mode.
const DIM: f32 = 0.8;
/// Background of the windowed mode when the photo does not fill the window
/// (#1E1E1E as linear premultiplied RGBA).
const WINDOW_BG: [f32; 4] = [0.0129, 0.0129, 0.0129, 1.0];
/// Gap between a fitted image and the screen edge, in 96-DPI pixels.
const FIT_MARGIN_DIP: f64 = 24.0;
/// Checkerboard cell size, in 96-DPI pixels.
const CHECKER_DIP: f64 = 8.0;
/// Animation lengths (`None` disables animations — a setting in M6).
const ZOOM_ANIM: Option<Duration> = Some(Duration::from_millis(100));
const MODE_ANIM: Option<Duration> = Some(Duration::from_millis(150));
const TURN_ANIM: Option<Duration> = Some(Duration::from_millis(220));
const OPEN_ANIM: Option<Duration> = Some(Duration::from_millis(200));
/// Page turn: how far (fraction of the viewport width) the images slide.
const TURN_SHIFT: f64 = 0.06;
/// The zoom badge stays this long after the last zoom, then fades out.
const BADGE_HOLD_MS: u32 = 1000;
const BADGE_FADE: Duration = Duration::from_millis(300);
const BADGE_TIMER: usize = 1;

/// Result of the background hardware-device creation.
pub type PendingGpu = Receiver<Result<Gpu>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Fullscreen,
    Windowed,
}

enum AnimKind {
    /// Zoom / fit change; `anchor` keeps the image point under it fixed.
    View {
        from: View,
        to: View,
        anchor: Option<(f64, f64)>,
    },
    /// Mode transition (or the opening zoom) inside the current window: the
    /// image moves between two screen rectangles while the backdrop and the
    /// image fade.
    Mode {
        from: Rect,
        to: Rect,
        dim: (f32, f32),
        opacity: (f32, f32),
        /// Windowed target rectangle (screen) to switch to at the end.
        enter_windowed: Option<Rect>,
        /// Exact view to settle on at the end.
        end_view: Option<View>,
    },
    /// Page turn: the previous image slides out and fades while the new one
    /// slides in (`shift` pixels, signed by direction). Without `old` (windowed
    /// mode) the new image just fades in.
    Turn {
        old: Option<(Rc<GpuImage>, View)>,
        shift: f64,
    },
}

struct Anim {
    kind: AnimKind,
    start: Instant,
    duration: Duration,
    /// Frames drawn so far (diagnostics).
    frames: u32,
}

/// An in-progress left-button drag (pan) or click.
struct Drag {
    start: (f64, f64),
    last: (f64, f64),
    moved: bool,
}

pub struct App {
    hwnd: HWND,
    gpu: Gpu,
    compositor: Compositor,
    /// Hardware device still being created while `gpu` is WARP.
    pending_hw: Option<PendingGpu>,
    monitor: Monitor,
    mode: Mode,

    // Files.
    files: Vec<PathBuf>,
    index: usize,
    current: PathBuf,
    /// +1 / -1: direction of the last step, for prefetching.
    direction: isize,
    library_rx: Option<Receiver<Vec<PathBuf>>>,
    cache: Cache,
    loader: Option<Loader>,
    textures: Textures,
    /// Image on screen (may still be the previous file while the current
    /// one is decoding).
    shown: Option<(String, Rc<GpuImage>)>,
    /// Why the current file could not be opened.
    error: Option<String>,

    // View.
    view: View,
    /// The view follows "fit" on resize until the user zooms or pans.
    fitted: bool,
    dim: f32,
    /// Opacity of the image (opening animation).
    opacity: f32,
    anim: Option<Anim>,
    /// Zoom badge: shown, and since when it fades out.
    badge: bool,
    badge_fade: Option<Instant>,
    /// Checkerboard (vs. nothing) behind transparent areas.
    checker: bool,
    filter: Filter,

    // Input.
    drag: Option<Drag>,
    wheel_accum: i32,
    /// Navigation driven by key auto-repeat: no page-turn animation.
    fast_nav: bool,
    mode_switched_at: Instant,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

impl App {
    pub fn new(
        hwnd: HWND,
        gpu: Gpu,
        pending_hw: Option<PendingGpu>,
        path: PathBuf,
        decoded: Result<DecodedImage, DecodeError>,
        monitor: Monitor,
    ) -> Result<Self> {
        let compositor = Compositor::new(hwnd)?;
        let mut app = Self {
            hwnd,
            gpu,
            compositor,
            pending_hw,
            monitor,
            mode: Mode::Fullscreen,
            files: vec![path.clone()],
            index: 0,
            current: path,
            direction: 1,
            library_rx: None,
            cache: Cache::new(cache::DEFAULT_BUDGET),
            loader: None,
            textures: Textures::default(),
            shown: None,
            error: None,
            view: View {
                scale: 1.0,
                center: (0.0, 0.0),
            },
            fitted: true,
            dim: DIM,
            opacity: 1.0,
            anim: None,
            badge: false,
            badge_fade: None,
            checker: true,
            filter: Filter::Smooth,
            drag: None,
            wheel_accum: 0,
            fast_nav: false,
            mode_switched_at: Instant::now(),
        };
        let current = app.current.clone();
        app.store_result(
            &current,
            decoded.map(std::sync::Arc::new).map_err(|e| e.to_string()),
        );
        app.show_current()?;
        crate::bench::mark("texture upload");
        Ok(app)
    }

    /// Installs the app as the window's message handler.
    pub fn install(self) {
        APP.with(|a| *a.borrow_mut() = Some(self));
    }

    /// Draws, presents and commits the first frame. The window must already
    /// be shown (see `main`).
    pub fn first_frame(&mut self) -> Result<()> {
        self.start_open_anim();
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
                return Ok(());
            }
            Ok(Err(e)) => {
                tracing::warn!("hardware GPU unavailable, staying on WARP: {e:#}");
                self.pending_hw = None;
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
        self.textures.clear();
        let new_tex = match (&self.shown, self.cache.image(&self.current)) {
            (Some(_), Some(img)) => Some(Rc::new(GpuImage::upload(&gpu, img)?)),
            _ => None,
        };
        let _old = std::mem::replace(&mut self.gpu, gpu);
        self.shown = new_tex.map(|t| {
            let key = cache::key(&self.current);
            self.textures.insert(key.clone(), Rc::clone(&t));
            (key, t)
        });
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

    fn ui_scale(&self) -> f64 {
        f64::from(self.monitor.dpi) / 96.0
    }

    fn shown_image(&self) -> Option<&GpuImage> {
        self.shown.as_ref().map(|(_, t)| &**t)
    }

    /// "Fit" for the current mode: fullscreen fits within a margin without
    /// enlarging; windowed fills the window.
    fn fit_view(&self) -> Option<View> {
        let img = self.shown_image()?;
        Some(match self.mode {
            Mode::Fullscreen => View::fit(
                img.display,
                self.viewport(),
                self.monitor.px(FIT_MARGIN_DIP),
                false,
            ),
            Mode::Windowed => View::fit(img.display, self.viewport(), 0.0, true),
        })
    }

    /// Image rectangle on screen (viewport coordinates), if an image is shown.
    fn image_rect(&self) -> Option<Rect> {
        let img = self.shown_image()?;
        Some(self.view.image_rect(img.display, self.viewport()))
    }

    /// One image layer: `view` placement at `opacity`.
    fn layer<'a>(&self, img: &'a GpuImage, view: &View, opacity: f32) -> Option<Layer<'a>> {
        let vp = self.viewport();
        let rect = view.image_rect(img.display, vp);
        let quad = view::visible_quad(rect, vp)?;
        Some(Layer {
            image: img,
            quad,
            origin: [rect.x0 as f32, rect.y0 as f32],
            // Skip the background pass entirely for opaque images.
            background: if img.has_alpha && self.checker {
                Background::Checker(self.monitor.px(CHECKER_DIP) as f32)
            } else {
                Background::None
            },
            opacity,
        })
    }

    /// Eased progress (0..1) of the running animation.
    fn anim_t(&self) -> f64 {
        self.anim.as_ref().map_or(1.0, |a| {
            let raw = (a.start.elapsed().as_secs_f64() / a.duration.as_secs_f64()).min(1.0);
            1.0 - (1.0 - raw).powi(3) // ease-out cubic
        })
    }

    fn render(&mut self) -> Result<()> {
        let shown = self.shown.as_ref().map(|(_, t)| Rc::clone(t));
        let mut layers = Vec::with_capacity(2);
        match (&self.anim, &shown) {
            (
                Some(Anim {
                    kind: AnimKind::Turn { old, shift },
                    ..
                }),
                Some(new),
            ) => {
                let t = self.anim_t();
                if let Some((old_img, old_view)) = old {
                    let v = old_view.pan(-shift * t, 0.0);
                    layers.extend(self.layer(old_img, &v, (1.0 - t) as f32));
                }
                let v = self.view.pan(shift * (1.0 - t), 0.0);
                layers.extend(self.layer(new, &v, t as f32));
            }
            (_, Some(img)) => layers.extend(self.layer(img, &self.view, self.opacity)),
            (_, None) => {}
        }
        // Mode transitions run in fullscreen geometry, so they use `dim`.
        let clear = match self.mode {
            Mode::Windowed => WINDOW_BG,
            Mode::Fullscreen => [0.0, 0.0, 0.0, self.dim],
        };
        let params = DrawParams {
            clear,
            layers: &layers,
            filter: self.filter,
        };
        let ui = Ui {
            scale: self.ui_scale() as f32,
            message: self.error.as_ref().map(|e| {
                let name = self
                    .current
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("Не удалось открыть файл\n{name}: {e}")
            }),
            badge: self.badge_text(),
        };
        self.gpu.draw(&params, &ui)
    }

    /// Zoom level badge ("150 %") with its current opacity.
    fn badge_text(&self) -> Option<(String, f32)> {
        if !self.badge || self.shown.is_none() {
            return None;
        }
        let opacity = self.badge_fade.map_or(1.0, |f| {
            1.0 - (f.elapsed().as_secs_f32() / BADGE_FADE.as_secs_f32()).min(1.0)
        });
        let pct = self.zoom_target().scale * 100.0;
        let text = if pct < 10.0 {
            format!("{pct:.1} %")
        } else {
            format!("{pct:.0} %")
        };
        Some((text, opacity))
    }

    /// Shows the zoom badge and (re)starts its hide timer.
    fn show_badge(&mut self) {
        self.badge = true;
        self.badge_fade = None;
        // SAFETY: own window; re-arming an existing timer id just resets it.
        unsafe { SetTimer(Some(self.hwnd), BADGE_TIMER, BADGE_HOLD_MS, None) };
    }

    /// Target of a running zoom animation, or the current view.
    fn zoom_target(&self) -> View {
        match &self.anim {
            Some(Anim {
                kind: AnimKind::View { to, .. },
                ..
            }) => *to,
            _ => self.view,
        }
    }

    /// Advances animations by one frame. Returns `true` while something is
    /// still moving (the message loop then keeps calling at display rate).
    fn frame(&mut self) -> bool {
        let mut moving = false;
        let mut dirty = false;
        if let Some(anim) = &self.anim {
            let raw = (anim.start.elapsed().as_secs_f64() / anim.duration.as_secs_f64()).min(1.0);
            if raw >= 1.0 {
                // Renders the final state itself.
                if let Err(e) = self.finish_anim() {
                    tracing::error!("animation end: {e:#}");
                }
            } else {
                let t = self.anim_t();
                let vp = self.viewport();
                match &anim.kind {
                    AnimKind::View { from, to, anchor } => {
                        self.view = from.interpolate(to, t, *anchor, vp);
                    }
                    AnimKind::Mode {
                        from,
                        to,
                        dim,
                        opacity,
                        ..
                    } => {
                        if let Some(img) = self.shown_image() {
                            self.view = View::placed(img.display, from.lerp(to, t), vp);
                        }
                        self.dim = dim.0 + (dim.1 - dim.0) * t as f32;
                        self.opacity = opacity.0 + (opacity.1 - opacity.0) * t as f32;
                    }
                    AnimKind::Turn { .. } => {} // drawn from `anim_t` in render
                }
                if let Some(a) = &mut self.anim {
                    a.frames += 1;
                }
                moving = true;
                dirty = true;
            }
        }
        if let Some(f) = self.badge_fade {
            dirty = true;
            if f.elapsed() >= BADGE_FADE {
                self.badge = false;
                self.badge_fade = None;
            } else {
                moving = true;
            }
        }
        if dirty && let Err(e) = self.render() {
            tracing::error!("render: {e:#}");
        }
        moving
    }

    /// Jumps a running animation to its end state.
    fn finish_anim(&mut self) -> Result<()> {
        let Some(anim) = self.anim.take() else {
            return Ok(());
        };
        tracing::debug!(
            "animation {} done: {} frames in {:.0} ms",
            match anim.kind {
                AnimKind::View { .. } => "zoom",
                AnimKind::Mode { .. } => "mode/open",
                AnimKind::Turn { .. } => "turn",
            },
            anim.frames,
            anim.start.elapsed().as_secs_f64() * 1000.0
        );
        match anim.kind {
            AnimKind::View { to, .. } => self.view = to,
            AnimKind::Mode {
                dim,
                opacity,
                enter_windowed,
                end_view,
                ..
            } => {
                self.dim = dim.1;
                self.opacity = opacity.1;
                if let Some(v) = end_view {
                    self.view = v;
                }
                if let Some(r) = enter_windowed {
                    return self.enter_windowed(r);
                }
            }
            AnimKind::Turn { .. } => {}
        }
        self.schedule();
        self.render()
    }

    /// Opening animation: the photo grows from the centre while the
    /// backdrop darkens. The first frame is already visible (not empty).
    fn start_open_anim(&mut self) {
        let (Some(duration), Some(img), Some(fit)) = (
            OPEN_ANIM,
            self.shown_image().map(|i| i.display),
            self.fit_view(),
        ) else {
            return;
        };
        let vp = self.viewport();
        let to = fit.image_rect(img, vp);
        let (cx, cy) = to.center();
        let k = 0.82;
        let from = Rect {
            x0: cx - to.width() * k / 2.0,
            y0: cy - to.height() * k / 2.0,
            x1: cx + to.width() * k / 2.0,
            y1: cy + to.height() * k / 2.0,
        };
        self.view = View::placed(img, from, vp);
        self.dim = 0.3;
        self.opacity = 0.35;
        self.anim = Some(Anim {
            kind: AnimKind::Mode {
                from,
                to,
                dim: (self.dim, DIM),
                opacity: (self.opacity, 1.0),
                enter_windowed: None,
                end_view: Some(fit),
            },
            start: Instant::now(),
            duration,
            frames: 0,
        });
    }

    /// Restarts the clock of the running animation (after the slow window
    /// activation, so that no part of the opening animation is skipped).
    pub fn restart_anim_clock(&mut self) {
        if let Some(a) = &mut self.anim {
            a.start = Instant::now();
        }
    }

    fn on_timer(&mut self, id: usize) {
        if id == BADGE_TIMER {
            // SAFETY: own window and timer.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), BADGE_TIMER);
            }
            if self.badge {
                self.badge_fade = Some(Instant::now());
            }
        }
    }

    fn handle(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Result<Option<LRESULT>> {
        match msg {
            WM_PAINT => {
                self.render()?;
                // SAFETY: own window.
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
                Ok(Some(LRESULT(0)))
            }
            WM_ERASEBKGND => Ok(Some(LRESULT(1))),
            WM_TIMER => {
                self.on_timer(wparam.0);
                Ok(Some(LRESULT(0)))
            }
            WM_APP_GPU_READY => {
                self.adopt_hardware(false)?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_DECODED => {
                self.on_decoded()?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_LIBRARY => {
                self.on_library()?;
                Ok(Some(LRESULT(0)))
            }
            _ => {
                if let Some(r) = self.handle_window(msg, wparam, lparam)? {
                    return Ok(Some(r));
                }
                self.handle_input(msg, wparam, lparam)
            }
        }
    }
}

/// Runs one animation frame of the installed app (see the message loop).
pub fn frame() -> bool {
    APP.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut guard| guard.as_mut().map(App::frame))
            .unwrap_or(false)
    })
}

/// Window procedure. Dispatches to the thread-local [`App`]; messages that
/// arrive re-entrantly (while the app is already borrowed) or before the app
/// is installed go to `DefWindowProcW`.
pub extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Stateless geometry answers, handled here because they also arrive
    // re-entrantly from our own SetWindowPos calls (when the app is busy).
    match msg {
        WM_DESTROY => {
            // SAFETY: no preconditions.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        // No visible frame in either mode: the client area is the whole window.
        WM_NCCALCSIZE if wparam.0 != 0 => return LRESULT(0),
        // Allow small windows (tiny images); the system default for
        // captioned windows is ~136×39.
        WM_GETMINMAXINFO => {
            // SAFETY: for WM_GETMINMAXINFO, lParam points to a MINMAXINFO.
            let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = 32;
            mmi.ptMinTrackSize.y = 32;
            return LRESULT(0);
        }
        _ => {}
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

/// Posts `msg` to a window identified by its raw handle (callable from any
/// thread; a stale handle just fails).
fn post(hwnd: isize, msg: u32) {
    // SAFETY: PostMessageW has no memory-safety preconditions.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(HWND(hwnd as *mut _)),
            msg,
            WPARAM(0),
            LPARAM(0),
        );
    }
}
