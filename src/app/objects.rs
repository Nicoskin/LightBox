//! The objects card of a 3D project (O): every object with its colour; a
//! click hides or shows it, Ctrl + click shows it alone; "Контрастные
//! цвета" paints each object its own colour.

use anyhow::Result;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
};

use super::App;
use crate::cache;
use crate::gfx::{ObjectRowView, ObjectsView, SegmentView};
use crate::i18n::tr;
use crate::model::DEFAULT_COLOR;
use crate::t;
use crate::ui::objects::{self, Hit, Layout};

/// Rows per wheel notch.
const WHEEL_ROWS: f64 = 3.0;

#[derive(Default)]
pub(super) struct CardState {
    hot: Option<Hit>,
    scroll: f64,
    /// The button went down on the card (so its release acts).
    pressed: bool,
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
        ))
    }

    pub(super) fn objects_view(&self) -> Option<ObjectsView> {
        let l = self.objects_layout()?;
        let key = cache::key(&self.current);
        let m = self.model.as_ref().filter(|m| m.key == key)?;
        let hot = m.card.as_ref().and_then(|c| c.hot);
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
                    let color = if m.contrast {
                        objects::contrast_color(i)
                    } else {
                        m.mesh
                            .colors
                            .as_ref()
                            .and_then(|c| c.get(o.tris.start * 3).copied())
                            .unwrap_or(DEFAULT_COLOR)
                    };
                    ObjectRowView {
                        rect,
                        name: o.name.clone(),
                        color,
                        visible: !m.hidden[i],
                        hot: hot == Some(Hit::Row(i)),
                    }
                })
                .collect(),
        })
    }

    /// Mouse over the card; `true` when consumed.
    pub(super) fn objects_input(&mut self, msg: u32, wparam: usize, p: (f64, f64)) -> Result<bool> {
        let Some(l) = self.objects_layout() else {
            return Ok(false);
        };
        let hit = l.hit(p);
        let key = cache::key(&self.current);
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        let Some(card) = m.card.as_mut() else {
            return Ok(false);
        };
        match msg {
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
                self.render()?;
                Ok(true)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                card.pressed = hit.is_some();
                Ok(hit.is_some())
            }
            WM_LBUTTONUP => {
                let pressed = std::mem::take(&mut card.pressed);
                if !pressed {
                    return Ok(false);
                }
                // SAFETY: no preconditions.
                let ctrl = unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } < 0;
                match hit {
                    Some(Hit::Contrast) => m.contrast = !m.contrast,
                    Some(Hit::ShowAll) => m.hidden.fill(false),
                    // Ctrl: this object alone.
                    Some(Hit::Row(i)) if ctrl => {
                        m.hidden.fill(true);
                        m.hidden[i] = false;
                    }
                    Some(Hit::Row(i)) => m.hidden[i] = !m.hidden[i],
                    _ => return Ok(true),
                }
                m.rebuild(&self.gpu.device, false)?;
                self.render()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
