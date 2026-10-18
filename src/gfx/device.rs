use anyhow::{Context as _, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL,
    D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_RENDER_TARGET_VIEW_DESC,
    D3D11_RENDER_TARGET_VIEW_DESC_0, D3D11_RTV_DIMENSION_TEXTURE2D, D3D11_SDK_VERSION,
    D3D11_TEX2D_RTV, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice1, IDXGIFactory2,
    IDXGISwapChain1,
};
use windows::core::Interface as _;

use super::renderer::Renderer;

/// Which Direct3D implementation backs a [`Gpu`].
///
/// Creating a hardware device costs ~140 ms on some drivers (measured on
/// NVIDIA), far more than decoding a typical photo, while WARP (the CPU
/// rasteriser) is ready in ~20 ms. The first frame is therefore drawn with
/// whichever device is ready first, and the hardware device takes over
/// seamlessly once it exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    Hardware,
    Warp,
}

/// Device, composition swap chain and pipeline state. Independent of any window.
pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub swap_chain: IDXGISwapChain1,
    pub renderer: Renderer,
    adapter: Adapter,
    rtv: Option<ID3D11RenderTargetView>,
    size: (u32, u32),
}

// SAFETY: D3D11 devices and DXGI objects are free-threaded. `Gpu` is built on
// a worker thread and then *moved* to the UI thread, which is its only user
// afterwards (the immediate context is never shared).
unsafe impl Send for Gpu {}

impl Gpu {
    /// Creates everything needed to draw a `width × height` frame.
    pub fn new(adapter: Adapter, width: u32, height: u32) -> Result<Self> {
        let (device, context) = create_device(adapter)?;
        crate::bench::mark(match adapter {
            Adapter::Hardware => "d3d11 device (hardware)",
            Adapter::Warp => "d3d11 device (warp)",
        });

        let dxgi_device: IDXGIDevice1 = device.cast()?;
        // Lowest latency: never queue more than one frame.
        // SAFETY: valid interface; plain value argument.
        unsafe { dxgi_device.SetMaximumFrameLatency(1)? };

        // SAFETY: no preconditions.
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
        let swap_chain = create_swap_chain(&factory, &device, width, height)?;
        let renderer = Renderer::new(&device)?;

        let mut gpu = Self {
            device,
            context,
            swap_chain,
            renderer,
            adapter,
            rtv: None,
            size: (width.max(1), height.max(1)),
        };
        gpu.create_rtv()?;
        Ok(gpu)
    }

    pub fn adapter(&self) -> Adapter {
        self.adapter
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn rtv(&self) -> Option<&ID3D11RenderTargetView> {
        self.rtv.as_ref()
    }

    /// Resizes the back buffers (e.g. after a display or DPI change).
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let size = (width.max(1), height.max(1));
        if size == self.size {
            return Ok(());
        }
        self.rtv = None;
        // SAFETY: the context must not hold references to the back buffer.
        unsafe {
            self.context.OMSetRenderTargets(None, None);
            self.context.Flush();
            self.swap_chain.ResizeBuffers(
                0,
                size.0,
                size.1,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        self.size = size;
        self.create_rtv()
    }

    pub fn present(&self) -> Result<()> {
        // SAFETY: valid swap chain.
        unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()? };
        Ok(())
    }

    fn create_rtv(&mut self) -> Result<()> {
        // SAFETY: buffer 0 always exists; the sRGB view of a UNORM flip-model
        // back buffer is explicitly allowed.
        unsafe {
            let back: ID3D11Texture2D = self.swap_chain.GetBuffer(0)?;
            let desc = D3D11_RENDER_TARGET_VIEW_DESC {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
                ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 },
                },
            };
            let mut rtv = None;
            self.device
                .CreateRenderTargetView(&back, Some(&desc), Some(&mut rtv))?;
            self.rtv = rtv;
        }
        Ok(())
    }
}

fn create_device(adapter: Adapter) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    const LEVELS: [D3D_FEATURE_LEVEL; 4] = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    // BGRA support is required for Direct2D interop (UI overlays).
    let mut flags: D3D11_CREATE_DEVICE_FLAG = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
    if cfg!(debug_assertions) && std::env::var_os("LIGHTBOX_D3D_DEBUG").is_some() {
        flags |= windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_DEBUG;
    }
    let driver: D3D_DRIVER_TYPE = match adapter {
        Adapter::Hardware => D3D_DRIVER_TYPE_HARDWARE,
        Adapter::Warp => D3D_DRIVER_TYPE_WARP,
    };
    let mut device = None;
    let mut context = None;
    // SAFETY: out-pointers are valid for the call.
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            flags,
            Some(&LEVELS),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .with_context(|| format!("D3D11CreateDevice({adapter:?})"))?;
    }
    Ok((device.context("no device")?, context.context("no context")?))
}

fn create_swap_chain(
    factory: &IDXGIFactory2,
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<IDXGISwapChain1> {
    let desc = |effect: DXGI_SWAP_EFFECT| DXGI_SWAP_CHAIN_DESC1 {
        Width: width.max(1),
        Height: height.max(1),
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: effect,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        Flags: 0,
    };
    // SAFETY: descriptor outlives the call; device is valid.
    unsafe {
        factory
            .CreateSwapChainForComposition(device, &desc(DXGI_SWAP_EFFECT_FLIP_DISCARD), None)
            .or_else(|_| {
                factory.CreateSwapChainForComposition(
                    device,
                    &desc(DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL),
                    None,
                )
            })
            .context("CreateSwapChainForComposition")
    }
}
