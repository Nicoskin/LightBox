//! Images on the GPU. Ordinary images are one texture; images larger than
//! the 16384 px texture limit are split into tiles (with a one-pixel overlap
//! so bilinear filtering has no seams) plus a reduced overview texture that
//! is drawn whenever the image is shown smaller than the overview's size.

use anyhow::{Context as _, Result};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
    D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};

use super::Gpu;
use crate::decode::{DecodedImage, MAX_TEXTURE_SIDE, Pixels};
use crate::orientation::Orientation;

/// Tile content size for huge images (plus a 1 px border where they meet).
const TILE: u32 = 8192;

/// One texture and the part of the image it shows.
pub struct Tile {
    _texture: ID3D11Texture2D,
    pub srv: ID3D11ShaderResourceView,
    /// Content rectangle in image pixels (stored orientation).
    pub x0: u32,
    pub y0: u32,
    pub w: u32,
    pub h: u32,
    /// Where the content sits in the texture, in texture UV:
    /// (offset x, offset y, size x, size y).
    pub uv: [f32; 4],
}

/// An image uploaded to the GPU, plus what is needed to display it without
/// the CPU copy.
pub struct GpuImage {
    pub tiles: Vec<Tile>,
    /// Reduced copy of a tiled image (whole image, one texture).
    pub overview: Option<(Tile, u32, u32)>,
    /// Image size in pixels (as stored, before orientation).
    pub width: u32,
    pub height: u32,
    /// Full-resolution size in display orientation.
    pub display: (f64, f64),
    pub orientation: Orientation,
    pub has_alpha: bool,
    /// `false` for a reduced preview.
    pub full: bool,
}

/// Bytes per pixel and texture format for the decoded pixel storage.
fn format_of(p: &Pixels) -> (DXGI_FORMAT, usize, *const u8) {
    match p {
        Pixels::Srgb8(v) => (DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, 4, v.as_ptr()),
        Pixels::Linear16F(v) => (DXGI_FORMAT_R16G16B16A16_FLOAT, 8, v.as_ptr().cast()),
    }
}

/// Creates a mipmapped texture from the `src_box` region of an image whose
/// rows are `pitch` bytes apart.
fn upload_region(
    gpu: &Gpu,
    pixels: &Pixels,
    pitch: usize,
    (x, y, w, h): (u32, u32, u32, u32),
) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
    let (format, bpp, base) = format_of(pixels);
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 0, // full chain
        ArraySize: 1,
        Format: format,
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
    // SAFETY: the region lies inside the pixel buffer (checked by the
    // callers); UpdateSubresource copies the data before returning.
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
        let start = base.add(y as usize * pitch + x as usize * bpp);
        let dst = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: w,
            bottom: h,
            back: 1,
        };
        gpu.context
            .UpdateSubresource(&texture, 0, Some(&dst), start.cast(), pitch as u32, 0);
        gpu.context.GenerateMips(&srv);
        Ok((texture, srv))
    }
}

impl GpuImage {
    pub fn upload(gpu: &Gpu, img: &DecodedImage) -> Result<Self> {
        let (_, bpp, _) = format_of(&img.pixels);
        let pitch = img.width as usize * bpp;
        anyhow::ensure!(
            img.pixels.bytes() == pitch * img.height as usize,
            "pixel buffer size mismatch"
        );
        let (dw, dh) = img.display_size();
        let mut tiles = Vec::new();
        if img.width <= MAX_TEXTURE_SIDE && img.height <= MAX_TEXTURE_SIDE {
            let (texture, srv) =
                upload_region(gpu, &img.pixels, pitch, (0, 0, img.width, img.height))?;
            tiles.push(Tile {
                _texture: texture,
                srv,
                x0: 0,
                y0: 0,
                w: img.width,
                h: img.height,
                uv: [0.0, 0.0, 1.0, 1.0],
            });
        } else {
            for ty in (0..img.height).step_by(TILE as usize) {
                for tx in (0..img.width).step_by(TILE as usize) {
                    let w = TILE.min(img.width - tx);
                    let h = TILE.min(img.height - ty);
                    // One extra pixel from each neighbour.
                    let bl = u32::from(tx > 0);
                    let bt = u32::from(ty > 0);
                    let br = u32::from(tx + w < img.width);
                    let bb = u32::from(ty + h < img.height);
                    let (rx, ry) = (tx - bl, ty - bt);
                    let (rw, rh) = (w + bl + br, h + bt + bb);
                    let (texture, srv) = upload_region(gpu, &img.pixels, pitch, (rx, ry, rw, rh))?;
                    tiles.push(Tile {
                        _texture: texture,
                        srv,
                        x0: tx,
                        y0: ty,
                        w,
                        h,
                        uv: [
                            bl as f32 / rw as f32,
                            bt as f32 / rh as f32,
                            w as f32 / rw as f32,
                            h as f32 / rh as f32,
                        ],
                    });
                }
            }
        }
        let overview = match &img.overview {
            Some(o) => {
                let (_, obpp, _) = format_of(&o.pixels);
                let (texture, srv) = upload_region(
                    gpu,
                    &o.pixels,
                    o.width as usize * obpp,
                    (0, 0, o.width, o.height),
                )?;
                Some((
                    Tile {
                        _texture: texture,
                        srv,
                        x0: 0,
                        y0: 0,
                        w: img.width,
                        h: img.height,
                        uv: [0.0, 0.0, 1.0, 1.0],
                    },
                    o.width,
                    o.height,
                ))
            }
            None => None,
        };
        Ok(Self {
            tiles,
            overview,
            width: img.width,
            height: img.height,
            display: (f64::from(dw), f64::from(dh)),
            orientation: img.orientation,
            has_alpha: img.has_alpha,
            full: img.is_full(),
        })
    }

    /// Texture size in display orientation.
    pub fn display_texels(&self) -> (f64, f64) {
        let (w, h) = self.orientation.apply_size(self.width, self.height);
        (f64::from(w), f64::from(h))
    }
}
