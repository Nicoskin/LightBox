//! User settings, persisted in `%APPDATA%\Lightbox\settings.toml`.
//!
//! Missing or unknown keys fall back to defaults, so old files keep working
//! as options are added.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpenMode {
    /// Over the dimmed desktop (Picasa style).
    #[default]
    Fullscreen,
    /// In a window sized to the photo.
    Windowed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThumbSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl ThumbSize {
    /// Filmstrip height in 96-DPI pixels.
    pub fn strip_height(self) -> f64 {
        match self {
            ThumbSize::Small => 60.0,
            ThumbSize::Medium => 78.0,
            ThumbSize::Large => 108.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Wheel {
    /// Zoom at the cursor (Ctrl + wheel browses).
    #[default]
    Zoom,
    /// Browse files (Ctrl + wheel zooms).
    Browse,
}

/// What shows through transparent areas of an image (when enabled).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlphaBg {
    /// Grey / white checkerboard.
    #[default]
    Checker,
    White,
    Gray,
    Black,
}

impl AlphaBg {
    pub const ALL: [AlphaBg; 4] = [
        AlphaBg::Checker,
        AlphaBg::White,
        AlphaBg::Gray,
        AlphaBg::Black,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AlphaBg::Checker => "Шахматка",
            AlphaBg::White => "Белый",
            AlphaBg::Gray => "Серый",
            AlphaBg::Black => "Чёрный",
        }
    }

    /// Solid colour as sRGB grey level (`None` for the checkerboard).
    pub fn gray(self) -> Option<f32> {
        match self {
            AlphaBg::Checker => None,
            AlphaBg::White => Some(1.0),
            AlphaBg::Gray => Some(0.5),
            AlphaBg::Black => Some(0.0),
        }
    }
}

/// When a rotation made on screen is written to the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SaveMode {
    /// When leaving the photo (next / previous / close).
    #[default]
    Auto,
    /// Ask when leaving the photo.
    Ask,
    /// Only on Ctrl+S / the Save button.
    Manual,
}

/// What to do with a JPEG whose size is not a multiple of the MCU, where a
/// lossless rotation is not possible exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImperfectJpeg {
    /// Crop the partial block at the edge (≤ 15 px).
    Trim,
    /// Write only the EXIF Orientation tag.
    ExifOnly,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub open_mode: OpenMode,
    pub filmstrip: bool,
    pub thumb_size: ThumbSize,
    /// Show a background behind transparent areas, and which one.
    pub checker: bool,
    pub alpha_bg: AlphaBg,
    pub smoothing: bool,
    pub wheel: Wheel,
    /// Zoom, mode and filmstrip animations.
    pub animations: bool,
    /// Slide / fade when stepping to another photo.
    pub turn_animation: bool,
    /// Backdrop darkness in fullscreen mode, percent.
    pub dim_percent: u8,
    pub save_mode: SaveMode,
    /// Keep the file's modification date when writing a rotation.
    pub keep_file_dates: bool,
    /// Remembered answer for non-MCU-aligned JPEGs (`None` = ask).
    pub imperfect_jpeg: Option<ImperfectJpeg>,
    /// JPEG quality for "Save as".
    pub jpeg_quality: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            open_mode: OpenMode::Fullscreen,
            filmstrip: true,
            thumb_size: ThumbSize::Medium,
            checker: true,
            alpha_bg: AlphaBg::Checker,
            smoothing: true,
            wheel: Wheel::Zoom,
            animations: true,
            turn_animation: true,
            dim_percent: 80,
            save_mode: SaveMode::Auto,
            keep_file_dates: true,
            imperfect_jpeg: None,
            jpeg_quality: 92,
        }
    }
}

fn path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join("Lightbox")
            .join("settings.toml"),
    )
}

impl Settings {
    /// Reads the settings file; defaults if it is missing or unreadable.
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| Self::parse(&s))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        match toml::from_str::<Settings>(text) {
            Ok(mut s) => {
                s.dim_percent = s.dim_percent.clamp(0, 100);
                s.jpeg_quality = s.jpeg_quality.clamp(1, 100);
                s
            }
            Err(e) => {
                tracing::warn!("settings.toml ignored: {e}");
                Self::default()
            }
        }
    }

    /// Writes the settings (via a temporary file, so a crash never leaves a
    /// half-written file behind).
    pub fn save(&self) {
        let Some(p) = path() else {
            return;
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
            let tmp = p.with_extension("toml.tmp");
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &p)
        })();
        if let Err(e) = result {
            tracing::warn!("cannot save settings: {e}");
        }
    }

    pub fn dim(&self) -> f32 {
        f32::from(self.dim_percent) / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = Settings {
            open_mode: OpenMode::Windowed,
            filmstrip: false,
            thumb_size: ThumbSize::Large,
            imperfect_jpeg: Some(ImperfectJpeg::Trim),
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(Settings::parse(&text), s);
    }

    #[test]
    fn partial_and_broken_files_fall_back() {
        let s = Settings::parse("filmstrip = false\nunknown_key = 5\n");
        assert!(!s.filmstrip);
        assert_eq!(s.open_mode, OpenMode::Fullscreen);
        assert_eq!(Settings::parse("this is = = not toml"), Settings::default());
        assert_eq!(Settings::parse("dim_percent = 250").dim_percent, 100);
    }
}
