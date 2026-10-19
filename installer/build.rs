//! Embeds the viewer built by `tools/release.ps1` (LIGHTBOX_EXE) and gives
//! the installer the viewer's icon, manifest and version.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    let exe = env::var("LIGHTBOX_EXE").unwrap_or_else(|_| {
        panic!("set LIGHTBOX_EXE to the release lightbox.exe (see tools/release.ps1)")
    });
    println!("cargo:rerun-if-env-changed=LIGHTBOX_EXE");
    println!("cargo:rerun-if-env-changed=LIGHTBOX_VERSION");
    println!("cargo:rerun-if-changed={exe}");
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    let manifest = root.join("lightbox.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    resources(&root, &out);
}

fn resources(root: &Path, out: &Path) {
    let Some(rc) = find_sdk_tool("rc.exe") else {
        println!("cargo:warning=rc.exe not found: installer without icon");
        return;
    };
    let version = env::var("LIGHTBOX_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let nums: Vec<u16> = version
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .chain(std::iter::repeat(0))
        .take(3)
        .collect();
    let icon = root
        .join("assets")
        .join("lightbox.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");
    let text = format!(
        r#"1 ICON "{icon}"
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
      VALUE "FileDescription", "Установка Lightbox"
      VALUE "ProductName", "Lightbox"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x419, 1200
  END
END
"#,
        a = nums[0],
        b = nums[1],
        c = nums[2],
    );
    let rc_file = out.join("setup.rc");
    // rc.exe reads UTF-16 with a BOM reliably (Cyrillic strings).
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    fs::write(&rc_file, bytes).unwrap();
    let res = out.join("setup.res");
    let ok = Command::new(rc)
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(&rc_file)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "rc.exe failed");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

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
