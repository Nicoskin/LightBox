//! G-code toolpaths (`.gcode`, `.gco`, and Bambu's `.gcode.3mf`): the
//! extruding moves as segments with their line type, split into layers,
//! plus what slicers write in comments: print time, filament, the preview
//! image. Travel moves are left out.

use super::Mesh;

/// Line types, as the shader colours them.
pub mod kind {
    pub const OTHER: u32 = 0;
    pub const OUTER_WALL: u32 = 1;
    pub const INNER_WALL: u32 = 2;
    pub const SPARSE_INFILL: u32 = 3;
    pub const SOLID_INFILL: u32 = 4;
    pub const TOP: u32 = 5;
    pub const BOTTOM: u32 = 6;
    pub const SUPPORT: u32 = 7;
    pub const SKIRT: u32 = 8;
    pub const BRIDGE: u32 = 9;
    pub const OVERHANG: u32 = 10;
}

/// One extruding move in Y-up model space (as meshes are).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub kind: u32,
    /// Line width, mm.
    pub width: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Toolpath {
    pub segs: Vec<Seg>,
    /// Index past the last segment of each layer.
    pub layer_ends: Vec<u32>,
    /// Height of each layer, mm.
    pub layer_z: Vec<f32>,
    /// "1 ч 23 мин" from the slicer's estimate.
    pub time: Option<String>,
    /// "12,3 г" or "4,56 м".
    pub filament: Option<String>,
    /// The slicer's preview: width, height, straight RGBA.
    pub thumbnail: Option<(u32, u32, Vec<u8>)>,
}

fn line_kind(c: &str) -> Option<u32> {
    let c = c.to_ascii_lowercase();
    let c = c.trim();
    let t = c
        .strip_prefix(";type:")
        .or_else(|| c.strip_prefix("; feature:"))
        .or_else(|| c.strip_prefix(";feature:"))?
        .trim();
    use kind::*;
    Some(
        if t.contains("outer") || t.contains("external") || t == "wall-outer" {
            OUTER_WALL
        } else if t.contains("overhang") {
            OVERHANG
        } else if t.contains("bridge") {
            BRIDGE
        } else if t.contains("top") || t == "skin" {
            TOP
        } else if t.contains("bottom") {
            BOTTOM
        } else if t.contains("solid") {
            SOLID_INFILL
        } else if t.contains("wall") || t.contains("perimeter") {
            INNER_WALL
        } else if t.contains("infill") || t == "fill" {
            SPARSE_INFILL
        } else if t.contains("support") {
            SUPPORT
        } else if t.contains("skirt") || t.contains("brim") {
            SKIRT
        } else {
            OTHER
        },
    )
}

/// "1d 2h 3m 4s" / seconds → "1 д 2 ч 3 мин".
fn human_time(secs: u64) -> String {
    let (d, h, m) = (
        secs / 86400,
        secs / 3600 % 24,
        (secs / 60 % 60).max(u64::from(secs < 60)),
    );
    match (d, h) {
        (0, 0) => format!("{m} мин"),
        (0, _) => format!("{h} ч {m} мин"),
        _ => format!("{d} д {h} ч {m} мин"),
    }
}

/// Seconds from "1d 2h 3m 4s" (slicer style).
fn parse_duration(s: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut num = String::new();
    let mut any = false;
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num.push(ch);
        } else if let Some(k) = match ch {
            'd' => Some(86400),
            'h' => Some(3600),
            'm' => Some(60),
            's' => Some(1),
            _ => None,
        } {
            if let Ok(n) = num.parse::<u64>() {
                total += n * k;
                any = true;
            }
            num.clear();
        } else if !ch.is_whitespace() {
            num.clear();
        }
    }
    any.then_some(total)
}

fn base64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for v in s.bytes().filter_map(val) {
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Parses G-code text.
pub fn parse_text(text: &[u8]) -> Result<Toolpath, String> {
    let mut tp = Toolpath::default();
    let (mut x, mut y, mut z, mut e) = (0f32, 0f32, 0f32, 0f32);
    let (mut abs_xyz, mut abs_e) = (true, true);
    let mut kind = kind::OTHER;
    let mut width = 0.45f32;
    let mut layer_top = f32::MIN;
    // Thumbnails: the biggest one wins.
    let mut thumb: Option<(u32, u32, String)> = None;
    let mut in_thumb: Option<(u32, u32, String)> = None;
    let push = |tp: &mut Toolpath,
                a: [f32; 3],
                b: [f32; 3],
                kind: u32,
                width: f32,
                layer_top: &mut f32| {
        if b[2] > *layer_top + 1e-4 {
            if !tp.segs.is_empty() {
                tp.layer_ends.push(tp.segs.len() as u32);
            }
            tp.layer_z.push(b[2]);
            *layer_top = b[2];
        }
        // Z-up printer space → Y-up model space.
        let conv = |p: [f32; 3]| [p[0], p[2], -p[1]];
        tp.segs.push(Seg {
            a: conv(a),
            b: conv(b),
            kind,
            width,
        });
    };
    for raw in text.split(|&c| c == b'\n') {
        let line = String::from_utf8_lossy(raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with(';') {
            if let Some((w, h, data)) = &mut in_thumb {
                if line.contains("thumbnail end") {
                    let done = (*w, *h, std::mem::take(data));
                    if thumb.as_ref().is_none_or(|t| t.0 * t.1 < done.0 * done.1) {
                        thumb = Some(done);
                    }
                    in_thumb = None;
                } else {
                    data.push_str(line.trim_start_matches(';').trim());
                }
                continue;
            }
            let lower = line.to_ascii_lowercase();
            if lower.contains("thumbnail begin") {
                // "; thumbnail begin 300x300 12345"
                let size = lower.split_whitespace().find_map(|w| {
                    w.split_once('x')
                        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                });
                if let Some((w, h)) = size {
                    in_thumb = Some((w, h, String::new()));
                }
                continue;
            }
            if let Some(k) = line_kind(line) {
                kind = k;
            }
            if let Some(v) = lower
                .strip_prefix(";width:")
                .or_else(|| lower.strip_prefix("; line_width:"))
                .and_then(|v| v.trim().parse::<f32>().ok())
            {
                width = v.clamp(0.05, 3.0);
            }
            if tp.time.is_none()
                && (lower.contains("estimated printing time")
                    || lower.contains("total estimated time"))
            {
                let v = lower.rsplit(['=', ':']).next().unwrap_or("");
                tp.time = parse_duration(v).map(human_time);
            }
            if let Some(v) = lower
                .strip_prefix(";time:")
                .and_then(|v| v.trim().parse::<u64>().ok())
            {
                tp.time = Some(human_time(v));
            }
            if (lower.contains("filament used [g]") || lower.contains("total filament weight"))
                && let Some(g) = lower
                    .rsplit(['=', ':'])
                    .next()
                    .and_then(|v| v.split(',').next())
                    .and_then(|v| v.trim().parse::<f32>().ok())
                && g > 0.0
            {
                tp.filament = Some(format!("{g:.1} г").replace('.', ","));
            }
            if tp.filament.is_none()
                && let Some(m) = lower
                    .strip_prefix(";filament used:")
                    .and_then(|v| v.trim().trim_end_matches('m').parse::<f32>().ok())
            {
                tp.filament = Some(format!("{m:.2} м").replace('.', ","));
            }
            continue;
        }
        let code = line.split(';').next().unwrap_or("").trim();
        let mut words = code.split_whitespace();
        let Some(cmd) = words.next() else {
            continue;
        };
        let cmd = cmd.to_ascii_uppercase();
        let arg = |name: char| -> Option<f32> {
            code.split_whitespace()
                .skip(1)
                .find(|w| w.starts_with(name) || w.starts_with(name.to_ascii_lowercase()))
                .and_then(|w| w[1..].parse().ok())
        };
        match cmd.as_str() {
            "G90" => abs_xyz = true,
            "G91" => abs_xyz = false,
            "M82" => abs_e = true,
            "M83" => abs_e = false,
            "G92" => {
                if let Some(v) = arg('E') {
                    e = v;
                }
            }
            "G0" | "G1" | "G2" | "G3" => {
                let to = |cur: f32, v: Option<f32>| match v {
                    Some(v) if abs_xyz => v,
                    Some(v) => cur + v,
                    None => cur,
                };
                let (nx, ny, nz) = (to(x, arg('X')), to(y, arg('Y')), to(z, arg('Z')));
                let extrudes = match arg('E') {
                    Some(v) if abs_e => {
                        let d = v - e;
                        e = v;
                        d > 1e-6
                    }
                    Some(v) => v > 1e-6,
                    None => false,
                };
                if extrudes && (nx != x || ny != y) {
                    if cmd == "G2" || cmd == "G3" {
                        // Arc around (I, J) relative to the start.
                        let (cx, cy) = (x + arg('I').unwrap_or(0.0), y + arg('J').unwrap_or(0.0));
                        let r = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                        let a0 = (y - cy).atan2(x - cx);
                        let mut a1 = (ny - cy).atan2(nx - cx);
                        let cw = cmd == "G2";
                        if cw && a1 >= a0 {
                            a1 -= std::f32::consts::TAU;
                        } else if !cw && a1 <= a0 {
                            a1 += std::f32::consts::TAU;
                        }
                        let steps = ((a1 - a0).abs() * r / 0.5).ceil().clamp(2.0, 256.0) as u32;
                        let mut prev = [x, y, z];
                        for i in 1..=steps {
                            let t = i as f32 / steps as f32;
                            let a = a0 + (a1 - a0) * t;
                            let p = [cx + r * a.cos(), cy + r * a.sin(), z + (nz - z) * t];
                            push(&mut tp, prev, p, kind, width, &mut layer_top);
                            prev = p;
                        }
                    } else {
                        push(
                            &mut tp,
                            [x, y, z],
                            [nx, ny, nz],
                            kind,
                            width,
                            &mut layer_top,
                        );
                    }
                }
                (x, y, z) = (nx, ny, nz);
            }
            _ => {}
        }
    }
    if tp.segs.is_empty() {
        return Err("в G-code нет движений с подачей пластика".into());
    }
    tp.layer_ends.push(tp.segs.len() as u32);
    if let Some((_, _, data)) = thumb
        && let Ok(img) = image::load_from_memory(&base64(&data))
    {
        let img = img.into_rgba8();
        tp.thumbnail = Some((img.width(), img.height(), img.into_raw()));
    }
    Ok(tp)
}

/// A mesh holding the toolpath (bounds for the camera and the floor).
pub fn parse(data: &[u8]) -> Result<Mesh, String> {
    // Bambu Studio's printable project: the G-code of the first plate.
    let tp = if data.starts_with(b"PK") {
        let files = super::threemf::unzip(data)?;
        let mut names: Vec<&String> = files.keys().filter(|n| n.ends_with(".gcode")).collect();
        names.sort();
        let name = names.first().ok_or("в архиве нет G-code")?;
        parse_text(&files[*name])?
    } else {
        parse_text(data)?
    };
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for s in &tp.segs {
        for p in [s.a, s.b] {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
    }
    Ok(Mesh {
        min,
        max,
        path: Some(Box::new(tp)),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
; thumbnail begin 1x1 92
; iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==
; thumbnail end
; estimated printing time (normal mode) = 1h 2m 30s
; filament used [g] = 12.34
G90
M82
G92 E0
G1 Z0.2 F600
;TYPE:External perimeter
G1 X10 Y0 E1
G1 X10 Y10 E2
G1 X20 Y20 F9000
;TYPE:Internal infill
G1 X30 Y20 E3
G1 Z0.4
G1 X40 Y20 E4
G2 X40 Y30 I0 J5 E5
";

    #[test]
    fn layers_kinds_and_comments() {
        let tp = parse_text(SAMPLE.as_bytes()).unwrap();
        // Two outer-wall moves and one infill move on layer 1, then a
        // straight move and an arc on layer 2 (travel moves dropped).
        assert_eq!(tp.layer_z, [0.2, 0.4]);
        assert_eq!(tp.layer_ends[0], 3);
        assert_eq!(tp.segs[0].kind, kind::OUTER_WALL);
        assert_eq!(tp.segs[2].kind, kind::SPARSE_INFILL);
        assert!(tp.segs.len() > 5, "arc split into pieces");
        // Z-up → Y-up: printer (10, 0, 0.2) is (10, 0.2, 0).
        assert_eq!(tp.segs[0].b, [10.0, 0.2, -0.0]);
        assert_eq!(tp.time.as_deref(), Some("1 ч 2 мин"));
        assert_eq!(tp.filament.as_deref(), Some("12,3 г"));
        assert_eq!(tp.thumbnail.as_ref().map(|t| (t.0, t.1)), Some((1, 1)));
    }

    #[test]
    fn relative_extrusion_and_cura() {
        let g = ";TIME:5400\n;Filament used: 1.5m\nM83\nG1 Z0.3\nG1 X5 Y5 E0.5\nG1 X6 Y5 E-0.2\n";
        let tp = parse_text(g.as_bytes()).unwrap();
        assert_eq!(tp.segs.len(), 1, "retraction is not a line");
        assert_eq!(tp.time.as_deref(), Some("1 ч 30 мин"));
        assert_eq!(tp.filament.as_deref(), Some("1,50 м"));
        assert!(parse_text(b"G1 X1 Y1\n").is_err());
    }
}
