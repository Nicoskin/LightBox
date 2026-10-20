//! Build script: compiles HLSL shaders to bytecode with `fxc.exe` and embeds the
//! application manifest (per-monitor v2 DPI awareness etc.).
//!
//! Shaders are compiled ahead of time so that nothing is compiled at startup.
//! If `fxc.exe` cannot be found (no Windows SDK), the committed bytecode in
//! `shaders/compiled/` is used instead. Refresh it with `shaders/compile.ps1`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

/// (source file, entry point, profile, output name)
const SHADERS: &[(&str, &str, &str, &str)] = &[
    ("shaders/image.hlsl", "VSMain", "vs_5_0", "image_vs.cso"),
    ("shaders/image.hlsl", "PSMain", "ps_5_0", "image_ps.cso"),
    (
        "shaders/image.hlsl",
        "PSDiff",
        "ps_5_0",
        "image_diff_ps.cso",
    ),
    ("shaders/model.hlsl", "VSMain", "vs_5_0", "model_vs.cso"),
    ("shaders/model.hlsl", "PSMain", "ps_5_0", "model_ps.cso"),
    (
        "shaders/model.hlsl",
        "VSPath",
        "vs_5_0",
        "model_path_vs.cso",
    ),
];

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    println!("cargo:rerun-if-changed=shaders");
    println!("cargo:rerun-if-changed=lightbox.manifest");
    println!("cargo:rerun-if-changed=lightbox.rc");
    println!("cargo:rerun-if-changed=assets/lightbox.ico");
    println!("cargo:rerun-if-env-changed=FXC");

    compile_shaders(&manifest_dir, &out_dir);

    // Embed the manifest into the executable (no extra resource compiler needed).
    let manifest = manifest_dir.join("lightbox.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );

    compile_resources(&manifest_dir, &out_dir);
    build_stamp(&manifest_dir);

    // Graphics DLLs are loaded lazily on the GPU-init thread instead of by the
    // loader before `main`, so their load time overlaps with decoding.
    for dll in DELAY_LOAD {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bins=delayimp.lib");
}

const DELAY_LOAD: &[&str] = &["d3d11.dll", "dxgi.dll", "dcomp.dll"];

fn compile_shaders(manifest_dir: &Path, out_dir: &Path) {
    let fxc = find_fxc();
    for (src, entry, profile, out_name) in SHADERS {
        let out = out_dir.join(out_name);
        let fallback = manifest_dir.join("shaders/compiled").join(out_name);
        match &fxc {
            Some(fxc) => {
                let status = Command::new(fxc)
                    .args(["/nologo", "/O3", "/Qstrip_reflect", "/Qstrip_debug"])
                    .arg("/E")
                    .arg(entry)
                    .arg("/T")
                    .arg(profile)
                    .arg("/Fo")
                    .arg(&out)
                    .arg(manifest_dir.join(src))
                    .status()
                    .expect("failed to run fxc.exe");
                assert!(status.success(), "fxc failed for {src}:{entry}");
            }
            None => {
                println!(
                    "cargo:warning=fxc.exe not found, using precompiled {}",
                    fallback.display()
                );
                fs::copy(&fallback, &out).expect("precompiled shader missing");
            }
        }
    }
}

/// Locates `fxc.exe`: `FXC` env var, then the newest Windows 10/11 SDK.
fn find_fxc() -> Option<PathBuf> {
    if let Ok(p) = env::var("FXC") {
        return Some(PathBuf::from(p));
    }
    find_sdk_tool("fxc.exe")
}

/// Compiles `lightbox.rc` (the icon) with the SDK's `rc.exe` and links the
/// result. Without a Windows SDK the exe simply has no icon.
/// Name shown by Windows for the program (must match `APP_NAME` in main.rs).
const PRODUCT: &str = "Lightbox";

fn compile_resources(manifest_dir: &Path, out_dir: &Path) {
    let Some(rc) = find_sdk_tool("rc.exe") else {
        println!("cargo:warning=rc.exe not found, building without an icon");
        return;
    };
    // lightbox.rc (icon) plus a version block generated from Cargo.toml:
    // Explorer shows its FileDescription in "Open with" and Task Manager.
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts = version
        .split('.')
        .map(|p| p.parse::<u16>().unwrap_or(0))
        .chain(std::iter::repeat(0));
    let (a, b, c) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    // The generated script lives in OUT_DIR: the icon needs an absolute path.
    let icon = manifest_dir
        .join("assets")
        .join("lightbox.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");
    let base = fs::read_to_string(manifest_dir.join("lightbox.rc"))
        .expect("lightbox.rc")
        .replace("assets/lightbox.ico", &icon);
    let rc_text = format!(
        r#"{base}
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "041904B0"
    BEGIN
      VALUE "FileDescription", "{PRODUCT}"
      VALUE "ProductName", "{PRODUCT}"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "InternalName", "lightbox"
      VALUE "OriginalFilename", "lightbox.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x419, 1200
  END
END
"#
    );
    let rc_file = out_dir.join("lightbox.rc");
    fs::write(&rc_file, rc_text).expect("write rc");
    let res = out_dir.join("lightbox.res");
    let status = Command::new(rc)
        .current_dir(manifest_dir)
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(&rc_file)
        .status()
        .expect("failed to run rc.exe");
    assert!(status.success(), "rc.exe failed");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// Newest Windows 10/11 SDK copy of an x64 tool (fxc.exe, rc.exe).
fn find_sdk_tool(name: &str) -> Option<PathBuf> {
    let kits = env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let bin = Path::new(&kits).join(r"Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = fs::read_dir(&bin)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("x64").join(name).is_file())
        .collect();
    versions.sort();
    versions.pop().map(|p| p.join("x64").join(name))
}

/// `LIGHTBOX_BUILD`: "485c787 от 19.10.2026" (commit and its date) for the
/// settings card, so that it is clear which build is running.
fn build_stamp(dir: &Path) {
    let git = dir.join(".git");
    println!("cargo:rerun-if-changed={}", git.join("HEAD").display());
    println!(
        "cargo:rerun-if-changed={}",
        git.join("refs").join("heads").display()
    );
    let stamp = Command::new("git")
        .args(["log", "-1", "--format=%h от %cd", "--date=format:%d.%m.%Y"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=LIGHTBOX_BUILD={stamp}");
}
