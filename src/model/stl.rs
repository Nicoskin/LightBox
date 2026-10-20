//! STL, binary and ASCII (Z-up, usually millimetres).

use super::{Builder, Mesh};
use crate::i18n::tr;

pub fn parse(data: &[u8]) -> Result<Mesh, String> {
    let binary_len = data
        .get(80..84)
        .map(|c| 84 + 50 * u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize);
    // Binary files may start with "solid" too: trust the exact size.
    if binary_len == Some(data.len()) || !data.trim_ascii_start().starts_with(b"solid") {
        binary(data)
    } else {
        ascii(data)
    }
}

fn binary(data: &[u8]) -> Result<Mesh, String> {
    let count = data
        .get(80..84)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize)
        .ok_or(tr("STL: файл слишком короткий"))?;
    let mut b = Builder::new(true);
    let f = |o: usize| f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    for i in 0..count {
        let o = 84 + i * 50;
        if o + 50 > data.len() {
            break; // truncated: keep what is there
        }
        let v = |k: usize| {
            let p = o + 12 + k * 12;
            [f(p), f(p + 4), f(p + 8)]
        };
        b.triangle([v(0), v(1), v(2)], None);
    }
    b.finish()
}

fn ascii(data: &[u8]) -> Result<Mesh, String> {
    let text = String::from_utf8_lossy(data);
    let mut b = Builder::new(true);
    let mut v: Vec<[f32; 3]> = Vec::with_capacity(3);
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("vertex") => {
                let n: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if let [x, y, z] = n[..] {
                    v.push([x, y, z]);
                }
            }
            Some("endloop") => {
                if let [a, b2, c] = v[..] {
                    b.triangle([a, b2, c], None);
                } else if v.len() > 3 {
                    for k in 1..v.len() - 1 {
                        b.triangle([v[0], v[k], v[k + 1]], None);
                    }
                }
                v.clear();
            }
            _ => {}
        }
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_binary_agree() {
        let text = b"solid t\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 10 0 0\n   vertex 0 10 0\n  endloop\n endfacet\nendsolid t\n";
        let a = parse(text).unwrap();
        let mut bin = vec![0u8; 80];
        bin.extend_from_slice(&1u32.to_le_bytes());
        for f in [
            0.0f32, 0.0, 1.0, 0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0,
        ] {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        bin.extend_from_slice(&[0, 0]);
        let b = parse(&bin).unwrap();
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.size(), [10.0, 0.0, 10.0]); // Z-up → Y-up: depth is Z
    }
}
