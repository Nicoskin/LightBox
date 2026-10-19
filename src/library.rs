//! The list of images in the current folder: scanning and sorting.
//!
//! Scanning runs on its own thread so the first photo is on screen long
//! before a large folder has been listed.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::decode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortMode {
    /// Natural, case-insensitive file-name order (`img2` before `img10`).
    #[default]
    Name,
    /// Last-modified time, oldest first (ties broken by name).
    #[expect(dead_code, reason = "selectable in settings (M6)")]
    Modified,
}

#[derive(Debug, Clone)]
struct Entry {
    pub path: PathBuf,
    modified: Option<SystemTime>,
}

/// Lists the supported images in `dir`, sorted. `current` is always included
/// (it may have an unusual extension, yet the user opened it explicitly).
pub fn scan(dir: &Path, current: &Path, mode: SortMode) -> Vec<PathBuf> {
    let mut entries: Vec<Entry> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| decode::is_supported(p))
            .map(|path| Entry {
                // On Windows this comes from the directory listing itself.
                modified: std::fs::metadata(&path).and_then(|m| m.modified()).ok(),
                path,
            })
            .collect(),
        Err(e) => {
            tracing::warn!("cannot list {}: {e}", dir.display());
            Vec::new()
        }
    };
    // (Unless it is gone: deleted or renamed meanwhile.)
    if current.is_file() && !entries.iter().any(|e| same_path(&e.path, current)) {
        entries.push(Entry {
            path: current.to_path_buf(),
            modified: None,
        });
    }
    sort(&mut entries, mode);
    entries.into_iter().map(|e| e.path).collect()
}

fn sort(entries: &mut [Entry], mode: SortMode) {
    // Sort keys (lower-cased names) are computed once, not per comparison.
    let mut keyed: Vec<(String, usize)> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (file_key(&e.path), i))
        .collect();
    keyed.sort_by(|a, b| {
        let by_name = || natural_cmp(&a.0, &b.0);
        match mode {
            SortMode::Name => by_name(),
            SortMode::Modified => entries[a.1]
                .modified
                .cmp(&entries[b.1].modified)
                .then_with(by_name),
        }
    });
    let order: Vec<Entry> = keyed.iter().map(|(_, i)| entries[*i].clone()).collect();
    entries.clone_from_slice(&order);
}

fn file_key(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Case-insensitive path equality (Windows file systems are case-insensitive).
pub fn same_path(a: &Path, b: &Path) -> bool {
    a == b || a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// Natural order: runs of digits compare by numeric value, everything else
/// character by character. Expects already lower-cased input.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (na, ra) = split_digits(a);
                let (nb, rb) = split_digits(b);
                // Compare numerically without overflow: strip leading zeros,
                // then longer is bigger, then lexicographic.
                let ta = trim_zeros(na);
                let tb = trim_zeros(nb);
                let ord = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    // "01" after "1": fewer leading zeros first.
                    .then_with(|| na.len().cmp(&nb.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
                (a, b) = (ra, rb);
            }
            (Some(_), Some(_)) => {
                // Compare one UTF-8 character at a time.
                let ca = char_at(a);
                let cb = char_at(b);
                if ca.0 != cb.0 {
                    return ca.0.cmp(&cb.0);
                }
                (a, b) = (&a[ca.1..], &b[cb.1..]);
            }
        }
    }
}

fn split_digits(s: &[u8]) -> (&[u8], &[u8]) {
    let n = s.iter().take_while(|c| c.is_ascii_digit()).count();
    s.split_at(n)
}

fn trim_zeros(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|&&c| c == b'0').count();
    &s[n..]
}

/// First character of a UTF-8 byte slice and its length.
fn char_at(s: &[u8]) -> (char, usize) {
    let len = match s[0] {
        b if b < 0x80 => 1,
        b if b >= 0xF0 => 4,
        b if b >= 0xE0 => 3,
        _ => 2,
    }
    .min(s.len());
    let c = std::str::from_utf8(&s[..len])
        .ok()
        .and_then(|t| t.chars().next())
        .unwrap_or(char::REPLACEMENT_CHARACTER);
    (c, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_lowercase()).collect();
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(
            sorted(&["img10.jpg", "img2.jpg", "img1.jpg", "img20.jpg"]),
            ["img1.jpg", "img2.jpg", "img10.jpg", "img20.jpg"]
        );
    }

    #[test]
    fn mixed_and_unicode() {
        assert_eq!(
            sorted(&["b.png", "A.png", "а.png", "c1d10.png", "c1d9.png"]),
            ["a.png", "b.png", "c1d9.png", "c1d10.png", "а.png"]
        );
    }

    #[test]
    fn leading_zeros_and_huge_numbers() {
        assert_eq!(sorted(&["x01", "x1", "x001"]), ["x1", "x01", "x001"]);
        assert_eq!(
            sorted(&["n99999999999999999999999", "n100000000000000000000000"]),
            ["n99999999999999999999999", "n100000000000000000000000"]
        );
    }

    #[test]
    fn prefix_is_less() {
        assert_eq!(natural_cmp("img", "img1"), Ordering::Less);
        assert_eq!(natural_cmp("img1", "img1"), Ordering::Equal);
    }

    #[test]
    fn scan_includes_current_and_filters() {
        let dir = std::env::temp_dir().join(format!("lightbox-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["p10.jpg", "p2.JPG", "notes.txt", "p1.png", "odd.webp"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let list = scan(&dir, &dir.join("odd.webp"), SortMode::Name);
        let names: Vec<_> = list
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["odd.webp", "p1.png", "p2.JPG", "p10.jpg"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
