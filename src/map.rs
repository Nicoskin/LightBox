//! A small map for photos with GPS coordinates: OpenStreetMap tiles
//! (downloaded once, cached under %LOCALAPPDATA%\Lightbox\tiles), stitched
//! around the point with a marker. Blocking: call from a worker thread.

use std::path::PathBuf;

use anyhow::{Result, bail};

/// Tile zoom level: streets and districts.
pub const ZOOM: u32 = 15;
const TILE: f64 = 256.0;

/// The world pixel of a coordinate at `ZOOM` (Web Mercator).
fn world(lat: f64, lon: f64) -> (f64, f64) {
    let n = f64::from(1u32 << ZOOM) * TILE;
    let lat = lat.clamp(-85.0511, 85.0511).to_radians();
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| {
        PathBuf::from(d)
            .join("Lightbox")
            .join("tiles")
            .join(ZOOM.to_string())
    })
}

/// One tile as straight RGBA8, from the cache or the tile server.
fn tile(x: u32, y: u32) -> Result<Vec<u8>> {
    let file = cache_dir().map(|d| d.join(format!("{x}_{y}.png")));
    let png = match file.as_ref().and_then(|f| std::fs::read(f).ok()) {
        Some(p) => p,
        None => {
            let url = format!("https://tile.openstreetmap.org/{ZOOM}/{x}/{y}.png");
            let (status, data) = crate::update::http_get(&url)?;
            if status != 200 {
                bail!("сервер карт ответил {status}");
            }
            if let Some(f) = &file {
                if let Some(d) = f.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(f, &data);
            }
            data
        }
    };
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?;
    Ok(img.into_rgba8().into_raw())
}

/// A `w` × `h` map centred on the point, as premultiplied BGRA (opaque).
pub fn render(lat: f64, lon: f64, w: u32, h: u32) -> Result<Vec<u8>> {
    let (cx, cy) = world(lat, lon);
    let (x0, y0) = (cx - f64::from(w) / 2.0, cy - f64::from(h) / 2.0);
    let mut out = vec![0u8; (w * h * 4) as usize];
    let max = 1u32 << ZOOM;
    let (tx0, ty0) = ((x0 / TILE).floor() as i64, (y0 / TILE).floor() as i64);
    let (tx1, ty1) = (
        ((x0 + f64::from(w)) / TILE).floor() as i64,
        ((y0 + f64::from(h)) / TILE).floor() as i64,
    );
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            if ty < 0 || ty >= i64::from(max) {
                continue;
            }
            let wx = tx.rem_euclid(i64::from(max)) as u32;
            let px = tile(wx, ty as u32)?;
            let (ox, oy) = (tx as f64 * TILE - x0, ty as f64 * TILE - y0);
            for row in 0..256i64 {
                let y = oy as i64 + row;
                if !(0..i64::from(h)).contains(&y) {
                    continue;
                }
                for col in 0..256i64 {
                    let x = ox as i64 + col;
                    if !(0..i64::from(w)).contains(&x) {
                        continue;
                    }
                    let s = ((row * 256 + col) * 4) as usize;
                    let d = ((y * i64::from(w) + x) * 4) as usize;
                    // RGBA → BGRA, tiles are opaque.
                    out[d] = px[s + 2];
                    out[d + 1] = px[s + 1];
                    out[d + 2] = px[s];
                    out[d + 3] = 255;
                }
            }
        }
    }
    marker(&mut out, w, h);
    Ok(out)
}

/// A red dot with a white ring in the centre.
fn marker(px: &mut [u8], w: u32, h: u32) {
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    for y in 0..h {
        for x in 0..w {
            let d = ((f64::from(x) + 0.5 - cx).powi(2) + (f64::from(y) + 0.5 - cy).powi(2)).sqrt();
            let c: Option<[u8; 3]> = if d <= 6.0 {
                Some([40, 40, 220]) // BGR red
            } else if d <= 8.5 {
                Some([255, 255, 255])
            } else {
                None
            };
            if let Some(c) = c {
                let i = ((y * w + x) * 4) as usize;
                px[i..i + 3].copy_from_slice(&c);
                px[i + 3] = 255;
            }
        }
    }
}

/// The OpenStreetMap page of the point.
pub fn url(lat: f64, lon: f64) -> String {
    format!("https://www.openstreetmap.org/?mlat={lat:.6}&mlon={lon:.6}#map=16/{lat:.6}/{lon:.6}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mercator() {
        let n = f64::from(1u32 << ZOOM) * TILE;
        let (x, y) = world(0.0, 0.0);
        assert!((x - n / 2.0).abs() < 1e-6 && (y - n / 2.0).abs() < 1e-6);
        // Moscow is north-east of the origin: right and up.
        let (x, y) = world(55.7558, 37.6173);
        assert!(x > n / 2.0 && y < n / 2.0);
    }
}
