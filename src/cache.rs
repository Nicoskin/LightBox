//! In-memory LRU cache of decoded images, bounded by pixel memory.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::decode::DecodedImage;

/// Default budget: ~1.5 GB of decoded pixels.
pub const DEFAULT_BUDGET: usize = 1536 << 20;

pub enum Slot {
    Image(Arc<DecodedImage>),
    Failed(Arc<str>),
}

struct Item {
    slot: Slot,
    last_used: u64,
}

pub struct Cache {
    items: HashMap<String, Item>,
    used: usize,
    budget: usize,
    tick: u64,
}

/// Cache key: paths compare case-insensitively on Windows.
pub fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

impl Cache {
    pub fn new(budget: usize) -> Self {
        Self {
            items: HashMap::new(),
            used: 0,
            budget,
            tick: 0,
        }
    }

    /// Looks up an entry and marks it as recently used.
    pub fn get(&mut self, path: &Path) -> Option<&Slot> {
        self.tick += 1;
        let tick = self.tick;
        self.items.get_mut(&key(path)).map(|it| {
            it.last_used = tick;
            &it.slot
        })
    }

    /// Cached image, if any (without touching the LRU order).
    pub fn image(&self, path: &Path) -> Option<&Arc<DecodedImage>> {
        match self.items.get(&key(path)).map(|i| &i.slot) {
            Some(Slot::Image(img)) => Some(img),
            _ => None,
        }
    }

    /// `true` if nothing more needs decoding for `path` at this quality.
    pub fn satisfies(&self, path: &Path, full: bool) -> bool {
        match self.items.get(&key(path)).map(|i| &i.slot) {
            Some(Slot::Image(img)) => !full || img.is_full(),
            Some(Slot::Failed(_)) => true,
            None => false,
        }
    }

    /// Stores a decode result. A full-resolution image is never replaced by
    /// a preview. Evicts least-recently-used entries outside `keep`.
    pub fn insert(&mut self, path: &Path, slot: Slot, keep: &[&Path]) {
        let k = key(path);
        if let (
            Some(Item {
                slot: Slot::Image(old),
                ..
            }),
            Slot::Image(new),
        ) = (self.items.get(&k), &slot)
            && old.is_full()
            && !new.is_full()
        {
            return;
        }
        self.tick += 1;
        let size = slot_bytes(&slot);
        if let Some(old) = self.items.insert(
            k,
            Item {
                slot,
                last_used: self.tick,
            },
        ) {
            self.used -= slot_bytes(&old.slot);
        }
        self.used += size;
        self.evict(keep);
    }

    fn evict(&mut self, keep: &[&Path]) {
        if self.used <= self.budget {
            return;
        }
        let keep: Vec<String> = keep.iter().map(|p| key(p)).collect();
        let mut candidates: Vec<(u64, String)> = self
            .items
            .iter()
            .filter(|(k, _)| !keep.contains(k))
            .map(|(k, it)| (it.last_used, k.clone()))
            .collect();
        candidates.sort_unstable();
        for (_, k) in candidates {
            if self.used <= self.budget {
                break;
            }
            if let Some(it) = self.items.remove(&k) {
                self.used -= slot_bytes(&it.slot);
            }
        }
    }

    #[cfg(test)]
    fn used(&self) -> usize {
        self.used
    }
}

fn slot_bytes(s: &Slot) -> usize {
    match s {
        Slot::Image(img) => img.bytes(),
        Slot::Failed(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orientation::Orientation;

    fn img(w: u32, full_w: u32) -> Slot {
        Slot::Image(Arc::new(DecodedImage {
            width: w,
            height: 1,
            pixels: vec![0; w as usize * 4],
            source_width: full_w,
            source_height: 1,
            has_alpha: false,
            orientation: Orientation::default(),
        }))
    }

    #[test]
    fn lru_eviction_respects_keep() {
        let mut c = Cache::new(1000);
        let (a, b, d) = (Path::new("a"), Path::new("b"), Path::new("d"));
        c.insert(a, img(100, 100), &[]); // 400 bytes
        c.insert(b, img(100, 100), &[]); // 800
        c.get(a); // a is now more recent than b
        c.insert(d, img(100, 100), &[d]); // 1200 > 1000 → evict b
        assert!(c.image(a).is_some());
        assert!(c.image(b).is_none());
        assert!(c.image(d).is_some());
        assert_eq!(c.used(), 800);

        // Everything kept: may exceed the budget rather than drop the current image.
        c.insert(b, img(100, 100), &[a, b, d]);
        assert_eq!(c.used(), 1200);
    }

    #[test]
    fn full_is_not_downgraded_and_keys_ignore_case() {
        let mut c = Cache::new(1 << 20);
        c.insert(Path::new("X.JPG"), img(100, 100), &[]);
        c.insert(Path::new("x.jpg"), img(50, 100), &[]);
        assert!(c.image(Path::new("x.Jpg")).unwrap().is_full());
        assert!(c.satisfies(Path::new("x.jpg"), true));
        assert!(!c.satisfies(Path::new("y.jpg"), false));
        c.insert(Path::new("y.jpg"), img(50, 100), &[]);
        assert!(c.satisfies(Path::new("y.jpg"), false));
        assert!(!c.satisfies(Path::new("y.jpg"), true));
        c.insert(Path::new("z.jpg"), Slot::Failed("bad".into()), &[]);
        assert!(c.satisfies(Path::new("z.jpg"), true));
    }
}
