//! Documents (PDF, text) shown page by page: the first page arrives through
//! the normal decoder, other pages are rendered by the vector worker and
//! kept on the GPU for going back and forth. ←/→, ↑/↓, PgUp/PgDn and the
//! wheel turn pages; Ctrl + wheel zooms.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;

use super::App;
use crate::cache;
use crate::decode::{self, DecodedImage};
use crate::gfx::GpuImage;
use crate::t;

/// Pages kept on the GPU besides the one shown.
const KEEP: usize = 6;

pub(super) struct DocState {
    pub key: String,
    pub page: u32,
    pub pages: u32,
    textures: HashMap<u32, Rc<GpuImage>>,
}

impl App {
    /// `true` while the current file is a document.
    pub(super) fn is_doc(&self) -> bool {
        decode::is_document(&self.current)
    }

    /// The state of the current document, if it is one and is open.
    pub(super) fn doc(&self) -> Option<&DocState> {
        let key = cache::key(&self.current);
        self.doc.as_ref().filter(|d| d.key == key)
    }

    /// Called by `show_current` with the texture of page 1: starts the
    /// document state, or returns the texture of the page being read.
    pub(super) fn doc_texture(
        &mut self,
        key: &str,
        pages: Option<u32>,
        first: Rc<GpuImage>,
    ) -> Rc<GpuImage> {
        if !self.is_doc() {
            self.doc = None;
            return first;
        }
        match &mut self.doc {
            Some(d) if d.key == key => {
                d.textures.insert(0, Rc::clone(&first));
                d.textures.get(&d.page).cloned().unwrap_or(first)
            }
            _ => {
                self.doc = Some(DocState {
                    key: key.to_string(),
                    page: 0,
                    pages: pages.unwrap_or(1).max(1),
                    textures: HashMap::from([(0, Rc::clone(&first))]),
                });
                first
            }
        }
    }

    /// Turns to page `page` (0-based, clamped). Returns `false` when the
    /// current file is not a document.
    pub(super) fn go_page(&mut self, page: i64) -> Result<bool> {
        let Some(d) = self
            .doc
            .as_mut()
            .filter(|d| d.key == cache::key(&self.current))
        else {
            return Ok(false);
        };
        let page = page.clamp(0, i64::from(d.pages) - 1) as u32;
        if page == d.page {
            return Ok(true);
        }
        d.page = page;
        let (pages, cached) = (d.pages, d.textures.get(&page).cloned());
        self.detail = None;
        self.notice(&t!("Страница {} из {}", page + 1, pages));
        match cached {
            Some(t) => self.show_page(t)?,
            None => self.request_page(page),
        }
        Ok(true)
    }

    /// Steps `delta` pages.
    pub(super) fn step_page(&mut self, delta: i64) -> Result<bool> {
        let Some(page) = self.doc().map(|d| i64::from(d.page)) else {
            return Ok(false);
        };
        self.go_page(page + delta)
    }

    /// A page rendered by the worker.
    pub(super) fn on_page(&mut self, key: &str, page: u32, image: DecodedImage) -> Result<()> {
        let tex = Rc::new(GpuImage::upload(&self.gpu, &image)?);
        let Some(d) = self.doc.as_mut().filter(|d| d.key == key) else {
            return Ok(());
        };
        // Keep the nearest pages.
        if d.textures.len() > KEEP {
            let current = d.page;
            if let Some(far) = d
                .textures
                .keys()
                .copied()
                .filter(|p| *p != 0)
                .max_by_key(|p| p.abs_diff(current))
            {
                d.textures.remove(&far);
            }
        }
        d.textures.insert(page, Rc::clone(&tex));
        if d.page == page {
            self.show_page(tex)?;
        }
        Ok(())
    }

    fn show_page(&mut self, tex: Rc<GpuImage>) -> Result<()> {
        let key = cache::key(&self.current);
        self.shown = Some((key, tex));
        if let Some(v) = self.fit_view() {
            self.view = v;
            self.fitted = true;
        }
        self.update_detail();
        self.render()
    }

    /// "стр. 3 из 12" for the panel and the title.
    pub(super) fn page_label(&self) -> Option<String> {
        self.doc()
            .filter(|d| d.pages > 1)
            .map(|d| t!("стр. {} из {}", d.page + 1, d.pages))
    }
}
