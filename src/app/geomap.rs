//! The map in the info panel for photos with GPS coordinates, rendered on
//! a worker thread from OpenStreetMap tiles (see `crate::map`).

use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use super::{App, WM_APP_MAP, post};
use crate::cache;
use crate::gfx::Thumb;

/// A map being rendered: file key and the result channel.
pub(super) type MapRx = (String, Receiver<Result<Thumb, String>>);

impl App {
    /// Starts rendering the map of the current photo if the info panel
    /// shows it and none is there or on its way.
    pub(super) fn request_map(&mut self, size: (u32, u32)) {
        let key = cache::key(&self.current);
        if !self.settings.map_tiles
            || self.map.as_ref().is_some_and(|(k, _)| *k == key)
            || self.map_rx.as_ref().is_some_and(|(k, _)| *k == key)
            || self.map_error.as_ref().is_some_and(|(k, _)| *k == key)
        {
            return;
        }
        let Some((lat, lon)) = self.gps() else {
            return;
        };
        let (tx, rx) = channel();
        let hwnd = self.hwnd.0 as isize;
        let id = 0x4D41_5000_0000_0000 ^ crc32fast::hash(key.as_bytes()) as u64;
        let spawned = thread::Builder::new().name("map".into()).spawn(move || {
            let r = crate::map::render(lat, lon, size.0, size.1)
                .map(|bgra| Thumb {
                    id,
                    width: size.0,
                    height: size.1,
                    bgra,
                })
                .map_err(|e| e.to_string());
            if tx.send(r).is_ok() {
                post(hwnd, WM_APP_MAP);
            }
        });
        if spawned.is_ok() {
            self.map_rx = Some((key, rx));
        }
    }

    pub(super) fn on_map(&mut self) -> anyhow::Result<()> {
        let Some((key, rx)) = &self.map_rx else {
            return Ok(());
        };
        let Ok(r) = rx.try_recv() else {
            return Ok(());
        };
        let key = key.clone();
        self.map_rx = None;
        match r {
            Ok(t) => self.map = Some((key, Rc::new(t))),
            Err(e) => {
                tracing::warn!("map: {e}");
                self.map_error = Some((key, e));
            }
        }
        self.render()
    }

    /// The map of the current photo and a status line for the panel.
    pub(super) fn map_view(&self) -> (Option<Rc<Thumb>>, String) {
        let key = cache::key(&self.current);
        if let Some((_, t)) = self.map.as_ref().filter(|(k, _)| *k == key) {
            return (Some(Rc::clone(t)), String::new());
        }
        let status = if !self.settings.map_tiles {
            "Загрузка карт выключена в настройках".to_string()
        } else if let Some((_, e)) = self.map_error.as_ref().filter(|(k, _)| *k == key) {
            format!("Карта не загрузилась: {e}")
        } else {
            "Загрузка карты…".to_string()
        };
        (None, status)
    }

    /// "Open on the map": the position in the browser.
    pub(super) fn open_map(&mut self) -> anyhow::Result<()> {
        if let Some((lat, lon)) = self.gps() {
            crate::win::shell_open(&crate::map::url(lat, lon));
        }
        Ok(())
    }
}
