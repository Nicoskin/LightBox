use anyhow::{Context as _, Result};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE,
    D3D11_BLEND_OP_ADD, D3D11_BUFFER_DESC, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_NEVER,
    D3D11_CULL_NONE, D3D11_FILL_SOLID, D3D11_FILTER, D3D11_FILTER_MIN_LINEAR_MAG_POINT_MIP_LINEAR,
    D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_FILTER_MIN_MAG_MIP_POINT, D3D11_FLOAT32_MAX,
    D3D11_RASTERIZER_DESC, D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SAMPLER_DESC,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT, ID3D11BlendState,
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RasterizerState,
    ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11VertexShader,
};

use crate::view::Quad;

static VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/image_vs.cso"));
static PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/image_ps.cso"));

/// Texture filtering mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Bilinear when magnifying, trilinear (mipmapped) when minifying.
    Smooth,
    /// Nearest-neighbour when magnifying (crisp pixels), trilinear when minifying.
    Pixelated,
    /// Nearest-neighbour everywhere.
    #[expect(dead_code, reason = "settings option in M6")]
    Nearest,
}

/// What shows through transparent image areas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Background {
    /// The dimmed desktop.
    None,
    /// Grey/white checkerboard with cells of the given size in screen pixels.
    Checker(f32),
    /// Solid colour, premultiplied linear RGBA.
    Solid([f32; 4]),
}

/// One textured piece of a layer: a whole image or one tile of a huge one.
pub struct Part<'a> {
    pub srv: &'a ID3D11ShaderResourceView,
    /// Visible part on screen and its UV range (display orientation).
    pub quad: Quad,
    /// Content rectangle inside the texture (offset xy, size zw).
    pub tex: [f32; 4],
    /// Sides that are edges of the whole image (anti-aliased when the
    /// layer is turned): 1 left, 2 top, 4 right, 8 bottom.
    pub edges: u32,
}

/// One image drawn in a frame (two during a page turn).
pub struct Layer<'a> {
    pub parts: Vec<Part<'a>>,
    /// Top-left of the whole (unclipped) image on screen; anchors the checkerboard.
    pub origin: [f32; 2],
    pub background: Background,
    pub opacity: f32,
    /// EXIF-style orientation 1..8 to display the texture with.
    pub orientation: u32,
    /// Free rotation (cos, sin) about `pivot` (screen pixels).
    pub rotation: [f32; 2],
    pub pivot: [f32; 2],
}

/// Everything needed to draw one frame.
pub struct DrawParams<'a> {
    /// Backdrop colour, premultiplied linear RGBA (e.g. 80 % black).
    pub clear: [f32; 4],
    /// Drawn back to front.
    pub layers: &'a [Layer<'a>],
    pub filter: Filter,
}

/// Must match `cbuffer Params` in shaders/image.hlsl.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Constants {
    dst_rect: [f32; 4],
    uv_rect: [f32; 4],
    bg_color: [f32; 4],
    viewport: [f32; 2],
    checker_origin: [f32; 2],
    checker_size: f32,
    orientation: u32,
    bg_mode: u32,
    channel_mode: u32,
    grid: u32,
    opacity: f32,
    rot: [f32; 2],
    tex_rect: [f32; 4],
    pivot: [f32; 2],
    edges: u32,
    pad: u32,
}

const _: () = assert!(std::mem::size_of::<Constants>().is_multiple_of(16));

pub struct Renderer {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    cbuffer: ID3D11Buffer,
    blend: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    smooth: ID3D11SamplerState,
    pixelated: ID3D11SamplerState,
    nearest: ID3D11SamplerState,
}

impl Renderer {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        // SAFETY: descriptors and bytecode slices outlive the calls.
        unsafe {
            let mut vs = None;
            device.CreateVertexShader(VS, None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(PS, None, Some(&mut ps))?;

            let cb_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<Constants>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let mut cbuffer = None;
            device.CreateBuffer(&cb_desc, None, Some(&mut cbuffer))?;

            // Premultiplied "over": out = src + dst * (1 - src.a).
            let mut blend_desc = D3D11_BLEND_DESC::default();
            blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut blend = None;
            device.CreateBlendState(&blend_desc, Some(&mut blend))?;

            let raster_desc = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            };
            let mut raster = None;
            device.CreateRasterizerState(&raster_desc, Some(&mut raster))?;

            Ok(Self {
                vs: vs.context("vs")?,
                ps: ps.context("ps")?,
                cbuffer: cbuffer.context("cbuffer")?,
                blend: blend.context("blend")?,
                raster: raster.context("raster")?,
                smooth: sampler(device, D3D11_FILTER_MIN_MAG_MIP_LINEAR)?,
                pixelated: sampler(device, D3D11_FILTER_MIN_LINEAR_MAG_POINT_MIP_LINEAR)?,
                nearest: sampler(device, D3D11_FILTER_MIN_MAG_MIP_POINT)?,
            })
        }
    }

    pub fn draw(
        &self,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        (w, h): (u32, u32),
        p: &DrawParams,
    ) -> Result<()> {
        // SAFETY: all resources are owned by `gpu`/`self` and outlive the calls.
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            ctx.ClearRenderTargetView(rtv, &p.clear);
            if p.layers.is_empty() {
                return Ok(());
            }

            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: w as f32,
                Height: h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.RSSetState(&self.raster);
            ctx.OMSetBlendState(&self.blend, None, 0xffff_ffff);
            ctx.IASetInputLayout(None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cbuffer.clone())]));
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cbuffer.clone())]));
            let sampler = match p.filter {
                Filter::Smooth => &self.smooth,
                Filter::Pixelated => &self.pixelated,
                Filter::Nearest => &self.nearest,
            };
            ctx.PSSetSamplers(0, Some(&[Some(sampler.clone())]));

            for layer in p.layers {
                let (bg_mode, bg_color, checker) = match layer.background {
                    Background::None => (0, [0.0; 4], 8.0),
                    Background::Checker(size) => (1, [0.0; 4], size.max(1.0)),
                    Background::Solid(c) => (2, c, 8.0),
                };
                for part in &layer.parts {
                    let constants = Constants {
                        dst_rect: part.quad.dst,
                        uv_rect: part.quad.uv,
                        bg_color,
                        viewport: [w as f32, h as f32],
                        checker_origin: layer.origin,
                        checker_size: checker,
                        orientation: layer.orientation,
                        bg_mode,
                        opacity: layer.opacity.clamp(0.0, 1.0),
                        tex_rect: part.tex,
                        rot: layer.rotation,
                        pivot: layer.pivot,
                        edges: part.edges,
                        ..Default::default()
                    };
                    ctx.UpdateSubresource(
                        &self.cbuffer,
                        0,
                        None,
                        std::ptr::from_ref(&constants).cast(),
                        0,
                        0,
                    );
                    ctx.PSSetShaderResources(0, Some(&[Some(part.srv.clone())]));
                    ctx.Draw(4, 0);
                }
            }
        }
        Ok(())
    }
}

fn sampler(device: &ID3D11Device, filter: D3D11_FILTER) -> Result<ID3D11SamplerState> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: filter,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: D3D11_FLOAT32_MAX,
    };
    let mut s = None;
    // SAFETY: descriptor outlives the call.
    unsafe { device.CreateSamplerState(&desc, Some(&mut s))? };
    s.context("sampler")
}
