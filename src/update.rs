//! Updates from GitHub Releases: the latest release's installer is
//! downloaded, its SHA-256 checked and run in update mode (it replaces the
//! program in place, keeping settings and file associations).
//!
//! The repository ("owner/name") is set when building a release:
//! `LIGHTBOX_UPDATE_REPO=owner/name cargo build --release`. Without it the
//! update check is off.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use windows::Win32::Networking::WinHttp::{
    URL_COMPONENTS, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    WinHttpCloseHandle, WinHttpConnect, WinHttpCrackUrl, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest,
};
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::core::{HSTRING, PCWSTR, w};

/// "owner/name" of the GitHub repository with the releases, if built in.
pub const REPO: Option<&str> = option_env!("LIGHTBOX_UPDATE_REPO");
/// This program's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A newer release than this program.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub setup_url: String,
    /// Lower-case hex SHA-256 of the installer, from the release.
    pub sha256: Option<String>,
    pub notes: String,
}

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a handle returned by WinHTTP, closed once.
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// GET `url` (following redirects); returns the status and the body.
pub fn http_get(url: &str) -> Result<(u32, Vec<u8>)> {
    let wide: Vec<u16> = url.encode_utf16().collect();
    let mut host = vec![0u16; 256];
    let mut path = vec![0u16; 4096];
    let mut parts = URL_COMPONENTS {
        dwStructSize: std::mem::size_of::<URL_COMPONENTS>() as u32,
        lpszHostName: windows::core::PWSTR(host.as_mut_ptr()),
        dwHostNameLength: host.len() as u32,
        lpszUrlPath: windows::core::PWSTR(path.as_mut_ptr()),
        dwUrlPathLength: path.len() as u32,
        ..Default::default()
    };
    // SAFETY: the component buffers are sized as declared; all handles are
    // closed by `Handle`; buffers passed to WinHTTP live for each call.
    unsafe {
        WinHttpCrackUrl(&wide, 0, &mut parts).context("URL")?;
        let host = String::from_utf16_lossy(&host[..parts.dwHostNameLength as usize]);
        let path = String::from_utf16_lossy(&path[..parts.dwUrlPathLength as usize]);
        let secure = parts.nScheme.0 == 2; // INTERNET_SCHEME_HTTPS
        let session = Handle(WinHttpOpen(
            w!("Lightbox-Updater"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            bail!("WinHttpOpen failed");
        }
        let connect = Handle(WinHttpConnect(
            session.0,
            &HSTRING::from(host.as_str()),
            parts.nPort,
            0,
        ));
        if connect.0.is_null() {
            bail!("cannot connect to {host}");
        }
        let flags = if secure {
            WINHTTP_FLAG_SECURE
        } else {
            WINHTTP_OPEN_REQUEST_FLAGS(0)
        };
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            &HSTRING::from(path.as_str()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        ));
        if request.0.is_null() {
            bail!("WinHttpOpenRequest failed");
        }
        // GitHub's API asks for a User-Agent (WinHttpOpen sets it) and
        // this Accept header.
        let headers: Vec<u16> = "Accept: application/vnd.github+json, application/octet-stream\r\n"
            .encode_utf16()
            .collect();
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0).context("send")?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).context("response")?;
        let mut status = 0u32;
        let mut len = 4u32;
        let mut index = 0u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(std::ptr::from_mut(&mut status).cast()),
            &mut len,
            &mut index,
        )
        .context("status")?;
        let mut body = Vec::new();
        loop {
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut avail).context("read")?;
            if avail == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr().cast(),
                avail,
                &mut read,
            )
            .context("read")?;
            body.truncate(start + read as usize);
            if body.len() > 512 * 1024 * 1024 {
                bail!("download too large");
            }
        }
        Ok((status, body))
    }
}

/// Lower-case hex SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> String {
    let mut out = [0u8; 32];
    // SAFETY: the pseudo-handle needs no opening; buffers are sized.
    let _ = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// `true` if version `a` ("1.2.10", "v1.3") is newer than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches(['v', 'V'])
            .split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (mut x, mut y) = (parse(a), parse(b));
    x.resize(3, 0);
    y.resize(3, 0);
    x > y
}

/// The latest release of `repo` if it is newer than this program.
pub fn check(repo: &str) -> Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let (status, body) = http_get(&url)?;
    if status == 404 {
        return Ok(None); // no release yet
    }
    if status != 200 {
        bail!("GitHub: HTTP {status}");
    }
    let json: serde_json::Value = serde_json::from_slice(&body).context("release JSON")?;
    parse_release(&json)
}

fn parse_release(json: &serde_json::Value) -> Result<Option<Release>> {
    let tag = json["tag_name"].as_str().unwrap_or_default();
    if !newer(tag, VERSION) {
        return Ok(None);
    }
    let assets = json["assets"].as_array().cloned().unwrap_or_default();
    let name = |a: &serde_json::Value| a["name"].as_str().unwrap_or_default().to_ascii_lowercase();
    let Some(setup) = assets
        .iter()
        .find(|a| name(a).ends_with(".exe") && name(a).contains("setup"))
    else {
        bail!("в выпуске {tag} нет установщика");
    };
    // GitHub publishes "digest": "sha256:…" per asset; else a .sha256 file.
    let mut sha = setup["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase);
    if sha.is_none()
        && let Some(sum) = assets
            .iter()
            .find(|a| name(a) == format!("{}.sha256", name(setup)))
        && let Some(url) = sum["browser_download_url"].as_str()
    {
        let (_, text) = http_get(url)?;
        sha = String::from_utf8_lossy(&text)
            .split_whitespace()
            .next()
            .map(str::to_ascii_lowercase);
    }
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        setup_url: setup["browser_download_url"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        sha256: sha,
        notes: json["body"].as_str().unwrap_or_default().to_string(),
    }))
}

/// Downloads the installer to the temp folder and checks its SHA-256.
pub fn download(release: &Release) -> Result<PathBuf> {
    let (status, data) = http_get(&release.setup_url)?;
    if status != 200 {
        bail!("загрузка: HTTP {status}");
    }
    let Some(expected) = &release.sha256 else {
        bail!("у выпуска нет контрольной суммы — обновление небезопасно");
    };
    let got = sha256(&data);
    if &got != expected {
        bail!("контрольная сумма не совпала");
    }
    let path = std::env::temp_dir().join(format!("lightbox-setup-{}.exe", release.version));
    std::fs::write(&path, data)?;
    Ok(path)
}

/// Starts the downloaded installer in update mode (it closes this program,
/// replaces it and starts it again).
pub fn run_installer(setup: &Path, reopen: Option<&Path>) -> Result<()> {
    let mut cmd = std::process::Command::new(setup);
    cmd.arg("/update");
    if let Some(p) = reopen {
        cmd.arg(p);
    }
    cmd.spawn().context("installer")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_hash() {
        assert!(newer("v0.2.0", "0.1.9"));
        assert!(newer("1.10.0", "1.9.3"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("garbage", "0.1.0"));
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn release_json() {
        let json = serde_json::json!({
            "tag_name": "v99.0.0",
            "body": "notes",
            "assets": [
                {"name": "lightbox-99.0.0-setup.exe",
                 "browser_download_url": "https://example.invalid/s.exe",
                 "digest": "sha256:ABCDEF"},
                {"name": "lightbox.zip", "browser_download_url": "x"}
            ]
        });
        let r = parse_release(&json).unwrap().unwrap();
        assert_eq!(r.version, "99.0.0");
        assert_eq!(r.sha256.as_deref(), Some("abcdef"));
        let old = serde_json::json!({"tag_name": "v0.0.1", "assets": []});
        assert!(parse_release(&old).unwrap().is_none());
    }
}

#[cfg(test)]
mod network {
    /// Real request to GitHub (needs the network): `--ignored`.
    #[test]
    #[ignore = "network"]
    fn github_latest_release() {
        let (status, body) =
            super::http_get("https://api.github.com/repos/BurntSushi/ripgrep/releases/latest")
                .unwrap();
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["tag_name"].as_str().is_some_and(|t| !t.is_empty()));
    }
}
