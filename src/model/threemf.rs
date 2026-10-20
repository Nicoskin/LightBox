//! 3MF: a ZIP package with the model in XML (Z-up, units in the file).
//! Objects may be meshes or assemblies of other objects (components),
//! each placed by a transform; the build lists what is printed. Slicer
//! projects (Bambu Studio, Orca, PrusaSlicer) add objects in other model
//! parts, filament colours and several build plates.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use super::{Builder, Group, Mesh};
use crate::i18n::tr;

/// 3 × 4 affine transform, 3MF order: m00 m01 m02 m10 … m32 (row vectors).
type Transform = [f32; 12];
const IDENTITY: Transform = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

fn apply(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    [
        p[0] * t[0] + p[1] * t[3] + p[2] * t[6] + t[9],
        p[0] * t[1] + p[1] * t[4] + p[2] * t[7] + t[10],
        p[0] * t[2] + p[1] * t[5] + p[2] * t[8] + t[11],
    ]
}

/// `a` then `b`.
fn compose(a: &Transform, b: &Transform) -> Transform {
    let mut out = [0.0; 12];
    for r in 0..4 {
        let row = if r < 3 {
            [a[r * 3], a[r * 3 + 1], a[r * 3 + 2], 0.0]
        } else {
            [a[9], a[10], a[11], 1.0]
        };
        for c in 0..3 {
            out[r * 3 + c] =
                row[0] * b[c] + row[1] * b[3 + c] + row[2] * b[6 + c] + row[3] * b[9 + c];
        }
    }
    out
}

fn parse_transform(s: Option<&str>) -> Transform {
    let Some(s) = s else {
        return IDENTITY;
    };
    let v: Vec<f32> = s
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    v.try_into().unwrap_or(IDENTITY)
}

/// "#RRGGBB" / "#RRGGBBAA" → sRGB.
fn parse_color(s: &str) -> Option<[f32; 4]> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    let a = if h.len() >= 8 { byte(6)? } else { 255 };
    Some([byte(0)?, byte(2)?, byte(4)?, a].map(|v| f32::from(v) / 255.0))
}

/// What a slicer project says about filaments: their colours and which
/// extruder each object, part (component) or volume (run of triangles) uses.
#[derive(Default)]
struct Filaments {
    palette: Vec<[f32; 4]>,
    objects: HashMap<String, usize>,
    /// (object id, component object id) → extruder (Bambu Studio, Orca).
    parts: HashMap<(String, String), usize>,
    /// PrusaSlicer: triangle runs of an object's mesh (first, last, extruder).
    volumes: HashMap<String, Vec<(usize, usize, usize)>>,
    /// Object names the slicer shows.
    names: HashMap<String, String>,
}

impl Filaments {
    /// Extruders count from 1.
    fn color(&self, extruder: usize) -> Option<[f32; 4]> {
        self.palette.get(extruder.checked_sub(1)?).copied()
    }
}

/// A build plate from the project settings: the object instances on it.
struct PlateInfo {
    id: usize,
    name: String,
    instances: Vec<(String, usize)>,
}

fn file<'f>(files: &'f HashMap<String, Vec<u8>>, name: &str) -> Option<&'f Vec<u8>> {
    files
        .iter()
        .find(|(k, _)| k.trim_start_matches('/').eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

/// Filament colours: Bambu Studio / Orca keep them in a JSON config,
/// PrusaSlicer in an INI-like one (extruder colour, else filament colour).
fn palette(files: &HashMap<String, Vec<u8>>) -> Vec<[f32; 4]> {
    if let Some(json) = file(files, "Metadata/project_settings.config")
        && let Ok(v) = serde_json::from_slice::<serde_json::Value>(json)
        && let Some(list) = v.get("filament_colour").and_then(|c| c.as_array())
    {
        return list
            .iter()
            .map(|c| {
                c.as_str()
                    .and_then(parse_color)
                    .unwrap_or(super::DEFAULT_COLOR)
            })
            .collect();
    }
    let Some(ini) = file(files, "Metadata/Slic3r_PE.config") else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(ini);
    let list = |key: &str| -> Vec<Option<[f32; 4]>> {
        text.lines()
            .filter_map(|l| l.trim_start_matches([';', ' ']).strip_prefix(key))
            .filter_map(|l| l.trim_start().strip_prefix('='))
            .flat_map(|v| {
                v.split(';')
                    .map(|c| parse_color(c.trim().trim_matches('"')))
            })
            .collect()
    };
    let (extruders, filaments) = (list("extruder_colour"), list("filament_colour"));
    (0..extruders.len().max(filaments.len()))
        .map(|i| {
            extruders
                .get(i)
                .copied()
                .flatten()
                .or(filaments.get(i).copied().flatten())
                .unwrap_or(super::DEFAULT_COLOR)
        })
        .collect()
}

/// Extruders of objects, parts and volumes, and the build plates, from the
/// slicer's model settings.
fn model_settings(files: &HashMap<String, Vec<u8>>, f: &mut Filaments) -> Vec<PlateInfo> {
    let mut plates = Vec::new();
    let meta = |n: Node, key: &str| {
        n.children()
            .find(|c| c.has_tag_name("metadata") && c.attribute("key") == Some(key))
            .and_then(|c| c.attribute("value").map(str::to_string))
    };
    let number = |n: Node, key: &str| meta(n, key).and_then(|v| v.trim().parse::<usize>().ok());
    // 0 means "as the object".
    let extruder = |n: Node| number(n, "extruder").filter(|&e| e > 0);
    for name in [
        "Metadata/model_settings.config",
        "Metadata/Slic3r_PE_model.config",
    ] {
        let Some(bytes) = file(files, name) else {
            continue;
        };
        let text = String::from_utf8_lossy(bytes);
        let Ok(doc) = Document::parse(&text) else {
            continue;
        };
        let root = doc.root_element();
        for obj in root.children().filter(|n| n.has_tag_name("object")) {
            let Some(id) = obj.attribute("id") else {
                continue;
            };
            if let Some(e) = extruder(obj) {
                f.objects.insert(id.to_string(), e);
            }
            if let Some(name) = meta(obj, "name").filter(|n| !n.trim().is_empty()) {
                f.names.insert(id.to_string(), name.trim().to_string());
            }
            for c in obj.children().filter(Node::is_element) {
                let attr = |k| c.attribute(k).and_then(|s| s.parse::<usize>().ok());
                match (c.tag_name().name(), extruder(c)) {
                    ("part", Some(e)) => {
                        if let Some(part) = c.attribute("id") {
                            f.parts.insert((id.to_string(), part.to_string()), e);
                        }
                    }
                    ("volume", Some(e)) => {
                        if let (Some(a), Some(b)) = (attr("firstid"), attr("lastid")) {
                            f.volumes.entry(id.to_string()).or_default().push((a, b, e));
                        }
                    }
                    _ => {}
                }
            }
        }
        for plate in root.children().filter(|n| n.has_tag_name("plate")) {
            let instances = plate
                .children()
                .filter(|n| n.has_tag_name("model_instance"))
                .filter_map(|mi| {
                    Some((
                        meta(mi, "object_id")?,
                        number(mi, "instance_id").unwrap_or(0),
                    ))
                })
                .collect();
            plates.push(PlateInfo {
                id: number(plate, "plater_id").unwrap_or(plates.len() + 1),
                name: meta(plate, "plater_name")
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                instances,
            });
        }
    }
    plates
}

/// The objects of every model part, the colours of their resources and the
/// project's filaments.
struct Scene<'d, 't> {
    /// Model parts by normalised name → index.
    parts: HashMap<String, usize>,
    objects: HashMap<(usize, &'d str), Node<'d, 't>>,
    colors: HashMap<(usize, &'d str, usize), [f32; 4]>,
    filaments: Filaments,
}

fn part_name(name: &str) -> String {
    name.trim_start_matches('/').to_ascii_lowercase()
}

pub fn parse(data: &[u8]) -> Result<Mesh, String> {
    let files = unzip(data)?;
    // The root model part is named in _rels/.rels; "3D/3dmodel.model" by default.
    let model_name = files
        .get("_rels/.rels")
        .and_then(|r| {
            let text = String::from_utf8_lossy(r).into_owned();
            let doc = Document::parse(&text).ok()?;
            doc.descendants()
                .find(|n| n.attribute("Type").is_some_and(|t| t.ends_with("/3dmodel")))
                .and_then(|n| n.attribute("Target"))
                .map(part_name)
        })
        .unwrap_or_else(|| "3d/3dmodel.model".into());
    // Slicer projects keep objects in other parts (production extension).
    let texts: Vec<(String, String)> = files
        .iter()
        .map(|(k, v)| (part_name(k), v))
        .filter(|(k, _)| k == &model_name || k.ends_with(".model"))
        .map(|(k, v)| (k, String::from_utf8_lossy(v).into_owned()))
        .collect();
    let root_index = texts
        .iter()
        .position(|(k, _)| *k == model_name)
        .ok_or(tr("3MF: нет модели в пакете"))?;
    let mut docs = Vec::with_capacity(texts.len());
    for (i, (_, text)) in texts.iter().enumerate() {
        match Document::parse(text) {
            Ok(d) => docs.push(Some(d)),
            Err(e) if i == root_index => return Err(format!("3MF: {e}")),
            Err(_) => docs.push(None),
        }
    }
    let mut scene = Scene {
        parts: texts
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (k.clone(), i))
            .collect(),
        objects: HashMap::new(),
        colors: HashMap::new(),
        filaments: Filaments {
            palette: palette(&files),
            ..Default::default()
        },
    };
    let plates = model_settings(&files, &mut scene.filaments);
    for (i, doc) in docs.iter().enumerate() {
        let Some(doc) = doc else {
            continue;
        };
        for n in doc.descendants() {
            let name = n.tag_name().name();
            let Some(id) = n.attribute("id") else {
                continue;
            };
            if name == "object" {
                scene.objects.insert((i, id), n);
            }
            // Colours: base materials and colour groups, by (group id, index).
            if name == "basematerials" || name == "colorgroup" {
                for (k, item) in n.children().filter(Node::is_element).enumerate() {
                    let c = item.attribute("displaycolor").or(item.attribute("color"));
                    if let Some(c) = c.and_then(parse_color) {
                        scene.colors.insert((i, id, k), c);
                    }
                }
            }
        }
    }

    let root = docs[root_index].as_ref().ok_or("3MF")?.root_element();
    let build = root.children().find(|n| n.has_tag_name("build"));
    let items: Vec<(&str, Transform)> = match build {
        Some(build) => build
            .children()
            .filter(|n| n.has_tag_name("item"))
            .filter_map(|n| {
                Some((
                    n.attribute("objectid")?,
                    parse_transform(n.attribute("transform")),
                ))
            })
            .collect(),
        None => scene
            .objects
            .keys()
            .filter(|(d, _)| *d == root_index)
            .map(|(_, id)| (*id, IDENTITY))
            .collect(),
    };
    // Each build item is an instance of its object; the project says which
    // plate it is on. Items go plate by plate so each plate is one run.
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let plate_of: Vec<usize> = items
        .iter()
        .map(|(id, _)| {
            let n = seen.entry(id).or_default();
            let instance = *n;
            *n += 1;
            plates
                .iter()
                .position(|p| p.instances.iter().any(|(o, k)| o == id && *k == instance))
                .unwrap_or(usize::MAX)
        })
        .collect();
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| plate_of[i]);
    let mut b = Builder::new(true);
    let mut runs: Vec<Option<std::ops::Range<usize>>> = vec![None; plates.len()];
    let mut objects: Vec<Group> = Vec::new();
    let mut named: HashMap<String, usize> = HashMap::new();
    for i in order {
        let (id, t) = &items[i];
        let start = b.count();
        let paint = match scene.filaments.objects.get(*id) {
            Some(&e) => scene.filaments.color(e),
            None => scene.filaments.color(1),
        };
        add_object(&scene, root_index, id, t, &mut b, (0, id), paint);
        if let Some(r) = runs.get_mut(plate_of[i]) {
            let from = r.as_ref().map_or(start, |r| r.start);
            *r = Some(from..b.count());
        }
        if b.count() > start {
            // The slicer's name, the file's, or a number; copies numbered.
            let base = scene
                .filaments
                .names
                .get(*id)
                .cloned()
                .or_else(|| {
                    scene
                        .objects
                        .get(&(root_index, *id))
                        .and_then(|o| o.attribute("name"))
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_string)
                })
                .map(|n| strip_mesh_ext(&n).to_string())
                .unwrap_or_else(|| crate::t!("Объект {}", objects.len() + 1));
            let n = named.entry(base.clone()).or_default();
            *n += 1;
            let name = if *n > 1 {
                format!("{base} ({n})")
            } else {
                base
            };
            objects.push(Group {
                name,
                tris: start..b.count(),
            });
        }
    }
    if objects.len() > 1 {
        b.set_objects(objects);
    }
    let found: Vec<Group> = plates
        .iter()
        .zip(runs)
        .filter_map(|(p, r)| Some((p, r.filter(|r| r.start < r.end)?)))
        .map(|(p, tris)| Group {
            name: if p.name.is_empty() {
                crate::t!("Стол {}", p.id)
            } else {
                crate::t!("Стол {} · {}", p.id, p.name)
            },
            tris,
        })
        .collect();
    if found.len() > 1 {
        b.set_plates(found);
    }
    b.finish()
}

/// Slicers name objects after the files they came from: "cube.stl" → "cube".
fn strip_mesh_ext(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    [".stl", ".obj", ".3mf", ".step", ".stp", ".amf"]
        .iter()
        .find(|e| lower.ends_with(*e))
        .map_or(name, |e| &name[..name.len() - e.len()])
}

/// Adds object `id` of model part `part`. `top`: nesting depth and the
/// id of the build item's object (filament settings refer to it); `paint`:
/// the filament colour inherited from above.
fn add_object(
    scene: &Scene,
    part: usize,
    id: &str,
    t: &Transform,
    b: &mut Builder,
    top: (u32, &str),
    paint: Option<[f32; 4]>,
) {
    let (depth, top_id) = top;
    let Some(obj) = scene.objects.get(&(part, id)) else {
        return;
    };
    if depth > 16 {
        return; // cyclic components
    }
    let pid = obj.attribute("pid");
    let pindex: usize = obj
        .attribute("pindex")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let filaments = &scene.filaments;
    for child in obj.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "mesh" => {
                let verts: Vec<[f32; 3]> = child
                    .descendants()
                    .filter(|n| n.has_tag_name("vertex"))
                    .map(|n| {
                        let c = |k| n.attribute(k).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                        apply(t, [c("x"), c("y"), c("z")])
                    })
                    .collect();
                let volumes = if depth == 0 {
                    filaments.volumes.get(top_id)
                } else {
                    None
                };
                for (k, tri) in child
                    .descendants()
                    .filter(|n| n.has_tag_name("triangle"))
                    .enumerate()
                {
                    let i = |k| tri.attribute(k).and_then(|s| s.parse::<usize>().ok());
                    let (Some(a), Some(b2), Some(c)) = (i("v1"), i("v2"), i("v3")) else {
                        continue;
                    };
                    if a.max(b2).max(c) >= verts.len() {
                        continue;
                    }
                    let p1 = i("p1").unwrap_or(pindex);
                    let own = tri
                        .attribute("pid")
                        .or(pid)
                        .and_then(|g| scene.colors.get(&(part, g, p1)).copied());
                    let volume = volumes
                        .and_then(|v| v.iter().find(|(f, l, _)| (*f..=*l).contains(&k)))
                        .and_then(|&(_, _, e)| filaments.color(e));
                    b.triangle([verts[a], verts[b2], verts[c]], own.or(volume).or(paint));
                }
            }
            "components" => {
                for comp in child.children().filter(|n| n.has_tag_name("component")) {
                    let Some(oid) = comp.attribute("objectid") else {
                        continue;
                    };
                    // p:path: the object lives in another model part.
                    let target = comp
                        .attributes()
                        .find(|a| a.name() == "path")
                        .and_then(|a| scene.parts.get(&part_name(a.value())).copied())
                        .unwrap_or(part);
                    let own = if depth == 0 {
                        filaments
                            .parts
                            .get(&(top_id.to_string(), oid.to_string()))
                            .and_then(|&e| filaments.color(e))
                    } else {
                        None
                    };
                    let ct = compose(&parse_transform(comp.attribute("transform")), t);
                    add_object(
                        scene,
                        target,
                        oid,
                        &ct,
                        b,
                        (depth + 1, top_id),
                        own.or(paint),
                    );
                }
            }
            _ => {}
        }
    }
}

/// The files of a ZIP archive (stored or deflated), by name.
pub(crate) fn unzip(data: &[u8]) -> Result<HashMap<String, Vec<u8>>, String> {
    let u16le = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32le = |o: usize| {
        data.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let u64le = |o: usize| {
        data.get(o..o + 8)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
    };
    // End of central directory: the last "PK\x05\x06".
    let eocd = (0..data.len().saturating_sub(21))
        .rev()
        .take(70_000)
        .find(|&i| data[i..].starts_with(b"PK\x05\x06"))
        .ok_or(tr("3MF: не ZIP-архив"))?;
    let mut count = u16le(eocd + 10).ok_or("ZIP")? as usize;
    let mut p = u32le(eocd + 16).ok_or("ZIP")? as usize;
    // ZIP64 (written by .NET packaging, e.g. 3D Builder): the real count
    // and offset are in the ZIP64 record its locator points to.
    if (count == 0xFFFF || p == 0xFFFF_FFFF)
        && let Some(loc) = eocd.checked_sub(20)
        && u32le(loc) == Some(0x0706_4b50)
        && let Some(rec) = u64le(loc + 8).map(|o| o as usize)
        && u32le(rec) == Some(0x0606_4b50)
    {
        count = u64le(rec + 32).ok_or("ZIP")? as usize;
        p = u64le(rec + 48).ok_or("ZIP")? as usize;
    }
    let mut out = HashMap::new();
    for _ in 0..count {
        if u32le(p) != Some(0x0201_4b50) {
            break;
        }
        let method = u16le(p + 10).unwrap_or(0);
        let mut csize = u32le(p + 20).unwrap_or(0) as usize;
        let usize32 = u32le(p + 24).unwrap_or(0);
        let name_len = u16le(p + 28).unwrap_or(0) as usize;
        let extra = u16le(p + 30).unwrap_or(0) as usize;
        let comment = u16le(p + 32).unwrap_or(0) as usize;
        let mut local = u32le(p + 42).unwrap_or(0) as usize;
        let name = String::from_utf8_lossy(data.get(p + 46..p + 46 + name_len).unwrap_or(&[]))
            .into_owned();
        // ZIP64 extra field: 64-bit values for the fields saturated above,
        // in the order uncompressed size, compressed size, offset.
        let mut x = p + 46 + name_len;
        while x + 4 <= p + 46 + name_len + extra {
            let (id, len) = (u16le(x).unwrap_or(0), u16le(x + 2).unwrap_or(0) as usize);
            if id == 1 {
                let mut f = x + 4;
                if usize32 == 0xFFFF_FFFF {
                    f += 8;
                }
                if csize == 0xFFFF_FFFF {
                    csize = u64le(f).unwrap_or(0) as usize;
                    f += 8;
                }
                if local == 0xFFFF_FFFF {
                    local = u64le(f).unwrap_or(0) as usize;
                }
            }
            x += 4 + len;
        }
        p += 46 + name_len + extra + comment;
        // Local header: its own name / extra lengths precede the data.
        let (Some(ln), Some(le)) = (u16le(local + 26), u16le(local + 28)) else {
            continue;
        };
        let start = local + 30 + ln as usize + le as usize;
        let Some(raw) = start
            .checked_add(csize)
            .and_then(|end| data.get(start..end))
        else {
            continue;
        };
        let bytes = match method {
            0 => raw.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec(raw).map_err(|e| format!("ZIP: {e:?}"))?,
            _ => continue,
        };
        out.insert(name, bytes);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored (uncompressed) ZIP with the given files.
    pub(crate) fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let off = out.len() as u32;
            let crc = crc32fast::hash(data);
            let hdr = |sig: u32, central: bool| {
                let mut h = sig.to_le_bytes().to_vec();
                if central {
                    h.extend_from_slice(&20u16.to_le_bytes());
                }
                h.extend_from_slice(&20u16.to_le_bytes()); // version
                h.extend_from_slice(&0u16.to_le_bytes()); // flags
                h.extend_from_slice(&0u16.to_le_bytes()); // stored
                h.extend_from_slice(&0u32.to_le_bytes()); // time / date
                h.extend_from_slice(&crc.to_le_bytes());
                h.extend_from_slice(&(data.len() as u32).to_le_bytes());
                h.extend_from_slice(&(data.len() as u32).to_le_bytes());
                h.extend_from_slice(&(name.len() as u16).to_le_bytes());
                h.extend_from_slice(&0u16.to_le_bytes()); // extra
                if central {
                    h.extend_from_slice(&[0; 6]); // comment, disk, internal attrs
                    h.extend_from_slice(&0u32.to_le_bytes()); // ext attrs
                    h.extend_from_slice(&off.to_le_bytes());
                }
                h.extend_from_slice(name.as_bytes());
                h
            };
            out.extend(hdr(0x0403_4b50, false));
            out.extend_from_slice(data);
            central.extend(hdr(0x0201_4b50, true));
        }
        let cd = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn components_transforms_and_colours() {
        let model = br##"<?xml version="1.0"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <resources>
  <basematerials id="5"><base name="red" displaycolor="#FF0000"/></basematerials>
  <object id="1" type="model" pid="5" pindex="0"><mesh>
   <vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/></vertices>
   <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
  </mesh></object>
  <object id="2" type="model"><components>
   <component objectid="1"/>
   <component objectid="1" transform="1 0 0 0 1 0 0 0 1 100 0 0"/>
  </components></object>
 </resources>
 <build><item objectid="2" transform="1 0 0 0 1 0 0 0 1 0 0 5"/></build>
</model>"##;
        let data = zip(&[("3D/3dmodel.model", model)]);
        let m = parse(&data).unwrap();
        assert_eq!(m.triangles(), 2);
        assert_eq!(m.size()[0], 110.0); // two copies, 100 mm apart
        assert_eq!(m.min[1], 5.0); // z = 5 → y after the Z-up conversion
        assert_eq!(m.colors.unwrap()[0], [1.0, 0.0, 0.0, 1.0]);
    }

    /// The directory found through the ZIP64 record, as .NET writes it.
    #[test]
    fn zip64_directory() {
        let mut data = zip(&[("a.txt", b"hi")]);
        let eocd = data.len() - 22;
        let field = |o: usize| u64::from(u32::from_le_bytes(data[o..o + 4].try_into().unwrap()));
        let (cd_size, cd) = (field(eocd + 12), field(eocd + 16));
        let mut z64 = 0x0606_4b50u32.to_le_bytes().to_vec();
        z64.extend(44u64.to_le_bytes()); // size of the rest
        z64.extend([45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // versions, disks
        z64.extend(1u64.to_le_bytes());
        z64.extend(1u64.to_le_bytes());
        z64.extend(cd_size.to_le_bytes());
        z64.extend(cd.to_le_bytes());
        z64.extend(0x0706_4b50u32.to_le_bytes()); // locator
        z64.extend(0u32.to_le_bytes());
        z64.extend((eocd as u64).to_le_bytes());
        z64.extend(1u32.to_le_bytes());
        let mut tail = data.split_off(eocd);
        tail[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        data.extend(z64);
        data.extend(tail);
        assert_eq!(unzip(&data).unwrap()["a.txt"], b"hi");
    }

    const TRIANGLE: &str = r#"<mesh>
   <vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/></vertices>
   <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
  </mesh>"#;

    /// Bambu Studio / Orca: objects in their own model parts, filament
    /// colours per object and part, two plates.
    #[test]
    fn bambu_project() {
        let core = r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" requiredextensions="p">
 <resources>
  <object id="2" type="model"><components>
   <component p:path="/3D/Objects/a.model" objectid="1"/>
   <component p:path="/3D/Objects/a.model" objectid="3" transform="1 0 0 0 1 0 0 0 1 0 0 10"/>
  </components></object>
  <object id="4" type="model"><components>
   <component p:path="/3D/Objects/b.model" objectid="1"/>
  </components></object>
 </resources>
 <build>
  <item objectid="4" transform="1 0 0 0 1 0 0 0 1 300 0 0"/>
  <item objectid="2"/>
  <item objectid="4" transform="1 0 0 0 1 0 0 0 1 400 0 0"/>
 </build>
</model>"#;
        let part = |ids: &[u32]| {
            let objects: String = ids
                .iter()
                .map(|id| format!(r#"<object id="{id}" type="model">{TRIANGLE}</object>"#))
                .collect();
            format!(
                r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources>{objects}</resources><build/></model>"#
            )
        };
        let (a, b) = (part(&[1, 3]), part(&[1]));
        let settings = br#"<?xml version="1.0"?>
<config>
  <object id="2">
    <metadata key="name" value="Stand.STL"/>
    <metadata key="extruder" value="1"/>
    <part id="1" subtype="normal_part"><metadata key="extruder" value="0"/></part>
    <part id="3" subtype="normal_part"><metadata key="extruder" value="2"/></part>
  </object>
  <object id="4"><metadata key="extruder" value="3"/></object>
  <plate>
    <metadata key="plater_id" value="1"/>
    <metadata key="plater_name" value=""/>
    <model_instance><metadata key="object_id" value="2"/><metadata key="instance_id" value="0"/></model_instance>
    <model_instance><metadata key="object_id" value="4"/><metadata key="instance_id" value="1"/></model_instance>
  </plate>
  <plate>
    <metadata key="plater_id" value="2"/>
    <metadata key="plater_name" value="Mounts"/>
    <model_instance><metadata key="object_id" value="4"/><metadata key="instance_id" value="0"/></model_instance>
  </plate>
</config>"#;
        let project = br##"{ "filament_colour": ["#FF0000", "#00FF00", "#0000FF"], "nozzle_diameter": ["0.4"] }"##;
        let data = zip(&[
            ("3D/3dmodel.model", core.as_bytes()),
            ("3D/Objects/a.model", a.as_bytes()),
            ("3D/Objects/b.model", b.as_bytes()),
            ("Metadata/model_settings.config", settings),
            ("Metadata/project_settings.config", project),
        ]);
        let m = parse(&data).unwrap();
        assert_eq!(m.triangles(), 4);
        let names: Vec<&str> = m.plates.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Стол 1", "Стол 2 · Mounts"]);
        // Plate 1: object 2 (two parts) and the second copy of object 4.
        assert_eq!(m.plates[0].tris, 0..3);
        assert_eq!(m.plates[1].tris, 3..4);
        let objects: Vec<(&str, std::ops::Range<usize>)> = m
            .objects
            .iter()
            .map(|o| (o.name.as_str(), o.tris.clone()))
            .collect();
        assert_eq!(
            objects,
            [("Stand", 0..2), ("Объект 2", 2..3), ("Объект 3", 3..4)]
        );
        // Only the second object, painted green.
        let one = m.pick(&[(2..3, Some([0.0, 1.0, 0.0, 1.0]))]);
        assert_eq!(one.triangles(), 1);
        assert_eq!(one.colors.as_ref().unwrap()[0], [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(m.pick(&[]).triangles(), 0);
        let colors = m.colors.as_ref().unwrap();
        let red = [1.0, 0.0, 0.0, 1.0];
        let green = [0.0, 1.0, 0.0, 1.0];
        let blue = [0.0, 0.0, 1.0, 1.0];
        assert_eq!(colors[0], red); // part 1: as its object, filament 1
        assert_eq!(colors[3], green); // part 3: filament 2
        assert_eq!(colors[6], blue); // object 4: filament 3
        assert_eq!(m.part(m.plates[1].tris.clone()).min[0], 300.0);
        assert_eq!(m.part(m.plates[0].tris.clone()).max[0], 410.0);
    }

    /// PrusaSlicer: volumes are runs of one mesh's triangles.
    #[test]
    fn prusa_volumes() {
        let model = r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <resources><object id="1" type="model"><mesh>
   <vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/></vertices>
   <triangles><triangle v1="0" v2="1" v3="2"/><triangle v1="0" v2="2" v3="1"/></triangles>
 </mesh></object></resources>
 <build><item objectid="1"/></build>
</model>"#;
        let config = br#"<?xml version="1.0"?>
<config><object id="1" instances_count="1">
  <metadata type="object" key="extruder" value="1"/>
  <volume firstid="0" lastid="0"><metadata type="volume" key="extruder" value="1"/></volume>
  <volume firstid="1" lastid="1"><metadata type="volume" key="extruder" value="2"/></volume>
</object></config>"#;
        let ini = b"; generated by PrusaSlicer\n; extruder_colour = \"\";\"#00FF00\"\n; filament_colour = \"#FF0000\";\"#0000FF\"\n";
        let data = zip(&[
            ("3D/3dmodel.model", model.as_bytes()),
            ("Metadata/Slic3r_PE_model.config", config),
            ("Metadata/Slic3r_PE.config", ini),
        ]);
        let m = parse(&data).unwrap();
        let colors = m.colors.unwrap();
        assert_eq!(colors[0], [1.0, 0.0, 0.0, 1.0]); // no extruder colour: filament
        assert_eq!(colors[3], [0.0, 1.0, 0.0, 1.0]);
        assert!(m.plates.is_empty());
    }
}
