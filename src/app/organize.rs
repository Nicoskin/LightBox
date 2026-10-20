//! Tags (Ctrl+K) and batch work on the photos picked in the filmstrip, or
//! the whole folder (Ctrl+B): rename by a template, convert to JPEG / PNG /
//! WebP and shrink. Both ask in a small native form; batches run on a
//! worker thread and report progress in the badge.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use anyhow::Result;

use super::{App, WM_APP_BATCH, post};
use crate::i18n::tr;
use crate::t;
use crate::win::form::{self, Field};
use crate::{cache, decode, encode, rating};

/// Progress of a running batch.
pub(super) enum BatchMsg {
    Progress(usize, usize),
    Done { ok: usize, failed: Vec<String> },
}

pub(super) type BatchRx = Receiver<BatchMsg>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Same,
    Jpeg,
    Png,
    WebP,
}

struct Job {
    files: Vec<PathBuf>,
    rename: bool,
    template: String,
    format: Format,
    long_side: u32,
    quality: u8,
    folder: PathBuf,
    keep_dates: bool,
}

impl App {
    /// Ctrl+K: edit the current photo's tags.
    pub(super) fn edit_tags(&mut self) -> Result<()> {
        if self.current.as_os_str().is_empty() || self.model_mode() || self.is_doc() {
            return Ok(());
        }
        let path = self.current.clone();
        let mut fields = [Field::Text {
            label: tr("Метки (через запятую)").into(),
            value: rating::keywords(&path).join(", "),
        }];
        let note = t!(
            "{} — метки видны в Проводнике и в Lightroom",
            Self::file_name(&path)
        );
        if !form::show(self.hwnd, tr("Метки"), &note, &mut fields, tr("Сохранить")) {
            return Ok(());
        }
        let Field::Text { value, .. } = &fields[0] else {
            return Ok(());
        };
        let words: Vec<String> = value
            .split([',', ';'])
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect();
        match rating::set_keywords(&path, &words, self.settings.keep_file_dates) {
            Ok(()) if words.is_empty() => self.notice(tr("Метки сняты")),
            Ok(()) => self.notice(&t!("Метки: {}", words.join(", "))),
            Err(e) => self.notice(&t!("Метки не записаны: {}", e)),
        }
        self.render()
    }

    /// Ctrl+B: rename / convert the picked photos (or the whole folder).
    pub(super) fn batch_dialog(&mut self) -> Result<()> {
        if self.batch_rx.is_some() {
            self.notice(tr("Пакетная обработка уже идёт"));
            return self.render();
        }
        let files: Vec<PathBuf> = if self.selected.is_empty() {
            self.files.clone()
        } else {
            self.files
                .iter()
                .filter(|p| self.selected.contains(&cache::key(p)))
                .cloned()
                .collect()
        };
        let files: Vec<PathBuf> = files
            .into_iter()
            .filter(|p| !crate::model::is_model(p) && !decode::is_document(p))
            .collect();
        if files.is_empty() || self.preview() {
            return Ok(());
        }
        let folder = self
            .current
            .parent()
            .map_or_else(PathBuf::new, |d| d.join("Lightbox"));
        let mut fields = [
            Field::Choice {
                label: tr("Действие").into(),
                options: vec![
                    tr("Переименовать").into(),
                    tr("Конвертировать в папку").into(),
                ],
                selected: 0,
            },
            Field::Text {
                label: tr("Имя файла").into(),
                value: tr("{имя}").into(),
            },
            Field::Choice {
                label: tr("Формат").into(),
                options: vec![
                    tr("Как у исходного").into(),
                    "JPEG".into(),
                    "PNG".into(),
                    tr("WebP (без потерь)").into(),
                ],
                selected: 1,
            },
            Field::Number {
                label: tr("Длинная сторона, px (0 — как есть)").into(),
                value: 0,
            },
            Field::Number {
                label: tr("Качество JPEG").into(),
                value: u32::from(self.settings.jpeg_quality),
            },
            Field::Folder {
                label: tr("Папка для результата").into(),
                value: folder,
            },
        ];
        let which = if self.selected.is_empty() {
            t!("Вся папка: {} фото", files.len())
        } else {
            t!("Выбрано в ленте: {} фото", files.len())
        };
        let note = t!(
            "{}. В имени: {{имя}} — прежнее имя, {{n}} — номер, {{дата}} — дата съёмки.",
            which
        );
        if !form::show(
            self.hwnd,
            tr("Пакетная обработка"),
            &note,
            &mut fields,
            tr("Начать"),
        ) {
            return Ok(());
        }
        let choice = |f: &Field| match f {
            Field::Choice { selected, .. } => *selected,
            _ => 0,
        };
        let text = |f: &Field| match f {
            Field::Text { value, .. } => value.clone(),
            _ => String::new(),
        };
        let number = |f: &Field| match f {
            Field::Number { value, .. } => *value,
            _ => 0,
        };
        let job = Job {
            files,
            rename: choice(&fields[0]) == 0,
            template: text(&fields[1]),
            format: match choice(&fields[2]) {
                1 => Format::Jpeg,
                2 => Format::Png,
                3 => Format::WebP,
                _ => Format::Same,
            },
            long_side: number(&fields[3]),
            quality: number(&fields[4]).clamp(1, 100) as u8,
            folder: match &fields[5] {
                Field::Folder { value, .. } => value.clone(),
                _ => PathBuf::new(),
            },
            keep_dates: self.settings.keep_file_dates,
        };
        if job.rename
            && !job.template.contains("{n}")
            && job.files.len() > 1
            && !job.template.contains("{имя}")
            && !job.template.contains("{name}")
        {
            self.notice(&t!("Для нескольких файлов в имени нужен {{n}} или {{имя}}"));
            return self.render();
        }
        let (tx, rx) = channel();
        let hwnd = self.hwnd.0 as isize;
        let spawned = thread::Builder::new().name("batch".into()).spawn(move || {
            let n = job.files.len();
            let mut ok = 0;
            let mut failed = Vec::new();
            for (i, p) in job.files.iter().enumerate() {
                match run_one(&job, p, i + 1) {
                    Ok(()) => ok += 1,
                    Err(e) => failed.push(format!(
                        "{}: {e}",
                        p.file_name().unwrap_or_default().to_string_lossy()
                    )),
                }
                let _ = tx.send(BatchMsg::Progress(i + 1, n));
                post(hwnd, WM_APP_BATCH);
            }
            let _ = tx.send(BatchMsg::Done { ok, failed });
            post(hwnd, WM_APP_BATCH);
        });
        if spawned.is_ok() {
            self.batch_rx = Some(rx);
            self.notice(tr("Обработка…"));
        }
        self.render()
    }

    pub(super) fn on_batch(&mut self) -> Result<()> {
        let mut done = None;
        while let Some(m) = self.batch_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match m {
                BatchMsg::Progress(i, n) => self.notice(&t!("Обработано {} из {}", i, n)),
                BatchMsg::Done { ok, failed } => done = Some((ok, failed)),
            }
        }
        if let Some((ok, failed)) = done {
            self.batch_rx = None;
            self.clear_selection()?;
            if failed.is_empty() {
                self.notice(&t!("Готово: {} файлов", ok));
            } else {
                tracing::warn!("batch failures: {failed:?}");
                self.notice(&t!(
                    "Готово: {}, с ошибками: {} ({})",
                    ok,
                    failed.len(),
                    failed[0]
                ));
            }
            self.rescan_folder();
        }
        self.render()
    }
}

/// The new file name (without extension) for `path` as item `n`.
fn file_stem(template: &str, path: &Path, n: usize, total_width: usize) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = template
        .replace("{имя}", &stem)
        .replace("{name}", &stem)
        .replace("{n}", &format!("{n:0total_width$}"))
        .replace("{дата}", &taken(path))
        .replace("{date}", &taken(path));
    // Characters Windows does not allow in names.
    name.chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// "2026-07-14": the EXIF capture date, or the file's modification date.
fn taken(path: &Path) -> String {
    let exif = std::fs::File::open(path).ok().and_then(|f| {
        exif::Reader::new()
            .read_from_container(&mut std::io::BufReader::new(f))
            .ok()
    });
    if let Some(d) = exif.as_ref().and_then(|e| {
        e.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
            .map(|f| f.display_value().to_string())
    }) && d.len() >= 10
    {
        return d[..10].replace(':', "-");
    }
    let secs = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    civil_date(secs / 86400)
}

/// Days since 1970-01-01 → "YYYY-MM-DD" (proleptic Gregorian).
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A free name: `stem.ext`, else `stem (2).ext`, …
fn free_name(dir: &Path, stem: &str, ext: &str, original: &Path) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut k = 2;
    while p.exists() && !crate::library::same_path(&p, original) {
        p = dir.join(format!("{stem} ({k}).{ext}"));
        k += 1;
    }
    p
}

fn run_one(job: &Job, path: &Path, n: usize) -> Result<(), String> {
    let width = job.files.len().to_string().len().max(2);
    let stem = file_stem(&job.template, path, n, width);
    if stem.is_empty() {
        return Err(tr("пустое имя").into());
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    if job.rename {
        let dir = path.parent().unwrap_or(Path::new("."));
        let to = free_name(dir, &stem, &ext, path);
        if crate::library::same_path(&to, path) {
            return Ok(());
        }
        return std::fs::rename(path, &to).map_err(|e| e.to_string());
    }
    // Convert: full resolution, upright, optionally smaller.
    let img = decode::decode_file(path, None).map_err(|e| e.to_string())?;
    let rgba = img.to_straight_rgba8();
    let (mut px, mut w, mut h) = encode::orient_pixels(
        &rgba,
        img.width as usize,
        img.height as usize,
        4,
        img.orientation,
    );
    let long = w.max(h) as u32;
    if job.long_side > 0 && long > job.long_side {
        let k = f64::from(job.long_side) / f64::from(long);
        let (nw, nh) = (
            ((w as f64 * k).round() as u32).max(1),
            ((h as f64 * k).round() as u32).max(1),
        );
        let buf = image::RgbaImage::from_raw(w as u32, h as u32, px).ok_or(tr("размер"))?;
        px =
            image::imageops::resize(&buf, nw, nh, image::imageops::FilterType::Lanczos3).into_raw();
        (w, h) = (nw as usize, nh as usize);
    }
    let format = match job.format {
        Format::Same => match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "jpe" | "jfif" => Format::Jpeg,
            "webp" => Format::WebP,
            _ => Format::Png,
        },
        f => f,
    };
    let (data, ext) = match format {
        Format::Jpeg => (
            encode::encode_jpeg(&px, w, h, job.quality).map_err(|e| e.to_string())?,
            "jpg",
        ),
        Format::WebP => {
            let mut out = Vec::new();
            image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(w as u32, h as u32, px).ok_or(tr("размер"))?,
            )
            .write_to(
                &mut std::io::Cursor::new(&mut out),
                image::ImageFormat::WebP,
            )
            .map_err(|e| e.to_string())?;
            (out, "webp")
        }
        _ => (
            encode::encode_png(&px, w, h).map_err(|e| e.to_string())?,
            "png",
        ),
    };
    std::fs::create_dir_all(&job.folder).map_err(|e| e.to_string())?;
    let to = free_name(&job.folder, &stem, ext, Path::new(""));
    std::fs::write(&to, data).map_err(|e| e.to_string())?;
    if job.keep_dates
        && let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified())
    {
        let _ = std::fs::File::options()
            .write(true)
            .open(&to)
            .and_then(|f| f.set_modified(t));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_templates() {
        let p = Path::new("C:/x/IMG_0001.JPG");
        assert_eq!(file_stem("{имя}", p, 3, 2), "IMG_0001");
        assert_eq!(file_stem("Отпуск {n}", p, 3, 3), "Отпуск 003");
        assert_eq!(file_stem("a/b:{n}", p, 12, 2), "a_b_12");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_725), "2026-09-29");
        assert_eq!(civil_date(11_016), "2000-02-29");
    }
}
