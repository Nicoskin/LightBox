//! Pixel inspector: colour under the cursor, channel views, pixel grid, histogram (H) and the info panel (I).

use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::thread;

use anyhow::Result;

use super::panel::Hot;
use super::{App, Mode, WM_APP_INFO, post};
use crate::cache;
use crate::decode::DecodedImage;
use crate::gfx::{HistogramView, InfoView};
use crate::i18n::tr;
use crate::inspect::{self, FileInfo, Histogram};
use crate::t;
use crate::ui::caption;
use crate::ui::info::{self, AiShape, InfoButton, InfoLayout};
use crate::win::clipboard;

/// Longest text drawn in a prompt box (the copy buttons give all of it).
const MAX_BLOCK_CHARS: usize = 1500;

/// The pixel under the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Probe {
    /// Full-resolution display coordinates.
    pub x: u32,
    pub y: u32,
    /// Straight sRGB RGBA.
    pub rgba: [u8; 4],
}

impl Probe {
    pub fn hex(&self) -> String {
        let [r, g, b, a] = self.rgba;
        if a == 255 {
            format!("#{r:02X}{g:02X}{b:02X}")
        } else {
            format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
        }
    }

    /// "X 1234  Y 567   RGBA 255 128 0 255   #FF8000".
    pub fn text(&self) -> String {
        let [r, g, b, a] = self.rgba;
        format!(
            "X {}  Y {}     RGBA {r} {g} {b} {a}     {}",
            self.x,
            self.y,
            self.hex()
        )
    }
}

/// Info panel state: the file it describes and what was read from it.
#[derive(Default)]
pub(super) struct InfoState {
    pub open: bool,
    /// (cache key, metadata) of the last file read.
    pub data: Option<(String, Rc<FileInfo>)>,
    /// Key being read on the background thread.
    pub pending: Option<String>,
    pub rx: Option<std::sync::mpsc::Receiver<(String, FileInfo)>>,
}

/// Histogram state: shown, and for which decoded image it was computed.
#[derive(Default)]
pub(super) struct HistState {
    pub open: bool,
    pub data: Option<(*const DecodedImage, Rc<Histogram>)>,
}

impl App {
    /// Shift+1..4: one channel; Shift+0: all.
    pub(super) fn set_channel(&mut self, c: u32) -> Result<()> {
        self.channel = if self.channel == c { 0 } else { c.min(4) };
        self.notice(match self.channel {
            1 => tr("Канал: красный"),
            2 => tr("Канал: зелёный"),
            3 => tr("Канал: синий"),
            4 => tr("Канал: альфа"),
            _ => tr("Все каналы"),
        });
        self.render()
    }

    pub(super) fn toggle_grid(&mut self) -> Result<()> {
        self.settings.pixel_grid = !self.settings.pixel_grid;
        self.settings.save();
        self.notice(if self.settings.pixel_grid {
            tr("Сетка пикселей: вкл (от 800 %)")
        } else {
            tr("Сетка пикселей: выкл")
        });
        self.render()
    }

    /// The pixel under screen point `p`, if the image is under it.
    fn probe_at(&self, p: (f64, f64)) -> Option<Probe> {
        let (key, tex) = self.shown.as_ref()?;
        if *key != cache::key(&self.current) {
            return None;
        }
        let img = self.cache.image(&self.current)?;
        let disp = self.display_of(key, tex);
        let (ix, iy) = self.view.image_point(p, self.image_vp());
        if ix < 0.0 || iy < 0.0 || ix >= disp.0 || iy >= disp.1 {
            return None;
        }
        let o = self.orientation_of(key, tex);
        let (u, v) = o.display_to_stored((ix / disp.0, iy / disp.1));
        let sx = ((u * f64::from(img.width)) as u32).min(img.width - 1);
        let sy = ((v * f64::from(img.height)) as u32).min(img.height - 1);
        Some(Probe {
            x: ix as u32,
            y: iy as u32,
            rgba: img.pixel(sx, sy)?,
        })
    }

    /// Mouse move: updates the colour readout (shown from 100 % zoom, or
    /// always with the info panel open).
    pub(super) fn update_probe(&mut self, p: (f64, f64)) -> Result<()> {
        let wanted = self.view.scale >= 1.0 - 1e-9 || self.info_state.open;
        let probe = if wanted && !self.over_panel(p) && !self.over_a(p) && self.drag.is_none() {
            self.probe_at(self.to_image_space(p))
        } else {
            None
        };
        if probe != self.probe {
            self.probe = probe;
            self.render()?;
        }
        Ok(())
    }

    /// Top of the inspector overlays: below the title bar or the corner
    /// close button.
    pub(super) fn overlay_top(&self, below_close: bool) -> f64 {
        let s = self.ui_scale();
        match self.mode {
            Mode::Windowed => (caption::TITLE_H + 8.0) * s,
            Mode::Fullscreen if below_close => 60.0 * s,
            Mode::Fullscreen => 12.0 * s,
        }
    }

    pub(super) fn toggle_histogram(&mut self) -> Result<()> {
        self.hist_state.open = !self.hist_state.open;
        self.render()
    }

    pub(super) fn toggle_info(&mut self) -> Result<()> {
        self.info_state.open = !self.info_state.open;
        self.want_map();
        self.render()
    }

    /// Keeps the histogram and the file info in step with the current
    /// image (called before drawing).
    pub(super) fn refresh_inspector(&mut self) {
        let img = self
            .cache
            .image(&self.current)
            .filter(|i| !i.thumbnail)
            .map(Arc::clone);
        if self.hist_state.open {
            match &img {
                Some(i) => {
                    let ptr = Arc::as_ptr(i);
                    if self.hist_state.data.as_ref().is_none_or(|(p, _)| *p != ptr) {
                        let h = inspect::histogram(i);
                        self.hist_state.data = Some((ptr, Rc::new(h)));
                    }
                }
                None => self.hist_state.data = None,
            }
        }
        let key = cache::key(&self.current);
        let st = &self.info_state;
        if st.open
            && st.data.as_ref().is_none_or(|(k, _)| *k != key)
            && st.pending.as_deref() != Some(key.as_str())
        {
            self.request_info(key);
        }
    }

    /// Reads the current file's metadata on a background thread.
    fn request_info(&mut self, key: String) {
        let (tx, rx) = channel();
        let path = self.current.clone();
        let hwnd = self.hwnd.0 as isize;
        let k = key.clone();
        let spawned = thread::Builder::new().name("info".into()).spawn(move || {
            let info = inspect::gather(&path);
            if tx.send((k, info)).is_ok() {
                post(hwnd, WM_APP_INFO);
            }
        });
        if let Err(e) = spawned {
            tracing::error!("cannot start the info thread: {e}");
            return;
        }
        self.info_state.pending = Some(key);
        self.info_state.rx = Some(rx);
    }

    pub(super) fn on_info(&mut self) -> Result<()> {
        let Some((key, info)) = self
            .info_state
            .rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return Ok(());
        };
        self.info_state.rx = None;
        self.info_state.pending = None;
        self.info_state.data = Some((key, Rc::new(info)));
        self.want_map();
        self.render()
    }

    pub(super) fn histogram_view(&self) -> Option<HistogramView> {
        if !self.hist_state.open {
            return None;
        }
        let (_, hist) = self.hist_state.data.as_ref()?;
        Some(HistogramView {
            rect: info::histogram_rect(self.ui_scale(), self.overlay_top(false)),
            hist: Rc::clone(hist),
        })
    }

    /// Metadata of the current file, once read.
    fn file_info(&self) -> Option<&FileInfo> {
        let (k, info) = self.info_state.data.as_ref()?;
        (*k == cache::key(&self.current)).then_some(&**info)
    }

    /// Label / value rows: the image first, then the file and EXIF.
    fn info_rows(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        let name = self
            .current
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        rows.push((tr("Имя").to_string(), name));
        if let Some(img) = self.cache.image(&self.current) {
            let (w, h) = img.display_size();
            let mp = f64::from(w) * f64::from(h) / 1e6;
            rows.push((
                tr("Размер").into(),
                t!("{} × {}   ({} Мп)", w, h, crate::i18n::dec(mp, 1)),
            ));
            let i = &img.info;
            let depth = match (i.bits, i.float) {
                (_, true) => tr("32 бит float").to_string(),
                (b, false) => t!("{} бит на канал", b),
            };
            rows.push((tr("Формат").into(), format!("{}   ·   {depth}", i.format)));
            rows.push((
                tr("Прозрачность").into(),
                if img.has_alpha {
                    tr("есть")
                } else {
                    tr("нет")
                }
                .into(),
            ));
            rows.push((
                tr("Профиль").into(),
                i.icc
                    .clone()
                    .unwrap_or_else(|| tr("sRGB (нет профиля)").into()),
            ));
        }
        // A model has no image rows: its triangles and size instead.
        if self.model_mode() {
            rows.extend(self.model_rows());
        }
        // The file row goes after the name and size (a model: after the name).
        let at = if self.model_mode() {
            1
        } else {
            rows.len().min(2)
        };
        match self.file_info() {
            Some(f) => {
                let mut file = inspect::human_size(f.size);
                if let Some(m) = &f.modified {
                    file = format!("{file}   ·   {m}");
                }
                rows.insert(at, (tr("Файл").into(), file));
                rows.extend(f.exif.iter().map(|(l, v)| ((*l).to_string(), v.clone())));
            }
            None => rows.insert(at, (tr("Файл").into(), "…".into())),
        }
        rows
    }

    fn info_layout(&self) -> Option<(InfoLayout, Vec<(String, String)>)> {
        if !self.info_state.open {
            return None;
        }
        let rows = self.info_rows();
        let ai = self
            .file_info()
            .and_then(|f| f.ai.as_ref())
            .map(|a| AiShape {
                negative: !a.negative.is_empty(),
                params: a.params.len(),
                workflow: a.workflow.is_some(),
            });
        let l = info::layout(
            self.viewport(),
            self.ui_scale(),
            self.overlay_top(true),
            rows.len(),
            self.gps().is_some(),
            ai,
        );
        Some((l, rows))
    }

    pub(super) fn info_view(&self) -> Option<InfoView> {
        let (l, rows) = self.info_layout()?;
        let clip = |t: &str| {
            if t.chars().count() > MAX_BLOCK_CHARS {
                let mut s: String = t.chars().take(MAX_BLOCK_CHARS).collect();
                s.push('…');
                s
            } else {
                t.to_string()
            }
        };
        let mut v = InfoView {
            card: l.card,
            title: (l.title, tr("Информация").into()),
            rows: l
                .rows
                .iter()
                .zip(rows)
                .map(|((lr, vr), (label, value))| (*lr, label, *vr, value))
                .collect(),
            headings: Vec::new(),
            blocks: Vec::new(),
            buttons: Vec::new(),
            map: None,
        };
        if let Some(ai) = self.file_info().and_then(|f| f.ai.as_ref()) {
            if let Some(h) = l.ai_heading {
                v.headings.push((h, t!("Генерация · {}", ai.tool)));
            }
            if let Some((label, text)) = l.prompt {
                v.rows
                    .push((label, tr("Промпт").into(), label, String::new()));
                v.blocks.push((text, clip(&ai.prompt)));
            }
            if let Some((label, text)) = l.negative {
                v.rows
                    .push((label, tr("Негативный промпт").into(), label, String::new()));
                v.blocks.push((text, clip(&ai.negative)));
            }
            for ((lr, vr), (k, val)) in l.params.iter().zip(&ai.params) {
                v.rows.push((*lr, k.clone(), *vr, val.clone()));
            }
        }
        for (b, r) in &l.buttons {
            v.buttons
                .push((*r, b.label().into(), self.hot == Some(Hot::Info(*b))));
        }
        if let Some(r) = l.map {
            let (img, status) = self.map_view();
            v.map = Some((r, img, status));
        }
        Some(v)
    }

    /// The GPS position of the current photo, once its metadata is read.
    pub(super) fn gps(&self) -> Option<(f64, f64)> {
        self.file_info().and_then(|f| f.gps)
    }

    /// Size of the map in the info panel, pixels (to render it for).
    fn map_size(&self) -> Option<(u32, u32)> {
        let (l, _) = self.info_layout()?;
        let r = l.map?;
        Some((r.width().round() as u32, r.height().round() as u32))
    }

    /// The panel shows a photo with coordinates: get its map.
    pub(super) fn want_map(&mut self) {
        if let Some(size) = self.map_size() {
            self.request_map(size);
        }
    }

    /// Info panel under `p`: `Some(button)` on a button, `Some(None)` on
    /// the card.
    pub(super) fn info_hit(&self, p: (f64, f64)) -> Option<Option<InfoButton>> {
        let (l, _) = self.info_layout()?;
        l.card.contains(p).then(|| l.hit(p))
    }

    pub(super) fn info_action(&mut self, b: InfoButton) -> Result<()> {
        if b == InfoButton::Map {
            return self.open_map();
        }
        let Some(ai) = self.file_info().and_then(|f| f.ai.clone()) else {
            return Ok(());
        };
        let (text, what) = match b {
            InfoButton::Prompt => (ai.prompt, tr("Промпт скопирован")),
            InfoButton::Workflow => (ai.workflow.unwrap_or_default(), tr("Workflow скопирован")),
            InfoButton::All => (ai.raw, tr("Параметры скопированы")),
            InfoButton::Map => return Ok(()),
        };
        match clipboard::set_text(self.hwnd, &text) {
            Ok(()) => self.notice(what),
            Err(e) => {
                tracing::warn!("clipboard: {e:#}");
                self.notice(tr("Не удалось скопировать"));
            }
        }
        self.render()
    }
}
