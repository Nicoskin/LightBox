//! The objects card of a 3D project (O): every object with its colour; a
//! click hides or shows it, Ctrl + click shows it alone; the swatch opens a
//! palette, the slider makes it see-through; "Контрастные цвета" paints
//! each object its own colour.

use anyhow::Result;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
};

use super::App;
use crate::cache;
use crate::gfx::{ObjectRowView, ObjectsView, SegmentView};
use crate::i18n::tr;
use crate::model::DEFAULT_COLOR;
use crate::t;
use crate::ui::objects::{self, Hit, Layout, PALETTE};

/// Rows per wheel notch.
const WHEEL_ROWS: f64 = 3.0;
/// Models up to this many triangles follow the opacity slider while it is
/// dragged; bigger ones are rebuilt when it is let go.
const LIVE_TRIANGLES: usize = 400_000;

#[derive(Default)]
pub(super) struct CardState {
    hot: Option<Hit>,
    scroll: f64,
    /// What the button went down on (its release acts there).
    pressed: Option<Hit>,
    /// The object whose palette is open.
    palette: Option<usize>,
    /// The object whose opacity is being dragged.
    dragging: Option<usize>,
}

impl App {
    /// `true` when the current model has several objects.
    pub(super) fn has_objects(&self) -> bool {
        self.object_count() > 0
    }

    /// Objects of the current model, when it has several.
    fn object_count(&self) -> usize {
        let key = cache::key(&self.current);
        self.model
            .as_ref()
            .filter(|m| m.key == key)
            .map_or(0, |m| m.mesh.objects.len())
    }

    /// `true` while the objects card is shown.
    pub(super) fn objects_open(&self) -> bool {
        let key = cache::key(&self.current);
        self.model
            .as_ref()
            .is_some_and(|m| m.key == key && m.card.is_some())
    }

    /// O: shows / hides the objects card.
    pub(super) fn toggle_objects(&mut self) -> Result<()> {
        if self.object_count() == 0 {
            if self.model_mode() {
                self.notice(tr("В модели один объект"));
                return self.render();
            }
            return Ok(());
        }
        let key = cache::key(&self.current);
        if let Some(m) = self.model.as_mut().filter(|m| m.key == key) {
            m.card = match m.card {
                Some(_) => None,
                None => Some(CardState::default()),
            };
        }
        self.render()
    }

    fn objects_layout(&self) -> Option<Layout> {
        let key = cache::key(&self.current);
        let m = self.model.as_ref().filter(|m| m.key == key)?;
        let card = m.card.as_ref()?;
        let bottom = self.viewport().1 - self.panel_height();
        Some(objects::layout(
            self.ui_scale(),
            self.overlay_top(false),
            bottom,
            m.mesh.objects.len(),
            card.scroll,
            card.palette,
        ))
    }

    pub(super) fn objects_view(&self) -> Option<ObjectsView> {
        let l = self.objects_layout()?;
        let key = cache::key(&self.current);
        let m = self.model.as_ref().filter(|m| m.key == key)?;
        let hot = m.card.as_ref().and_then(|c| c.hot);
        let s = self.ui_scale();
        let shown = m.hidden.iter().filter(|h| !**h).count();
        let button = |rect, text: &str, selected, hit, first, last| SegmentView {
            rect,
            text: text.to_string(),
            selected,
            hot: hot == Some(hit),
            first,
            last,
        };
        Some(ObjectsView {
            card: l.card,
            title: (
                l.title,
                t!("Объекты: {} из {}", shown, m.mesh.objects.len()),
            ),
            buttons: vec![
                button(
                    l.contrast,
                    tr("Контрастные цвета"),
                    m.contrast,
                    Hit::Contrast,
                    true,
                    false,
                ),
                button(
                    l.show_all,
                    tr("Показать все"),
                    false,
                    Hit::ShowAll,
                    false,
                    true,
                ),
            ],
            list: l.list,
            rows: l
                .rows
                .iter()
                .map(|&(i, rect)| {
                    let o = &m.mesh.objects[i];
                    let color = m.paint[i].unwrap_or_else(|| {
                        if m.contrast {
                            objects::contrast_color(i)
                        } else {
                            m.mesh
                                .colors
                                .as_ref()
                                .and_then(|c| c.get(o.tris.start * 3).copied())
                                .unwrap_or(DEFAULT_COLOR)
                        }
                    });
                    let (slider, text) = objects::opacity_rects(rect, s);
                    let alpha = m.alpha[i];
                    ObjectRowView {
                        rect,
                        name: o.name.clone(),
                        color,
                        visible: !m.hidden[i],
                        hot: matches!(hot, Some(Hit::Row(k) | Hit::Swatch(k)) if k == i),
                        opacity: (
                            slider,
                            alpha,
                            text,
                            format!("{} %", (alpha * 100.0).round()),
                        ),
                    }
                })
                .collect(),
            palette: l.palette.map(|(card, cells, reset)| {
                (
                    card,
                    cells
                        .into_iter()
                        .enumerate()
                        .map(|(k, r)| (r, PALETTE[k], hot == Some(Hit::Paint(k))))
                        .collect(),
                    SegmentView {
                        rect: reset,
                        text: tr("Как в файле").into(),
                        selected: false,
                        hot: hot == Some(Hit::PaintReset),
                        first: true,
                        last: true,
                    },
                )
            }),
        })
    }

    /// Mouse over the card; `true` when consumed.
    pub(super) fn objects_input(&mut self, msg: u32, wparam: usize, p: (f64, f64)) -> Result<bool> {
        let Some(l) = self.objects_layout() else {
            return Ok(false);
        };
        let hit = l.hit(p);
        let key = cache::key(&self.current);
        let hwnd = self.hwnd;
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        let live = m.mesh.triangles() <= LIVE_TRIANGLES;
        let Some(card) = m.card.as_mut() else {
            return Ok(false);
        };
        let slider_of = |i: usize| {
            l.rows
                .iter()
                .find(|(k, _)| *k == i)
                .map(|(_, r)| objects::opacity_rects(*r, r.height() / 30.0).0)
        };
        match msg {
            WM_MOUSEMOVE if card.dragging.is_some() => {
                let i = card.dragging.unwrap_or_default();
                if let Some(slider) = slider_of(i) {
                    m.alpha[i] = objects::opacity_at(slider, p.0);
                    if live {
                        m.rebuild(&self.gpu.device, false)?;
                    }
                    self.render()?;
                }
                Ok(true)
            }
            WM_MOUSEMOVE => {
                if card.hot != hit {
                    card.hot = hit;
                    self.render()?;
                }
                Ok(false)
            }
            WM_MOUSEWHEEL if hit.is_some() => {
                let delta = f64::from((wparam >> 16) as u16 as i16) / 120.0;
                let row = l.rows.first().map_or(30.0, |(_, r)| r.height());
                card.scroll = (card.scroll - delta * WHEEL_ROWS * row).clamp(0.0, l.max_scroll);
                card.hot = None;
                card.palette = None;
                self.render()?;
                Ok(true)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                card.pressed = hit;
                if let Some(Hit::Opacity(i)) = hit {
                    card.dragging = Some(i);
                    card.palette = None;
                    if let Some(slider) = slider_of(i) {
                        m.alpha[i] = objects::opacity_at(slider, p.0);
                    }
                    // SAFETY: own window.
                    unsafe { SetCapture(hwnd) };
                    if live {
                        m.rebuild(&self.gpu.device, false)?;
                    }
                    self.render()?;
                }
                Ok(hit.is_some())
            }
            WM_LBUTTONUP => {
                if card.dragging.take().is_some() {
                    card.pressed = None;
                    // SAFETY: releasing our own capture.
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    m.rebuild(&self.gpu.device, false)?;
                    self.render()?;
                    return Ok(true);
                }
                let Some(pressed) = card.pressed.take() else {
                    // A click elsewhere closes the palette.
                    if card.palette.take().is_some() {
                        self.render()?;
                    }
                    return Ok(false);
                };
                if hit != Some(pressed) {
                    return Ok(true);
                }
                // SAFETY: no preconditions.
                let ctrl = unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } < 0;
                let palette = card.palette.take();
                match pressed {
                    Hit::Contrast => m.contrast = !m.contrast,
                    Hit::ShowAll => m.hidden.fill(false),
                    Hit::Swatch(i) => {
                        // The same swatch again closes its palette.
                        card.palette = (palette != Some(i)).then_some(i);
                        self.render()?;
                        return Ok(true);
                    }
                    Hit::Paint(k) => {
                        if let Some(i) = palette {
                            m.paint[i] = Some(PALETTE[k]);
                        }
                    }
                    Hit::PaintReset => {
                        if let Some(i) = palette {
                            m.paint[i] = None;
                        }
                    }
                    // Ctrl: this object alone.
                    Hit::Row(i) if ctrl => {
                        m.hidden.fill(true);
                        m.hidden[i] = false;
                    }
                    Hit::Row(i) => m.hidden[i] = !m.hidden[i],
                    Hit::Opacity(_) | Hit::Card => {
                        self.render()?;
                        return Ok(true);
                    }
                }
                m.rebuild(&self.gpu.device, false)?;
                self.render()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
