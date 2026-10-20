//! Video playback with Media Foundation's media engine on the viewer's own
//! D3D11 device: frames are transferred into a texture (BGRA) and copied
//! into an sRGB view of the same memory layout, so they draw through the
//! normal image path. Audio plays through the default device.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_SAMPLE_DESC,
};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFDXGIDeviceManager, IMFMediaEngine,
    IMFMediaEngineClassFactory, IMFMediaEngineNotify, IMFMediaEngineNotify_Impl,
    MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER, MF_MEDIA_ENGINE_EVENT_ENDED,
    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA,
    MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION, MFARGB, MFCreateAttributes,
    MFCreateDXGIDeviceManager, MFSTARTUP_FULL, MFStartup,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{BSTR, Interface as _, implement};

/// Extensions played as video (lower case).
pub const EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "wmv", "avi", "mkv", "webm", "3gp", "mts", "m2ts", "mpg", "mpeg",
];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// What the engine reports, posted to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Size and duration are known.
    Loaded,
    Ended,
    Error,
}

impl Event {
    pub fn code(self) -> usize {
        self as usize
    }

    pub fn from_code(c: usize) -> Option<Self> {
        [Event::Loaded, Event::Ended, Event::Error]
            .into_iter()
            .find(|e| e.code() == c)
    }
}

#[implement(IMFMediaEngineNotify)]
struct Notify {
    post: Box<dyn Fn(Event) + Send + Sync>,
}

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, _param1: usize, _param2: u32) -> windows::core::Result<()> {
        let e = match event as i32 {
            x if x == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 => Some(Event::Loaded),
            x if x == MF_MEDIA_ENGINE_EVENT_ENDED.0 => Some(Event::Ended),
            x if x == MF_MEDIA_ENGINE_EVENT_ERROR.0 => Some(Event::Error),
            _ => None,
        };
        if let Some(e) = e {
            (self.post)(e);
        }
        Ok(())
    }
}

/// The frame textures: the engine writes the UNORM one, the shader reads
/// the sRGB copy.
struct Frame {
    target: ID3D11Texture2D,
    shown: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    size: (u32, u32),
}

pub struct Player {
    engine: IMFMediaEngine,
    _manager: IMFDXGIDeviceManager,
    frame: Option<Frame>,
}

fn startup() -> bool {
    static STARTED: OnceLock<bool> = OnceLock::new();
    // SAFETY: Media Foundation start-up, once per process.
    *STARTED.get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL).is_ok() })
}

impl Player {
    /// Opens `path` on `device` and starts playing; `post` is called from
    /// an engine thread with its events.
    pub fn open(
        device: &ID3D11Device,
        path: &Path,
        post: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Self> {
        anyhow::ensure!(startup(), "Media Foundation недоступен");
        // SAFETY: plain Media Foundation / COM calls with valid arguments;
        // the device is shared with the engine's threads (made safe below).
        unsafe {
            if let Ok(mt) = device.cast::<ID3D10Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            let mut token = 0;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.context("device manager")?;
            manager.ResetDevice(device, token)?;

            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 3)?;
            let attrs = attrs.context("attributes")?;
            let notify: IMFMediaEngineNotify = Notify {
                post: Box::new(post),
            }
            .into();
            attrs.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &manager)?;
            attrs.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
            attrs.SetUINT32(
                &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
            )?;
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
            let engine = factory.CreateInstance(0, &attrs)?;
            let abs = std::path::absolute(path)?;
            engine.SetSource(&BSTR::from(abs.to_string_lossy().as_ref()))?;
            engine.SetLoop(false)?;
            engine.Play()?;
            Ok(Self {
                engine,
                _manager: manager,
                frame: None,
            })
        }
    }

    /// The video's size, once known.
    pub fn size(&self) -> Option<(u32, u32)> {
        let (mut w, mut h) = (0, 0);
        // SAFETY: valid engine and out-pointers.
        unsafe {
            self.engine
                .GetNativeVideoSize(Some(&mut w), Some(&mut h))
                .ok()?
        };
        (w > 0 && h > 0).then_some((w, h))
    }

    /// Copies a new frame, if there is one; returns its texture and view.
    pub fn frame(
        &mut self,
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
    ) -> Result<Option<(ID3D11Texture2D, ID3D11ShaderResourceView, u32, u32)>> {
        let Some((w, h)) = self.size() else {
            return Ok(None);
        };
        // SAFETY: valid engine; the textures belong to the shared device.
        unsafe {
            if self.engine.OnVideoStreamTick().is_err() {
                return Ok(None);
            }
            if self.frame.as_ref().is_none_or(|f| f.size != (w, h)) {
                self.frame = Some(textures(device, w, h)?);
            }
            let Some(f) = &self.frame else {
                return Ok(None);
            };
            let rect = RECT {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            };
            let black = MFARGB {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbAlpha: 255,
            };
            self.engine
                .TransferVideoFrame(&f.target, None, &rect, Some(&black))?;
            ctx.CopyResource(&f.shown, &f.target);
            Ok(Some((f.shown.clone(), f.srv.clone(), w, h)))
        }
    }

    pub fn paused(&self) -> bool {
        // SAFETY: valid engine.
        unsafe { self.engine.IsPaused().as_bool() || self.engine.IsEnded().as_bool() }
    }

    pub fn toggle_pause(&self) {
        // SAFETY: valid engine.
        unsafe {
            if self.engine.IsEnded().as_bool() {
                let _ = self.engine.SetCurrentTime(0.0);
                let _ = self.engine.Play();
            } else if self.engine.IsPaused().as_bool() {
                let _ = self.engine.Play();
            } else {
                let _ = self.engine.Pause();
            }
        }
    }

    /// Position and length, seconds.
    pub fn time(&self) -> (f64, f64) {
        // SAFETY: valid engine.
        unsafe {
            let d = self.engine.GetDuration();
            (
                self.engine.GetCurrentTime(),
                if d.is_finite() { d } else { 0.0 },
            )
        }
    }

    pub fn seek(&self, seconds: f64) {
        // SAFETY: valid engine.
        unsafe {
            let _ = self.engine.SetCurrentTime(seconds.max(0.0));
        }
    }

    pub fn muted(&self) -> bool {
        // SAFETY: valid engine.
        unsafe { self.engine.GetMuted().as_bool() }
    }

    pub fn set_muted(&self, m: bool) {
        // SAFETY: valid engine.
        unsafe {
            let _ = self.engine.SetMuted(m);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // SAFETY: valid engine; stops its threads before release.
        unsafe {
            let _ = self.engine.Shutdown();
        }
    }
}

fn textures(device: &ID3D11Device, w: u32, h: u32) -> Result<Frame> {
    let desc = |format, bind: u32| D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        ..Default::default()
    };
    // SAFETY: plain resource creation with valid descriptors.
    unsafe {
        let mut target = None;
        device.CreateTexture2D(
            &desc(
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_BIND_RENDER_TARGET.0 as u32,
            ),
            None,
            Some(&mut target),
        )?;
        let mut shown = None;
        device.CreateTexture2D(
            &desc(
                DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
                D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ),
            None,
            Some(&mut shown),
        )?;
        let shown = shown.context("frame texture")?;
        let mut srv = None;
        device.CreateShaderResourceView(&shown, None, Some(&mut srv))?;
        Ok(Frame {
            target: target.context("target texture")?,
            shown,
            srv: srv.context("frame view")?,
            size: (w, h),
        })
    }
}

/// "1:05 / 3:20".
pub fn time_text(now: f64, total: f64) -> String {
    let t = |s: f64| {
        let s = s.max(0.0) as u64;
        if s >= 3600 {
            format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
        } else {
            format!("{}:{:02}", s / 60, s % 60)
        }
    };
    format!("{} / {}", t(now), t(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_types() {
        assert_eq!(time_text(65.4, 200.0), "1:05 / 3:20");
        assert_eq!(time_text(3725.0, 7200.0), "1:02:05 / 2:00:00");
        assert!(is_video(Path::new("a/B.MP4")) && !is_video(Path::new("a.png")));
        assert_eq!(Event::from_code(Event::Ended.code()), Some(Event::Ended));
    }
}
