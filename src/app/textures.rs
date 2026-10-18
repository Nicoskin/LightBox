//! Small LRU of GPU textures for the current image and its neighbours, so
//! that stepping to an already-decoded photo needs no upload at all.

use std::rc::Rc;

use crate::gfx::GpuImage;

/// Textures kept on the GPU (current + prefetched neighbours).
const CAPACITY: usize = 8;

#[derive(Default)]
pub struct Textures {
    /// Least recently used first.
    items: Vec<(String, Rc<GpuImage>)>,
}

impl Textures {
    /// Best texture for `key` (full resolution preferred), marked as used.
    pub fn get(&mut self, key: &str) -> Option<Rc<GpuImage>> {
        let pos = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, (k, _))| k == key)
            .max_by_key(|(_, (_, t))| t.full)
            .map(|(i, _)| i)?;
        let item = self.items.remove(pos);
        let tex = Rc::clone(&item.1);
        self.items.push(item);
        Some(tex)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.items.iter().any(|(k, _)| k == key)
    }

    /// Adds a texture, replacing any other texture of the same image.
    pub fn insert(&mut self, key: String, tex: Rc<GpuImage>) {
        self.items.retain(|(k, _)| *k != key);
        self.items.push((key, tex));
        if self.items.len() > CAPACITY {
            self.items.remove(0);
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.items.retain(|(k, _)| k != key);
    }

    /// Drops everything (e.g. when switching to another GPU device).
    pub fn clear(&mut self) {
        self.items.clear();
    }
}
