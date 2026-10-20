# Lightbox

[Русский](README.md) · **English**

A fast image and 3D model viewer for Windows 10/11 in the spirit of Picasa Photo Viewer: a photo opens instantly over the dimmed desktop, arrow keys browse the folder, Esc closes it. Written in Rust on bare Win32 + Direct3D 11. The interface follows the Windows language (English or Russian) and can be switched in Settings → Interface.

![Lightbox: a photo and the info panel](docs/viewer.png)

## Features

- **Instant opening.** The embedded thumbnail or a reduced copy shows first, full resolution loads in the background. Neighbouring photos are decoded ahead, so browsing has no delays.
- **Two modes, as in Picasa.** Fullscreen over the dimmed desktop, or a window sized to the photo. A click on the free area switches, Esc closes.
- **Zoom from 1 % to 50 000 %** towards the cursor. From 800 % a pixel grid appears, and the title shows the coordinates and colour of the pixel under the cursor.
- **Lossless rotation saved to the file.** JPEG without re-encoding, like jpegtran; PNG and APNG, GIF (all frames), BMP, QOI, TGA and lossless WebP are re-saved at their bit depth (PNG keeps all metadata, including ComfyUI prompts); TIFF gets the orientation tag. Ctrl+Z undoes the save. Lossy WebP, AVIF and HEIC are rotated for viewing only.
- **Editing:** crop (X) with aspect ratios (lossless for JPEG), exposure / contrast / saturation / warmth (A), straightening by any angle with an automatic crop. Everything can be undone with Ctrl+Z.
- **Colour.** ICC profiles (Adobe RGB, Display P3, CMYK) are converted to sRGB; HDR and EXR are tone-mapped.
- **Huge images** (30 000 × 20 000 and more) are drawn in tiles.
- **SVG stays vector at any zoom:** the visible part is redrawn at screen resolution.
- **Animation** in GIF, WebP and APNG: Space pauses, `,` and `.` step frames.
- **Video** (MP4, MOV, WMV, AVI, MKV/WebM with codecs): Space pauses, a seek bar, M mutes.
- **Documents on Space:** PDF and text files in Lightbox itself; DOCX, XLSX and other types through the Windows preview handlers.
- **3D models** STL, OBJ (+MTL) and 3MF with colours, including Bambu Studio, OrcaSlicer and PrusaSlicer projects: filament colours and build plates (↑/↓). Drag turns, the wheel zooms. Models are browsed together with photos and appear in the filmstrip.
- **Model tools:** a ruler between two points (D), wireframe (W), a section by height (P, ↑/↓ move it), lay flat on a face (B).
- **G-code** (.gcode, .gcode.3mf): the toolpath coloured by line type, layers with ↑/↓, print time and filament use.
- **Inspector:** histogram, single R/G/B/A channels, an info panel (EXIF, camera, GPS with a map, ICC). For PNGs from ComfyUI and A1111 it shows the prompt and generation parameters with copy buttons.
- **Ratings and tags** (Ctrl+1…5, Ctrl+K) written the Windows way, so Explorer and Lightroom see them; a filter by rating.
- **Batch rename and convert** of the selected photos (Ctrl+B).
- **Compare two photos** side by side with synced zoom, in one place by turns, or as a difference map.
- **Slideshow, filmstrip, folder watching, drag & drop** into and out of the window.
- **File handling:** delete to the Recycle Bin, copy the image or its path, quick folders F1–F9, open in an editor, show in Explorer.
- **Space in Explorer** previews the selected file (like Quick Look on macOS); a window can be pinned on top of the others.
- **Every shortcut can be reassigned** in Settings → Keys.

| 3D models | Pixel grid and the colour under the cursor |
|---|---|
| ![A 3MF model](docs/3d.png) | ![Pixel grid at 1800 %](docs/pixels.png) |

## Formats

| | |
|---|---|
| Photos | JPEG, PNG, APNG, GIF, WebP, BMP, ICO, TIFF, TGA, QOI, AVIF, HEIC/HEIF¹, JPEG XR |
| Graphics | SVG/SVGZ, PSD (composite), DDS, PBM/PGM/PPM/PAM |
| HDR | Radiance HDR, OpenEXR |
| RAW | DNG, CR2, CR3, NEF, ARW, ORF, RW2, RAF, PEF, SRW and more (embedded preview) |
| 3D | STL (ASCII and binary), OBJ + MTL, 3MF (and slicer projects), G-code |
| Video | MP4, MOV, M4V, WMV, AVI, MKV, WebM (with the codecs installed in Windows) |
| Documents | PDF, TXT and code; others through Windows preview handlers |

¹ HEIC opens with the Windows codec ("HEIF Image Extensions" from the Microsoft Store).

## Installation

**[Download the installer](https://github.com/Nicoskin/LightBox/releases/latest/download/lightbox-setup.exe)** (`lightbox-setup.exe`, latest version) and run it. All versions are on the [Releases](https://github.com/Nicoskin/LightBox/releases) page.

- Installs for the current user only; no administrator rights needed.
- The installer can make Lightbox the default for all its formats at once: Windows shows the Default apps page, where one click confirms it.
- The installer is not signed yet, so SmartScreen may warn about an unknown publisher: "More info" → "Run anyway".
- Remove it in Settings → Apps → Installed apps.

**Portable version:** [lightbox-portable.zip](https://github.com/Nicoskin/LightBox/releases/latest/download/lightbox-portable.zip). Unpack it anywhere, for example onto a USB stick. Settings and caches live in the `data` folder next to `lightbox.exe`. It uses no tray, start-up entry or registry, so Space in Explorer and file associations are only in the installed version.

**Updates.** Lightbox checks for a new version once a day; this can be turned off in Settings → System. The new version is downloaded from GitHub, its SHA-256 is verified, and it installs over the old one keeping the settings.

## Controls

| Keys | Action |
|---|---|
| ← → / PgUp PgDn / wheel¹ | previous / next photo |
| Home / End | first / last |
| wheel, + / − | zoom towards the cursor |
| 0 / 1 / Z | fit / 100 % / toggle |
| Enter / F11 / click on the background | fullscreen / window |
| R / L | rotate clockwise / counter-clockwise (saved to the file) |
| M / Shift+M | flip horizontally / vertically |
| [ / ] | tilt by 1° (Shift — 0.1°) |
| X / A | crop / adjustments |
| Ctrl+S / Ctrl+Shift+S / Ctrl+Z | save / save as / undo the save |
| Ctrl+C / Ctrl+Shift+C | copy the image / path |
| Ctrl+1…5 / Ctrl+0 / Ctrl+F | rating / clear rating / filter by rating |
| Ctrl+K / Ctrl+B | tags / batch rename and convert |
| Delete | to the Recycle Bin |
| E | open in an editor |
| F1–F4, F6–F9 | quick folders |
| I / H / G | info / histogram / pixel grid |
| Shift+1…4 / Shift+0 | channels R, G, B, A / all channels |
| T / N / F | transparency background / smoothing / filmstrip |
| C / V / hold Space | compare / layout / show A |
| S or F5 | slideshow |
| Space / , . | pause / previous, next frame |
| Esc | close |

¹ By default the wheel zooms; browsing with the wheel is a setting.

**3D models:** drag turns, Shift or Ctrl + drag moves, the wheel zooms, 0 or a double click resets the view. D — ruler, W — wireframe, P — section, B — lay flat; ↑/↓ switch plates, G-code layers or move the section.

## Tray and Space in Explorer

In Settings → System Lightbox can stay in the tray after closing and start with Windows. Photos then open even faster, and **Space preview** works: select a file in Explorer and press Space — a window sized to the photo opens. Space again or Esc closes it; the 📌 button keeps the photo in its own window on top of the others.

Settings are kept in `%APPDATA%\Lightbox\settings.toml`.

## Building from source

You need Rust (stable, `x86_64-pc-windows-msvc`), Visual Studio Build Tools with the Windows SDK, CMake and NASM (for SIMD in libjpeg-turbo).

```powershell
cargo build --release                            # target\release\lightbox.exe
cargo run --release --example gen_testdata       # test images in testdata\
cargo test --release
pwsh tools/release.ps1 -Repo Nicoskin/LightBox   # installer and portable zip in dist\
```

Shaders are compiled with `fxc.exe` from the Windows SDK during the build; without the SDK the precompiled ones in `shaders/compiled` are used.

Command line: `lightbox <file>`; without arguments — the start screen; `--register` / `--unregister` — "Open with" (HKCU); `--background` — start in the tray; `--bench <file>` — the time of each opening stage.

## License

[MIT](LICENSE)
