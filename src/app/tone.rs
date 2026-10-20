//! Adjustments (A): exposure, contrast, saturation and warmth previewed in
//! the shader while the sliders move; "Apply" writes them, together with
//! a straightening angle set on the dial, into the file (re-encoded in its
//! format; Ctrl+Z restores it).

use anyhow::Result;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

use super::App;
use crate::adjust::Adjust;
use crate::cache;
use crate::edit::Job;
use crate::gfx::{AdjustView, SegmentView, SliderView};
use crate::i18n::tr;
use crate::orientation::Orientation;
use crate::t;
use crate::ui::adjust::{self, Hit};

pub(super) struct ToneState {
    key: String,
    values: Adjust,
    /// The slider being dragged.
    drag: Option<usize>,
    hot: Option<Hit>,
}

impl App {
    /// A: opens the adjustments for the current photo (or closes them).
    pub(super) fn toggle_tone(&mut self) -> Result<()> {
        if self.tone.take().is_some() {
            return self.render();
        }
        if self.shown.is_none() || self.model_mode() || self.is_doc() || self.preview() {
            return Ok(());
        }
        self.crop = None;
        self.notice = None;
        self.tone = Some(ToneState {
            key: cache::key(&self.current),
            values: Adjust::default(),
            drag: None,
            hot: None,
        });
        self.show_panel();
        self.render()
    }

    fn tone_layout(&self) -> adjust::Layout {
        let bottom = self.viewport().1 - self.panel_height();
        adjust::layout(self.viewport(), self.ui_scale(), bottom)
    }

    /// The adjustments previewed on `key`'s layer.
    pub(super) fn adjust_preview(&self, key: &str) -> [f32; 4] {
        self.tone
            .as_ref()
            .filter(|t| t.key == key)
            .map_or([0.0; 4], |t| t.values.to_array())
    }

    /// Mouse and keys over the card; `true` when consumed.
    pub(super) fn tone_input(&mut self, msg: u32, wparam: usize, p: (f64, f64)) -> Result<bool> {
        if self
            .tone
            .as_ref()
            .is_some_and(|t| t.key != cache::key(&self.current))
        {
            // Another photo came up: the adjustments belonged to the old one.
            self.tone = None;
            return Ok(false);
        }
        let l = self.tone_layout();
        let hit = l.hit(p);
        match msg {
            WM_KEYDOWN if wparam == usize::from(VK_RETURN.0) => {
                self.apply_tone()?;
                Ok(true)
            }
            WM_KEYDOWN if wparam == usize::from(VK_ESCAPE.0) => {
                self.tone = None;
                self.render()?;
                Ok(true)
            }
            WM_LBUTTONDBLCLK => {
                if let (Some(Hit::Slider(i)), Some(t)) = (hit, self.tone.as_mut()) {
                    adjust::set(&mut t.values, i, 0.0);
                    self.render()?;
                }
                Ok(hit.is_some())
            }
            WM_LBUTTONDOWN => {
                if let (Some(Hit::Slider(i)), Some(t)) = (hit, self.tone.as_mut()) {
                    t.drag = Some(i);
                    adjust::set(&mut t.values, i, l.value_at(i, p.0));
                    // SAFETY: own window.
                    unsafe { SetCapture(self.hwnd) };
                    self.render()?;
                }
                Ok(hit.is_some())
            }
            WM_MOUSEMOVE => {
                let Some(t) = self.tone.as_mut() else {
                    return Ok(false);
                };
                let mut redraw = t.hot != hit;
                t.hot = hit;
                if let Some(i) = t.drag {
                    adjust::set(&mut t.values, i, l.value_at(i, p.0));
                    redraw = true;
                }
                let dragging = t.drag.is_some();
                if redraw {
                    self.render()?;
                }
                Ok(dragging)
            }
            WM_LBUTTONUP => {
                if self.tone.as_mut().and_then(|t| t.drag.take()).is_some() {
                    // SAFETY: releasing our own capture.
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    return Ok(true);
                }
                match hit {
                    Some(Hit::Apply) => self.apply_tone()?,
                    Some(Hit::Cancel) => {
                        self.tone = None;
                        self.render()?;
                    }
                    Some(Hit::Reset) => {
                        if let Some(t) = self.tone.as_mut() {
                            t.values = Adjust::default();
                        }
                        self.render()?;
                    }
                    _ => {}
                }
                Ok(hit.is_some())
            }
            _ => Ok(false),
        }
    }

    /// Writes the adjustments and the dial's angle into the file.
    fn apply_tone(&mut self) -> Result<()> {
        let Some(t) = self.tone.take() else {
            return Ok(());
        };
        let path = self.current.clone();
        let key = cache::key(&path);
        if t.key != key || self.saving.contains(&key) {
            return self.render();
        }
        if t.values.is_identity() && self.angle == 0.0 {
            return self.render();
        }
        let Some(writer) = &self.writer else {
            return self.render();
        };
        let edit = self
            .edits
            .get(&key)
            .copied()
            .unwrap_or(Orientation::IDENTITY);
        writer.submit(Job::Adjust {
            path: path.clone(),
            display: self.base_orientation(&path).then(edit),
            edit,
            adjust: t.values,
            angle: self.angle,
            quality: self.settings.jpeg_quality,
            keep_dates: self.settings.keep_file_dates,
        });
        self.saving.insert(key);
        // The tilt is now in the pixels.
        self.angle = 0.0;
        self.dial_open = false;
        self.notice(tr("Коррекция сохраняется…"));
        self.render()
    }

    pub(super) fn tone_view(&self) -> Option<AdjustView> {
        let t = self.tone.as_ref()?;
        if t.key != cache::key(&self.current) {
            return None;
        }
        let l = self.tone_layout();
        let sliders = (0..4)
            .map(|i| {
                let v = adjust::get(&t.values, i);
                let (label, track, value) = l.sliders[i];
                SliderView {
                    label: (label, tr(adjust::SLIDERS[i].0).to_string()),
                    track,
                    knob: l.knob_x(i, v),
                    zero: l.knob_x(i, 0.0),
                    value: (value, adjust::value_text(i, v)),
                    hot: t.hot == Some(Hit::Slider(i)) || t.drag == Some(i),
                }
            })
            .collect();
        let note = if self.angle != 0.0 {
            let sign = if self.angle > 0.0 { "+" } else { "−" };
            let angle = format!("{sign}{}", crate::i18n::dec(self.angle.abs(), 1));
            t!("Наклон {}° будет выпрямлен, края обрезаны", angle)
        } else {
            tr("Сохраняется с качеством JPEG из настроек").to_string()
        };
        let button = |r, text: &str, hit: Hit, main: bool| SegmentView {
            rect: r,
            text: text.to_string(),
            selected: main,
            hot: t.hot == Some(hit),
            first: true,
            last: true,
        };
        Some(AdjustView {
            card: l.card,
            sliders,
            note: (l.note, note),
            buttons: vec![
                button(l.reset, tr("Сбросить"), Hit::Reset, false),
                button(l.apply, tr("Применить"), Hit::Apply, true),
                button(l.cancel, tr("Отмена"), Hit::Cancel, false),
            ],
        })
    }
}

impl App {
    /// Notices go above an open crop bar or adjustments card: their
    /// distance from the bottom edge.
    pub(super) fn card_lift(&self) -> Option<f32> {
        let top = if self.tone.is_some() {
            self.tone_layout().card.y0
        } else if self.crop.is_some() {
            let b = crate::ui::crop::bar(
                self.viewport(),
                self.ui_scale(),
                self.viewport().1 - self.panel_height(),
            );
            b.apply.y0
        } else {
            return None;
        };
        Some((self.viewport().1 - top + 6.0 * self.ui_scale()) as f32)
    }
}
