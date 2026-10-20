//! Inspector data: the histogram of a decoded image and what a file tells
//! about itself (size, EXIF, AI generation parameters from A1111 / ComfyUI).

use std::io::{BufReader, Cursor};
use std::path::Path;

use serde_json::Value;

use crate::decode::{DecodedImage, Pixels};
use crate::i18n::tr;

/// Pixels sampled at most (larger images are subsampled evenly).
const HISTOGRAM_SAMPLES: usize = 500_000;

/// 256-bin histograms of the straight sRGB values.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub r: [u32; 256],
    pub g: [u32; 256],
    pub b: [u32; 256],
    /// Rec. 709 luma of the sRGB values.
    pub luma: [u32; 256],
}

/// Histogram of `img` (fully transparent pixels are skipped).
pub fn histogram(img: &DecodedImage) -> Histogram {
    let mut h = Histogram {
        r: [0; 256],
        g: [0; 256],
        b: [0; 256],
        luma: [0; 256],
    };
    let n = img.width as usize * img.height as usize;
    let step = n.div_ceil(HISTOGRAM_SAMPLES).max(1);
    let mut add = |p: [u8; 4]| {
        if p[3] == 0 {
            return;
        }
        h.r[usize::from(p[0])] += 1;
        h.g[usize::from(p[1])] += 1;
        h.b[usize::from(p[2])] += 1;
        let l = 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]);
        h.luma[(l + 0.5).min(255.0) as usize] += 1;
    };
    match &img.pixels {
        Pixels::Srgb8(v) => {
            for p in v.as_chunks::<4>().0.iter().step_by(step) {
                // Premultiplied in linear light: opaque pixels are as stored.
                if p[3] == 255 {
                    add(*p);
                } else if let Ok(s) = <[u8; 4]>::try_from(crate::decode::unpremultiply(p)) {
                    add(s);
                }
            }
        }
        Pixels::Linear16F(v) => {
            for p in v.as_chunks::<4>().0.iter().step_by(step) {
                if let Ok(s) = <[u8; 4]>::try_from(crate::decode::f16_to_straight_srgb8(p)) {
                    add(s);
                }
            }
        }
    }
    h
}

/// Generation parameters embedded by Stable Diffusion front-ends.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AiMeta {
    /// "A1111" or "ComfyUI".
    pub tool: &'static str,
    pub prompt: String,
    pub negative: String,
    /// Seed, steps, sampler, CFG, model…
    pub params: Vec<(String, String)>,
    /// ComfyUI workflow JSON (for "copy workflow").
    pub workflow: Option<String>,
    /// Everything as found in the file (for "copy all").
    pub raw: String,
}

/// What the file says about itself, for the info panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileInfo {
    /// File size in bytes and the modification date.
    pub size: u64,
    pub modified: Option<String>,
    /// EXIF rows: label, value.
    pub exif: Vec<(&'static str, String)>,
    /// GPS position: latitude, longitude (degrees, south and west negative).
    pub gps: Option<(f64, f64)>,
    pub ai: Option<AiMeta>,
}

/// Reads the file's metadata (call off the UI thread).
pub fn gather(path: &Path) -> FileInfo {
    let mut info = FileInfo::default();
    if let Ok(m) = std::fs::metadata(path) {
        info.size = m.len();
        info.modified = m.modified().ok().map(format_time);
    }
    let Ok(data) = std::fs::read(path) else {
        return info;
    };
    let exif = exif::Reader::new()
        .read_from_container(&mut BufReader::new(Cursor::new(&data)))
        .ok();
    if let Some(e) = &exif {
        info.exif = exif_rows(e);
        info.gps = gps_coords(e);
    }
    info.ai = png_text(&data)
        .and_then(|chunks| ai_from_text(&chunks))
        .or_else(|| {
            exif.as_ref()
                .and_then(user_comment)
                .and_then(|t| ai_from_text(&[("parameters".into(), t)]))
        });
    info
}

/// "18.10.2026 14:05".
fn format_time(t: std::time::SystemTime) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let Ok(since) = t.duration_since(std::time::UNIX_EPOCH) else {
        return String::new();
    };
    // FILETIME: 100 ns ticks since 1601.
    let ticks = since.as_nanos() as u64 / 100 + 116_444_736_000_000_000;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: valid in/out pointers for the duration of the calls.
    let ok = unsafe {
        FileTimeToSystemTime(&ft, &mut utc).is_ok()
            && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok()
    };
    if !ok {
        return String::new();
    }
    format!(
        "{:02}.{:02}.{} {:02}:{:02}",
        local.wDay, local.wMonth, local.wYear, local.wHour, local.wMinute
    )
}

fn exif_rows(e: &exif::Exif) -> Vec<(&'static str, String)> {
    use exif::{In, Tag};
    let text = |tag: Tag| {
        e.get_field(tag, In::PRIMARY).map(|f| {
            f.display_value()
                .with_unit(e)
                .to_string()
                .trim_matches('"')
                .trim()
                .to_string()
        })
    };
    let mut rows = Vec::new();
    let camera = match (text(Tag::Make), text(Tag::Model)) {
        (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => make.or(model),
    };
    if let Some(c) = camera.filter(|c| !c.is_empty()) {
        rows.push((tr("Камера"), c));
    }
    for (label, tag) in [
        (tr("Объектив"), Tag::LensModel),
        (tr("Выдержка"), Tag::ExposureTime),
        (tr("Диафрагма"), Tag::FNumber),
        ("ISO", Tag::PhotographicSensitivity),
        (tr("Фокусное"), Tag::FocalLength),
        (tr("Снято"), Tag::DateTimeOriginal),
    ] {
        if let Some(v) = text(tag).filter(|v| !v.is_empty()) {
            rows.push((label, v));
        }
    }
    if let Some(gps) = gps(e) {
        rows.push(("GPS", gps));
    }
    rows
}

/// "55.75580° N, 37.61730° E".
fn gps(e: &exif::Exif) -> Option<String> {
    let (lat, lon) = gps_coords(e)?;
    let (ns, ew) = (
        if lat < 0.0 { "S" } else { "N" },
        if lon < 0.0 { "W" } else { "E" },
    );
    Some(format!("{:.5}° {ns}, {:.5}° {ew}", lat.abs(), lon.abs()))
}

/// Latitude and longitude in degrees (south and west negative).
fn gps_coords(e: &exif::Exif) -> Option<(f64, f64)> {
    use exif::{In, Tag, Value};
    let coord = |tag: Tag, reference: Tag| -> Option<(f64, String)> {
        let Value::Rational(v) = &e.get_field(tag, In::PRIMARY)?.value else {
            return None;
        };
        let deg = v.first()?.to_f64() + v.get(1)?.to_f64() / 60.0 + v.get(2)?.to_f64() / 3600.0;
        let r = e
            .get_field(reference, In::PRIMARY)?
            .display_value()
            .to_string();
        Some((deg, r.trim_matches('"').to_string()))
    };
    let (lat, ns) = coord(Tag::GPSLatitude, Tag::GPSLatitudeRef)?;
    let (lon, ew) = coord(Tag::GPSLongitude, Tag::GPSLongitudeRef)?;
    let lat = if ns.starts_with('S') { -lat } else { lat };
    let lon = if ew.starts_with('W') { -lon } else { lon };
    (lat.is_finite() && lon.is_finite() && (lat, lon) != (0.0, 0.0)).then_some((lat, lon))
}

/// EXIF UserComment as text (A1111 stores its parameters there in JPEG /
/// WebP): "ASCII\0\0\0" or "UNICODE\0" (UTF-16) prefixed.
fn user_comment(e: &exif::Exif) -> Option<String> {
    let f = e.get_field(exif::Tag::UserComment, exif::In::PRIMARY)?;
    let exif::Value::Undefined(bytes, _) = &f.value else {
        return None;
    };
    let (head, body) = (bytes.get(..8)?, bytes.get(8..)?);
    let text = if head.starts_with(b"UNICODE") {
        let units = |be: bool| -> Vec<u16> {
            body.as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    if be {
                        u16::from_be_bytes(*c)
                    } else {
                        u16::from_le_bytes(*c)
                    }
                })
                .collect()
        };
        // Byte order is not recorded: ASCII-range text has its zero bytes
        // first in big-endian.
        let zeros_first = body.iter().step_by(2).filter(|&&b| b == 0).count();
        let be = zeros_first * 2 >= body.len() / 2;
        String::from_utf16_lossy(&units(be))
    } else {
        String::from_utf8_lossy(body).into_owned()
    };
    let text = text.trim_matches('\0').trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// PNG text chunks (tEXt, zTXt, iTXt) as (keyword, text).
fn png_text(data: &[u8]) -> Option<Vec<(String, String)>> {
    if !data.starts_with(b"\x89PNG") {
        return None;
    }
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_ignore_text_chunk(false);
    let reader = decoder.read_info().ok()?;
    let info = reader.info();
    let mut out: Vec<(String, String)> = info
        .uncompressed_latin1_text
        .iter()
        .map(|c| (c.keyword.clone(), c.text.clone()))
        .collect();
    for c in &info.compressed_latin1_text {
        if let Ok(t) = c.get_text() {
            out.push((c.keyword.clone(), t));
        }
    }
    for c in &info.utf8_text {
        if let Ok(t) = c.get_text() {
            out.push((c.keyword.clone(), t));
        }
    }
    Some(out)
}

/// Recognises A1111 ("parameters") and ComfyUI ("prompt", "workflow").
fn ai_from_text(chunks: &[(String, String)]) -> Option<AiMeta> {
    let get = |k: &str| {
        chunks
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(k))
            .map(|(_, t)| t.as_str())
    };
    let workflow = get("workflow").map(str::to_string);
    if let Some(prompt) = get("prompt")
        && let Ok(json) = serde_json::from_str::<Value>(prompt)
        && json.is_object()
    {
        let mut meta = comfy_prompt(&json);
        meta.workflow = workflow;
        meta.raw = prompt.to_string();
        return Some(meta);
    }
    if let Some(params) = get("parameters") {
        let mut meta = a1111(params);
        meta.workflow = workflow;
        return Some(meta);
    }
    let w = workflow?;
    let json = serde_json::from_str::<Value>(&w).ok()?;
    let mut meta = comfy_workflow(&json);
    meta.raw.clone_from(&w);
    meta.workflow = Some(w);
    Some(meta)
}

/// A1111 / Forge "parameters": prompt, "Negative prompt: …", then a line
/// of "Key: value" pairs starting with "Steps:".
fn a1111(text: &str) -> AiMeta {
    let (body, settings) = match text.rfind("\nSteps:") {
        Some(i) => (&text[..i], text[i + 1..].trim()),
        None if text.starts_with("Steps:") => ("", text.trim()),
        None => (text, ""),
    };
    let (prompt, negative) = match body.find("Negative prompt:") {
        Some(i) => (&body[..i], &body[i + "Negative prompt:".len()..]),
        None => (body, ""),
    };
    AiMeta {
        tool: "A1111",
        prompt: prompt.trim().to_string(),
        negative: negative.trim().to_string(),
        params: a1111_params(settings),
        workflow: None,
        raw: text.to_string(),
    }
}

/// "Steps: 30, Sampler: DPM++ 2M, Lora hashes: \"a: 1, b: 2\"" → pairs
/// (commas inside quotes do not split).
fn a1111_params(line: &str) -> Vec<(String, String)> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            ',' if !quoted => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    parts.push(cur);
    parts
        .iter()
        .filter_map(|p| {
            let (k, v) = p.split_once(':')?;
            Some((k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        })
        .collect()
}

/// ComfyUI API-format prompt: `{ "id": { "class_type", "inputs" } }`.
fn comfy_prompt(json: &Value) -> AiMeta {
    let nodes = json.as_object();
    let node = |id: &Value| -> Option<&Value> {
        let id = match id {
            Value::Array(a) => a.first()?,
            other => other,
        };
        let key = match id {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return None,
        };
        nodes?.get(&key)
    };
    // Text of a conditioning input, following links a few levels deep.
    fn text_of<'a>(
        v: &'a Value,
        node: &dyn Fn(&Value) -> Option<&'a Value>,
        depth: u32,
    ) -> Option<String> {
        let n = node(v)?;
        let inputs = n.get("inputs")?.as_object()?;
        for key in ["text", "text_g", "prompt", "positive", "string", "text_l"] {
            match inputs.get(key) {
                Some(Value::String(s)) if !s.trim().is_empty() => return Some(s.clone()),
                Some(link @ Value::Array(_)) if depth < 6 => {
                    if let Some(t) = text_of(link, node, depth + 1) {
                        return Some(t);
                    }
                }
                _ => {}
            }
        }
        // Combine / concat nodes: the first input that leads to text.
        inputs
            .values()
            .filter(|v| v.is_array())
            .find_map(|v| (depth < 6).then(|| text_of(v, node, depth + 1)).flatten())
    }
    let all: Vec<&Value> = nodes.map(|m| m.values().collect()).unwrap_or_default();
    fn class(n: &Value) -> &str {
        n.get("class_type").and_then(Value::as_str).unwrap_or("")
    }
    let sampler = all
        .iter()
        .find(|n| class(n).contains("KSampler") || class(n).contains("SamplerCustom"));
    let mut meta = AiMeta {
        tool: "ComfyUI",
        ..Default::default()
    };
    if let Some(s) = sampler.and_then(|s| s.get("inputs")) {
        if let Some(p) = s.get("positive") {
            meta.prompt = text_of(p, &node, 0).unwrap_or_default();
        }
        if let Some(n) = s.get("negative") {
            meta.negative = text_of(n, &node, 0).unwrap_or_default();
        }
        for (label, keys) in [
            ("Seed", &["seed", "noise_seed"][..]),
            ("Steps", &["steps"]),
            ("CFG", &["cfg"]),
            ("Sampler", &["sampler_name"]),
            ("Scheduler", &["scheduler"]),
            ("Denoise", &["denoise"]),
        ] {
            if let Some(v) = keys.iter().find_map(|k| s.get(*k)).and_then(scalar) {
                meta.params.push((label.into(), v));
            }
        }
    }
    if meta.prompt.is_empty() {
        // No sampler found: the first text encoder.
        meta.prompt = all
            .iter()
            .filter(|n| class(n).contains("TextEncode"))
            .find_map(|n| n.get("inputs")?.get("text")?.as_str().map(str::to_string))
            .unwrap_or_default();
    }
    let model = all.iter().find_map(|n| {
        let i = n.get("inputs")?;
        ["ckpt_name", "unet_name", "model_name"]
            .iter()
            .find_map(|k| i.get(*k)?.as_str().map(str::to_string))
    });
    if let Some(m) = model {
        meta.params.insert(0, ("Model".into(), m));
    }
    meta
}

/// ComfyUI UI-format workflow (no API prompt): text encoders' widget values.
fn comfy_workflow(json: &Value) -> AiMeta {
    let texts: Vec<String> = json
        .get("nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|n| {
            n.get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.contains("TextEncode"))
        })
        .filter_map(|n| {
            n.get("widgets_values")?
                .get(0)?
                .as_str()
                .map(str::to_string)
        })
        .collect();
    AiMeta {
        tool: "ComfyUI",
        prompt: texts.first().cloned().unwrap_or_default(),
        negative: texts.get(1).cloned().unwrap_or_default(),
        ..Default::default()
    }
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// "4.8 MB", "512 KB".
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        crate::t!("{} ГБ", format!("{:.2}", b / 1024.0 / 1024.0 / 1024.0))
    } else if b >= 1024.0 * 1024.0 {
        crate::t!("{} МБ", format!("{:.1}", b / 1024.0 / 1024.0))
    } else if b >= 1024.0 {
        crate::t!("{} КБ", format!("{:.0}", b / 1024.0))
    } else {
        crate::t!("{} Б", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a1111_parameters() {
        let m = a1111(
            "a lighthouse, oil\nsecond line\nNegative prompt: blurry, ugly\n\
             Steps: 30, Sampler: DPM++ 2M, CFG scale: 7, Seed: 1234, \
             Lora hashes: \"a: 1, b: 2\", Model: sdxl",
        );
        assert_eq!(m.prompt, "a lighthouse, oil\nsecond line");
        assert_eq!(m.negative, "blurry, ugly");
        let p = |k: &str| {
            m.params
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(p("Seed"), Some("1234"));
        assert_eq!(p("Lora hashes"), Some("a: 1, b: 2"));
        assert_eq!(p("Model"), Some("sdxl"));
        assert_eq!(a1111("just a prompt").prompt, "just a prompt");
    }

    #[test]
    fn comfy_prompt_follows_links() {
        let json: Value = serde_json::from_str(
            r#"{
              "3": {"class_type": "KSampler", "inputs": {"seed": 42, "steps": 20, "cfg": 7.5,
                    "sampler_name": "euler", "positive": ["6", 0], "negative": ["7", 0],
                    "model": ["4", 0]}},
              "4": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "sdxl.safetensors"}},
              "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "a red fox", "clip": ["4", 1]}},
              "7": {"class_type": "CLIPTextEncode", "inputs": {"text": ["8", 0]}},
              "8": {"class_type": "PrimitiveString", "inputs": {"string": "watermark"}}
            }"#,
        )
        .unwrap();
        let m = comfy_prompt(&json);
        assert_eq!(m.prompt, "a red fox");
        assert_eq!(m.negative, "watermark");
        assert!(m.params.contains(&("Seed".into(), "42".into())));
        assert_eq!(m.params[0], ("Model".into(), "sdxl.safetensors".into()));
    }

    #[test]
    fn png_text_chunks_and_camera_exif() {
        // A tiny PNG with ComfyUI chunks, built here (no sample file).
        let mut bytes = Vec::new();
        {
            let mut e = png::Encoder::new(&mut bytes, 2, 2);
            e.set_color(png::ColorType::Rgb);
            e.add_text_chunk(
                "prompt".into(),
                r#"{"3":{"class_type":"KSampler","inputs":{"seed":7,"positive":["6",0]}},"6":{"class_type":"CLIPTextEncode","inputs":{"text":"a red fox"}}}"#.into(),
            )
            .unwrap();
            e.add_ztxt_chunk("workflow".into(), r#"{"nodes":[]}"#.into())
                .unwrap();
            let mut w = e.write_header().unwrap();
            w.write_image_data(&[0; 12]).unwrap();
        }
        let ai = ai_from_text(&png_text(&bytes).unwrap()).expect("ai");
        assert_eq!((ai.tool, ai.prompt.as_str()), ("ComfyUI", "a red fox"));
        assert_eq!(ai.workflow.as_deref(), Some(r#"{"nodes":[]}"#));

        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/camera_exif.jpg");
        if !p.exists() {
            return;
        }
        let cam = gather(&p);
        assert!(cam.size > 0 && cam.modified.is_some());
        let row = |l: &str| {
            cam.exif
                .iter()
                .find(|(k, _)| *k == l)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(row("Камера").as_deref(), Some("Canon EOS R6"));
        assert_eq!(row("ISO").as_deref(), Some("200"));
        assert!(
            row("GPS").is_some_and(|g| g.starts_with("55.7558")),
            "{:?}",
            cam.exif
        );
    }

    #[test]
    fn histogram_counts_pixels() {
        let img = crate::decode::decode_file(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/tiny_1x1.png"),
            None,
        );
        let Ok(img) = img else {
            return;
        };
        let h = histogram(&img);
        assert_eq!(h.r[255], 1);
        assert_eq!(h.g[0], 1);
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 МБ");
    }
}
