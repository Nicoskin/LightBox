use anyhow::{Context as _, Result};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_RESOURCE_MISC_GENERATE_MIPS,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_SAMPLE_DESC};

use super::Gpu;
use crate::decode::DecodedImage;

/// An image uploaded to the GPU with a full mip chain.
pub struct GpuImage {
    _texture: ID3D11Texture2D,
    pub srv: ID3D11ShaderResourceView,
}

impl GpuImage {
    pub fn upload(gpu: &Gpu, img: &DecodedImage) -> Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: img.width,
            Height: img.height,
            MipLevels: 0, // full chain
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            // RENDER_TARGET is required by GenerateMips.
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
        };
        let expected = img.width as usize * img.height as usize * 4;
        anyhow::ensure!(img.pixels.len() == expected, "pixel buffer size mismatch");

        // SAFETY: descriptor and pixel buffer are valid for the duration of
        // the calls; UpdateSubresource copies the data before returning.
        unsafe {
            let mut texture = None;
            gpu.device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .context("CreateTexture2D")?;
            let texture = texture.context("no texture")?;
            let mut srv = None;
            gpu.device
                .CreateShaderResourceView(&texture, None, Some(&mut srv))?;
            let srv = srv.context("no SRV")?;
            gpu.context.UpdateSubresource(
                &texture,
                0,
                None,
                img.pixels.as_ptr().cast(),
                img.width * 4,
                0,
            );
            gpu.context.GenerateMips(&srv);
            Ok(Self {
                _texture: texture,
                srv,
            })
        }
    }
}
