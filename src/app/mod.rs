//! Application state and window message handling (UI thread only).
//!
//! - `nav`: folder library, decode scheduling, showing the current file;
//! - `input`: keyboard and mouse;
//! - `mode`: fullscreen ↔ windowed switching and window geometry.

mod animation;
mod chrome;
mod files;
mod input;
mod inspector;
mod mode;
mod nav;
mod panel;
mod save;
mod system;
mod textures;
mod tooltip;
mod vector;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, KillTimer, MINMAXINFO, PostQuitMessage, SetTimer, WM_APP, WM_CLOSE, WM_DESTROY,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_NCCALCSIZE, WM_PAINT, WM_TIMER,
};

use crate::cache::{self, Cache};
use crate::decode::{DecodeError, DecodedImage};
use crate::gfx::{
    Adapter, Background, Compositor, DrawParams, Filter, Gpu, GpuImage, Layer, Part, Ui,
};
use crate::loader::Loader;
use crate::orientation::Orientation;
use crate::settings::Settings;
use crate::ui::thumbs::Thumbnails;
use crate::view::{self, Rect, View};
use crate::win::monitor::Monitor;
use textures::Textures;

/// Posted by the hardware-GPU init thread when its device is ready.
pub const WM_APP_GPU_READY: u32 = WM_APP + 1;
/// Posted by decoder threads after each finished decode.
pub const WM_APP_DECODED: u32 = WM_APP + 2;

/// What the start-up decoder sends: maybe the EXIF thumbnail, then the result.
pub enum Startup {
    Thumb(DecodedImage),
    Final(Result<DecodedImage, DecodeError>),
}

/// The file given on the command line and what is known about it so far.
pub struct Opening {
    pub path: PathBuf,
    /// The start-up decode result, or the EXIF thumbnail standing in for it.
    pub decoded: Result<DecodedImage, DecodeError>,
    /// Set while the start-up decoder is still producing the preview.
    pub initial: Option<Receiver<Startup>>,
}
/// Posted by the folder-scan thread.
const WM_APP_LIBRARY: u32 = WM_APP + 3;

/// Windowed mode: the frame around the photo and any area it does not
/// cover (#2B2B2B as linear premultiplied RGBA).
const WINDOW_BG: [f32; 4] = [0.0242, 0.0242, 0.0242, 1.0];
/// Gap between a fitted image and the screen edge, in 96-DPI pixels.
const FIT_MARGIN_DIP: f64 = 24.0;
/// Pixel grid from this many screen pixels per image pixel (800 %).
const GRID_MIN_SCALE: f64 = 8.0;
/// Checkerboard cell size, in 96-DPI pixels.
const CHECKER_DIP: f64 = 8.0;
/// Animation lengths, milliseconds (all off with `animations = false`).
const ZOOM_ANIM: u64 = 100;
const MODE_ANIM: u64 = 150;
const TURN_ANIM: u64 = 220;
const OPEN_ANIM: u64 = 200;
/// Page turn: how far (fraction of the viewport width) the images slide.
const TURN_SHIFT: f64 = 0.06;
/// The zoom badge stays this long after the last zoom, then fades out.
const BADGE_HOLD_MS: u32 = 1000;
const BADGE_FADE: Duration = Duration::from_millis(300);
const BADGE_TIMER: usize = 1;
/// The panel hides this long after the last mouse movement.
const PANEL_HOLD_MS: u32 = 2000;
const PANEL_FADE: Duration = Duration::from_millis(180);
const PANEL_TIMER: usize = 2;
/// Hover delay before the transparency menu opens.
const MENU_TIMER: usize = 3;
/// Hover delay before a tooltip appears.
const TOOLTIP_TIMER: usize = 4;
/// Next animation frame.
const ANIM_TIMER: usize = 5;
/// Filmstrip glide: time constant of the exponential approach, seconds.
const STRIP_TAU: f64 = 0.06;
/// Posted by the thumbnail thread.
const WM_APP_THUMB: u32 = WM_APP + 4;
/// Posted by the writer thread after each finished job.
const WM_APP_SAVED: u32 = WM_APP + 5;
/// Posted by the vector thread with a sharper rendering of an SVG.
const WM_APP_VECTOR: u32 = WM_APP + 6;
/// Posted by the info thread with a file's metadata.
const WM_APP_INFO: u32 = WM_APP + 7;
/// Posted by the file thread after each operation.
const WM_APP_FILEOP: u32 = WM_APP + 8;
/// Posted by the pipe thread: a new launch handed over a file.
const WM_APP_OPEN: u32 = WM_APP + 9;
/// Posted by the keyboard hook: Space in Explorer (wParam: its window).
const WM_APP_QUICKLOOK: u32 = WM_APP + 10;
/// Tray icon events (lParam: the mouse message).
const WM_APP_TRAY: u32 = WM_APP + 11;
/// Posted by the frame decoder: all frames of an animation are ready.
const WM_APP_FRAMES: u32 = WM_APP + 12;

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
        old: Option<(String, Rc<GpuImage>, View)>,
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
    /// Start-up decoder, while the first frame shows the EXIF thumbnail.
    initial: Option<Receiver<Startup>>,
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
    settings: Settings,

    // Panel.
    /// Panel opacity now and where it is heading (0 hidden, 1 shown).
    panel_alpha: f32,
    panel_target: f32,
    panel_tick: Instant,
    hot: Option<panel::Hot>,
    pressed: Option<panel::Hot>,
    thumbs: Option<Thumbnails>,
    /// Rotations / mirrors made by the user, per file (cache key); written
    /// to the file in M2.
    edits: HashMap<String, Orientation>,
    /// Background file writer (rotations, "Save as", undo).
    writer: Option<crate::edit::Writer>,
    /// Files with a rotation being written.
    saving: HashSet<String>,
    /// Saved files being decoded again, with the edit that was written.
    reloading: HashMap<String, Orientation>,
    /// Writes that Ctrl+Z can undo, most recent last.
    undo: Vec<crate::edit::Undo>,
    /// Status message shown in the badge.
    notice: Option<(String, Instant)>,
    /// Settings card shown, and the item under the mouse.
    card_open: bool,
    card_hot: Option<crate::ui::settings_card::Hit>,
    /// Filmstrip scroll position (file index, fractional while gliding)
    /// and when its glide was last advanced.
    strip_pos: f64,
    strip_tick: Option<Instant>,
    /// Transparency-background menu shown.
    menu_open: bool,
    /// Straightening dial shown, the current free rotation (degrees, view
    /// only) and an ongoing ruler drag (start x, angle then).
    dial_open: bool,
    angle: f64,
    dial_drag: Option<(f64, f64)>,
    /// Channel view: 0 = colour, 1..3 = R, G, B, 4 = alpha (Shift+1..4).
    channel: u32,
    /// Control whose tooltip is shown.
    tooltip: Option<panel::Hot>,
    /// Animated GIF / WebP / APNG: playback and frames being decoded.
    playback: Option<animation::Playback>,
    frames_rx: Option<animation::FramesRx>,
    /// Pixel under the cursor (colour readout).
    probe: Option<inspector::Probe>,
    info_state: inspector::InfoState,
    hist_state: inspector::HistState,
    /// Recycle bin, copy / move, clipboard, shell commands.
    fileops: Option<crate::fileops::FileOps>,
    /// Copy / move waiting for the rotation of the photo to be written.
    after_save: Option<crate::fileops::Job>,
    /// Files handed over by new launches.
    open_rx: Option<Receiver<PathBuf>>,
    /// Resident: the window is hidden (in the tray), or waits to be shown
    /// until the requested image is decoded.
    hidden: bool,
    pending_show: bool,
    /// Opened by Space in Explorer (Space closes it again).
    quicklook: bool,
    /// A pinned preview: a standalone window for one photo (`--pinned`).
    pinned: bool,
    /// Tray icon present; a real exit is under way.
    tray: bool,
    quitting: bool,
    /// Thread running the QuickLook keyboard hook.
    hook_thread: Option<u32>,
    /// Registered "TaskbarCreated" message.
    taskbar_msg: u32,
    /// SVG: renders the visible region at screen resolution, and the
    /// latest such rendering.
    vector: Option<vector::VectorWorker>,
    detail: Option<vector::Detail>,

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
        opening: Opening,
        monitor: Monitor,
        settings: Settings,
    ) -> Result<Self> {
        let Opening {
            path,
            decoded,
            initial,
        } = opening;
        let compositor = Compositor::new(hwnd)?;
        let mut app = Self {
            hwnd,
            gpu,
            compositor,
            pending_hw,
            initial,
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
                angle: 0.0,
            },
            fitted: true,
            dim: settings.dim(),
            opacity: 1.0,
            anim: None,
            badge: false,
            badge_fade: None,
            panel_alpha: 1.0,
            panel_target: 1.0,
            panel_tick: Instant::now(),
            hot: None,
            pressed: None,
            thumbs: None,
            edits: HashMap::new(),
            writer: None,
            saving: HashSet::new(),
            reloading: HashMap::new(),
            undo: Vec::new(),
            notice: None,
            card_open: false,
            card_hot: None,
            strip_pos: 0.0,
            strip_tick: None,
            menu_open: false,
            dial_open: false,
            angle: 0.0,
            dial_drag: None,
            channel: 0,
            tooltip: None,
            playback: None,
            frames_rx: None,
            probe: None,
            info_state: inspector::InfoState::default(),
            hist_state: inspector::HistState::default(),
            fileops: None,
            after_save: None,
            open_rx: None,
            hidden: false,
            pending_show: false,
            quicklook: false,
            pinned: false,
            tray: false,
            quitting: false,
            hook_thread: None,
            taskbar_msg: system::taskbar_created_message(),
            vector: None,
            detail: None,
            drag: None,
            wheel_accum: 0,
            fast_nav: false,
            mode_switched_at: Instant::now(),
            settings,
        };
        let current = app.current.clone();
        app.store_result(
            &current,
            decoded.map(std::sync::Arc::new).map_err(|e| e.to_string()),
        );
        app.show_current()?;
        if app.settings.open_mode == crate::settings::OpenMode::Windowed {
            app.start_windowed()?;
        }
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

    /// Page-turn length, or `None` when that animation is turned off.
    fn turn_len(&self) -> Option<Duration> {
        self.settings
            .turn_animation
            .then(|| Duration::from_millis(TURN_ANIM))
    }

    /// Animation length, or `None` when animations are turned off.
    fn anim_len(&self, ms: u64) -> Option<Duration> {
        self.settings.animations.then(|| Duration::from_millis(ms))
    }

    fn filter(&self) -> Filter {
        if self.settings.smoothing {
            Filter::Smooth
        } else {
            Filter::Pixelated
        }
    }

    /// A preview of one photo (Space in Explorer, pinned): no folder.
    fn preview(&self) -> bool {
        self.quicklook || self.pinned
    }

    /// Filmstrip height (96-DPI pixels) if it is shown.
    fn strip_height(&self) -> Option<f64> {
        (self.settings.filmstrip && !self.preview())
            .then(|| self.settings.thumb_size.strip_height())
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

    /// Orientation an image is displayed with: EXIF plus the user's edits.
    fn orientation_of(&self, key: &str, img: &GpuImage) -> Orientation {
        self.edits
            .get(key)
            .map_or(img.orientation, |e| img.orientation.then(*e))
    }

    /// Full-resolution size in display orientation (including user rotation).
    fn display_of(&self, key: &str, img: &GpuImage) -> (f64, f64) {
        let (dw, dh) = img.display;
        let stored = if img.orientation.swaps_axes() {
            (dh, dw)
        } else {
            (dw, dh)
        };
        if self.orientation_of(key, img).swaps_axes() {
            (stored.1, stored.0)
        } else {
            stored
        }
    }

    /// Display size of the shown image.
    fn display(&self) -> Option<(f64, f64)> {
        self.shown.as_ref().map(|(k, t)| self.display_of(k, t))
    }

    /// Screen area photos are fitted into; fullscreen keeps the bottom free
    /// for the panel (and the filmstrip), like Picasa.
    fn view_area(&self) -> Rect {
        let (w, h) = self.viewport();
        match self.mode {
            Mode::Fullscreen => {
                let m = self.monitor.px(FIT_MARGIN_DIP);
                let bottom = (crate::ui::toolbar::reserved_height(self.strip_height())
                    * self.ui_scale())
                .round();
                Rect {
                    x0: m,
                    y0: m,
                    x1: w - m,
                    y1: h - bottom - (m / 2.0).round(),
                }
            }
            // Below the title bar, inside the frame.
            Mode::Windowed => mode::photo_area(
                Rect {
                    x0: 0.0,
                    y0: 0.0,
                    x1: w,
                    y1: h,
                },
                &self.monitor,
            ),
        }
    }

    /// "Fit": the whole photo (turned by the free angle) inside the view
    /// area, never enlarged past 100 %.
    fn fit_view(&self) -> Option<View> {
        let d = self.display()?;
        Some(View::fit_in(
            d,
            self.view_area(),
            self.viewport(),
            false,
            self.angle,
        ))
    }

    /// Keeps a view on screen (centred in the view area where it fits).
    fn clamp(&self, v: View) -> View {
        match self.display() {
            Some(d) => v.clamped_in(d, self.view_area(), self.viewport()),
            None => v,
        }
    }

    /// Image rectangle on screen (viewport coordinates), if an image is shown.
    fn image_rect(&self) -> Option<Rect> {
        Some(self.view.image_rect(self.display()?, self.viewport()))
    }

    /// One image layer: `view` placement at `opacity`. Huge images draw
    /// their overview when zoomed out, otherwise only the visible tiles.
    fn layer<'a>(
        &self,
        key: &str,
        img: &'a GpuImage,
        view: &View,
        opacity: f32,
        detail: Option<&'a GpuImage>,
    ) -> Option<Layer<'a>> {
        let vp = self.viewport();
        let orient = self.orientation_of(key, img);
        let disp = self.display_of(key, img);
        let rect = view.image_rect(disp, vp);
        let longest_on_screen = view.scale * disp.0.max(disp.1);
        // A turned view shows more than the viewport's own rectangle.
        let clip = view::view_clip(vp, view.angle);
        let mut parts = Vec::new();
        // SVG zoomed in: the sharp re-rendering of the visible region.
        let detail =
            detail.and_then(|t| Some((t.tiles.first()?, self.detail_part(key, img, view)?)));
        // Pixel grid: full-resolution pixels at 800 % and more (not for the
        // vector rendering of an SVG).
        let texel_scale =
            view.scale * disp.0 / f64::from(orient.apply_size(img.width, img.height).0);
        let grid = self.settings.pixel_grid
            && img.full
            && detail.is_none()
            && texel_scale >= GRID_MIN_SCALE - 1e-9;
        match &img.overview {
            _ if detail.is_some() => {
                parts.extend(detail.map(|(tile, (quad, edges))| Part {
                    srv: &tile.srv,
                    quad,
                    tex: tile.uv,
                    edges,
                }));
            }
            Some((t, ow, oh)) if longest_on_screen <= f64::from(*ow.max(oh)) => {
                parts.extend(view::visible_quad_in(rect, clip).map(|quad| Part {
                    srv: &t.srv,
                    quad,
                    tex: t.uv,
                    edges: 15,
                }));
            }
            _ => {
                let stored = (f64::from(img.width), f64::from(img.height));
                let (tdw, tdh) = orient.apply_size(img.width, img.height);
                let (tdw, tdh) = (f64::from(tdw), f64::from(tdh));
                // Screen pixels per texture pixel.
                let k = view.scale * disp.0 / tdw;
                for t in &img.tiles {
                    let r = orient.stored_rect_to_display(
                        [
                            f64::from(t.x0),
                            f64::from(t.y0),
                            f64::from(t.x0 + t.w),
                            f64::from(t.y0 + t.h),
                        ],
                        stored,
                    );
                    let screen = Rect {
                        x0: rect.x0 + r[0] * k,
                        y0: rect.y0 + r[1] * k,
                        x1: rect.x0 + r[2] * k,
                        y1: rect.y0 + r[3] * k,
                    };
                    let edges = u32::from(r[0] <= 0.0)
                        | u32::from(r[1] <= 0.0) << 1
                        | u32::from(r[2] >= tdw) << 2
                        | u32::from(r[3] >= tdh) << 3;
                    parts.extend(view::visible_quad_in(screen, clip).map(|quad| Part {
                        srv: &t.srv,
                        quad,
                        tex: t.uv,
                        edges,
                    }));
                }
            }
        }
        if parts.is_empty() {
            return None;
        }
        let (cos, sin) = view.rotation();
        Some(Layer {
            parts,
            origin: [rect.x0 as f32, rect.y0 as f32],
            // Skip the background pass entirely for opaque images.
            background: match (
                img.has_alpha && self.settings.checker,
                self.settings.alpha_bg.gray(),
            ) {
                (false, _) => Background::None,
                (true, None) => Background::Checker(self.monitor.px(CHECKER_DIP) as f32),
                (true, Some(g)) => {
                    let l = srgb_to_linear(g);
                    Background::Solid([l, l, l, 1.0])
                }
            },
            opacity,
            orientation: u32::from(orient.exif()),
            rotation: [cos as f32, sin as f32],
            pivot: [(vp.0 / 2.0) as f32, (vp.1 / 2.0) as f32],
            channel: self.channel,
            grid,
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
        self.refresh_inspector();
        let shown = self.shown.clone();
        let detail = self.detail_tex();
        let mut layers = Vec::with_capacity(2);
        match (&self.anim, &shown) {
            (
                Some(Anim {
                    kind: AnimKind::Turn { old, shift },
                    ..
                }),
                Some((key, new)),
            ) => {
                let t = self.anim_t();
                if let Some((old_key, old_img, old_view)) = old {
                    let v = old_view.pan(-shift * t, 0.0);
                    layers.extend(self.layer(old_key, old_img, &v, (1.0 - t) as f32, None));
                }
                let v = self.view.pan(shift * (1.0 - t), 0.0);
                layers.extend(self.layer(key, new, &v, t as f32, detail.as_deref()));
            }
            (_, Some((key, img))) => {
                layers.extend(self.layer(key, img, &self.view, self.opacity, detail.as_deref()));
            }
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
            filter: self.filter(),
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
            // The panel shows the zoom level itself; notices always show.
            badge: if self.panel_alpha < 0.5 || self.notice.is_some() {
                self.badge_text()
            } else {
                None
            },
            badge_lift: if self.panel_target > 0.5 {
                self.panel_height() as f32
            } else {
                0.0
            },
            panel: self.panel_view(),
            border: self.mode == Mode::Windowed,
            card: self.card_view(),
            caption: self.caption_view(),
            menu: self.menu_view(),
            dial: self.dial_view(),
            info: self.info_view(),
            histogram: self.histogram_view(),
            tooltip: self.tooltip_view(),
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
        let text = match &self.notice {
            Some((t, _)) => t.clone(),
            None => panel::zoom_label(self.zoom_target().scale),
        };
        Some((text, opacity))
    }

    /// Shows the zoom badge and (re)starts its hide timer.
    fn show_badge(&mut self) {
        self.badge = true;
        self.badge_fade = None;
        self.notice = None;
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
                        if let Some(d) = self.display() {
                            self.view = View::placed(d, from.lerp(to, t), vp);
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
        // Panel fade in / out.
        let now = Instant::now();
        let dt = now.duration_since(self.panel_tick).as_secs_f32();
        self.panel_tick = now;
        if (self.panel_alpha - self.panel_target).abs() > f32::EPSILON {
            let step = dt / PANEL_FADE.as_secs_f32();
            self.panel_alpha = if self.panel_target > self.panel_alpha {
                (self.panel_alpha + step).min(self.panel_target)
            } else {
                (self.panel_alpha - step).max(self.panel_target)
            };
            dirty = true;
            moving |= (self.panel_alpha - self.panel_target).abs() > f32::EPSILON;
        }
        // Filmstrip glide towards the current file.
        let target = self.index as f64;
        if self.strip_pos != target {
            let shown = self.panel_alpha > 0.0 && self.panel_layout().strip.is_some();
            let dt = self
                .strip_tick
                .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
            self.strip_tick = Some(now);
            // Never lag more than a screenful behind (Home / End).
            let max_lag = 12.0;
            self.strip_pos = self.strip_pos.clamp(target - max_lag, target + max_lag);
            self.strip_pos += (target - self.strip_pos) * (1.0 - (-dt / STRIP_TAU).exp());
            if !shown || !self.settings.animations || (self.strip_pos - target).abs() < 0.002 {
                self.strip_pos = target;
                self.strip_tick = None;
            } else {
                moving = true;
            }
            dirty |= shown;
        }
        if let Some(f) = self.badge_fade {
            dirty = true;
            if f.elapsed() >= BADGE_FADE {
                self.badge = false;
                self.badge_fade = None;
                self.notice = None;
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
        if self.pinned {
            return; // takes over a window already on screen
        }
        let (Some(duration), Some(img), Some(fit)) =
            (self.anim_len(OPEN_ANIM), self.display(), self.fit_view())
        else {
            return;
        };
        let vp = self.viewport();
        let to = fit.image_rect(img, vp);
        let (cx, cy) = to.center();
        // In a window the photo already fills the frame: a gentler zoom.
        let k = if self.mode == Mode::Windowed {
            0.94
        } else {
            0.82
        };
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
                dim: (self.dim, self.settings.dim()),
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
        if id == ANIM_TIMER
            && let Err(e) = self.frame_timer()
        {
            tracing::error!("animation: {e:#}");
        }
        if id == TOOLTIP_TIMER
            && let Err(e) = self.tooltip_timer()
        {
            tracing::error!("tooltip: {e:#}");
        }
        if id == MENU_TIMER
            && let Err(e) = self.menu_timer()
        {
            tracing::error!("menu: {e:#}");
        }
        if id == PANEL_TIMER {
            self.panel_timer();
        }
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
            WM_APP_THUMB => {
                self.on_thumbs()?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_SAVED => {
                self.on_saved()?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_VECTOR => {
                self.on_vector()?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_INFO => {
                self.on_info()?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_FILEOP => {
                self.on_fileop()?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_FRAMES => {
                self.on_frames()?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_OPEN => {
                self.on_open()?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_QUICKLOOK => {
                self.on_quick_look(HWND(wparam.0 as *mut _))?;
                Ok(Some(LRESULT(0)))
            }
            WM_APP_TRAY => {
                self.on_tray((lparam.0 & 0xffff) as u32)?;
                Ok(Some(LRESULT(0)))
            }
            m if m == self.taskbar_msg && m != 0 => {
                self.on_taskbar_created();
                Ok(Some(LRESULT(0)))
            }
            WM_CLOSE if self.hide_instead_of_close() => Ok(Some(LRESULT(0))),
            WM_CLOSE => {
                self.remove_tray();
                // Write a pending rotation and wait for all writes to finish.
                self.leave_current();
                if let Some(w) = &mut self.writer {
                    w.finish();
                }
                Ok(None)
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
        // Small enough for tiny images, large enough for the caption buttons.
        WM_GETMINMAXINFO => {
            // SAFETY: no preconditions; 0 (failure) falls back to 96 DPI.
            let dpi = match unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } {
                0 => 96,
                d => d,
            } as i32;
            // SAFETY: for WM_GETMINMAXINFO, lParam points to a MINMAXINFO.
            let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = 220 * dpi / 96;
            mmi.ptMinTrackSize.y = 140 * dpi / 96;
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
pub fn post(hwnd: isize, msg: u32) {
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

/// sRGB-encoded value → linear light.
fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
