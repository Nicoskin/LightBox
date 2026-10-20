//! 3D model drawing: the mesh lit by a key light with a fill and sky
//! ambient, over a floor grid, rendered with 4× multisampling and a depth
//! buffer into an off-screen target that is resolved into the back buffer.

use anyhow::{Context as _, Result};
use windows::Win32::Graphics::Direct3D::{
    D3D11_PRIMITIVE_TOPOLOGY_LINELIST, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
    D3D11_SRV_DIMENSION_BUFFER,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_DEPTH_STENCIL, D3D11_BIND_RENDER_TARGET,
    D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VERTEX_BUFFER, D3D11_BLEND_DESC,
    D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD, D3D11_BUFFER_DESC,
    D3D11_BUFFER_SRV, D3D11_BUFFER_SRV_0, D3D11_BUFFER_SRV_1, D3D11_CLEAR_DEPTH,
    D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_LESS_EQUAL, D3D11_CULL_NONE,
    D3D11_DEPTH_STENCIL_DESC, D3D11_DEPTH_STENCIL_VIEW_DESC, D3D11_DEPTH_WRITE_MASK_ALL,
    D3D11_DEPTH_WRITE_MASK_ZERO, D3D11_DSV_DIMENSION_TEXTURE2DMS, D3D11_FILL_SOLID,
    D3D11_FILL_WIREFRAME, D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA,
    D3D11_RASTERIZER_DESC, D3D11_RENDER_TARGET_BLEND_DESC, D3D11_RENDER_TARGET_VIEW_DESC,
    D3D11_RESOURCE_MISC_BUFFER_STRUCTURED, D3D11_RTV_DIMENSION_TEXTURE2DMS,
    D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_IMMUTABLE, D3D11_VIEWPORT,
    ID3D11BlendState, ID3D11Buffer, ID3D11DepthStencilState, ID3D11DepthStencilView, ID3D11Device,
    ID3D11DeviceContext, ID3D11InputLayout, ID3D11PixelShader, ID3D11RasterizerState,
    ID3D11RenderTargetView, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT,
    DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::core::s;

use crate::model::Mesh;

static VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model_vs.cso"));
static PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model_ps.cso"));
static VS_PATH: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model_path_vs.cso"));
const SAMPLES: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    col: [f32; 4],
}

/// Must match `cbuffer Params` in shaders/model.hlsl.
#[repr(C)]
#[derive(Clone, Copy)]
struct Constants {
    view_proj: [[f32; 4]; 4],
    light: [f32; 4],
    eye: [f32; 4],
    mode: u32,
    pad: [u32; 3],
    section: [f32; 4],
}

/// A mesh (or a G-code toolpath) and its floor grid on the GPU.
pub struct GpuMesh {
    vb: Option<ID3D11Buffer>,
    count: u32,
    /// Toolpath segments (structured buffer), its view and length.
    path: Option<(ID3D11Buffer, ID3D11ShaderResourceView, u32)>,
    grid: Option<(ID3D11Buffer, u32)>,
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn buffer(device: &ID3D11Device, data: &[Vertex]) -> Result<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: std::mem::size_of_val(data) as u32,
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
        ..Default::default()
    };
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: data.as_ptr().cast(),
        ..Default::default()
    };
    let mut vb = None;
    // SAFETY: the data outlives the call and matches the declared size.
    unsafe { device.CreateBuffer(&desc, Some(&init), Some(&mut vb))? };
    vb.context("vertex buffer")
}

/// Toolpath segments as a structured buffer the path shader reads.
fn path_buffer(
    device: &ID3D11Device,
    segs: &[crate::model::gcode::Seg],
) -> Result<(ID3D11Buffer, ID3D11ShaderResourceView, u32)> {
    let stride = std::mem::size_of::<crate::model::gcode::Seg>() as u32;
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: stride * segs.len().max(1) as u32,
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
        StructureByteStride: stride,
        ..Default::default()
    };
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: segs.as_ptr().cast(),
        ..Default::default()
    };
    // SAFETY: the data outlives the call and matches the declared size.
    unsafe {
        let mut b = None;
        device.CreateBuffer(&desc, Some(&init), Some(&mut b))?;
        let b = b.context("path buffer")?;
        let mut srv = None;
        device.CreateShaderResourceView(
            &b,
            Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_UNKNOWN,
                ViewDimension: D3D11_SRV_DIMENSION_BUFFER,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Buffer: D3D11_BUFFER_SRV {
                        Anonymous1: D3D11_BUFFER_SRV_0 { FirstElement: 0 },
                        Anonymous2: D3D11_BUFFER_SRV_1 {
                            NumElements: segs.len() as u32,
                        },
                    },
                },
            }),
            Some(&mut srv),
        )?;
        Ok((b, srv.context("path view")?, segs.len() as u32))
    }
}

/// Grid lines on the floor under the model: a "nice" spacing (1, 2 or 5 ×
/// 10ⁿ units), every tenth line brighter.
fn grid(mesh: &Mesh) -> Vec<Vertex> {
    let r = mesh.radius().max(1e-3);
    let extent = r * 2.6;
    let raw = extent / 12.0;
    let pow = 10f32.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * pow)
        .find(|s| *s >= raw)
        .unwrap_or(raw);
    let c = mesh.center();
    let y = mesh.min[1];
    let n = (extent / step).ceil() as i32;
    let (cx, cz) = ((c[0] / step).round() * step, (c[2] / step).round() * step);
    let half = n as f32 * step;
    let mut v = Vec::new();
    for i in -n..=n {
        let major = i % 10 == 0;
        let a = if major { 0.30 } else { 0.13 };
        // Premultiplied grey.
        let col = [0.55 * a, 0.55 * a, 0.58 * a, a];
        let t = i as f32 * step;
        let line = |p0: [f32; 3], p1: [f32; 3]| {
            [
                Vertex {
                    pos: p0,
                    nrm: [0.0, 1.0, 0.0],
                    col,
                },
                Vertex {
                    pos: p1,
                    nrm: [0.0, 1.0, 0.0],
                    col,
                },
            ]
        };
        v.extend(line([cx + t, y, cz - half], [cx + t, y, cz + half]));
        v.extend(line([cx - half, y, cz + t], [cx + half, y, cz + t]));
    }
    v
}

impl GpuMesh {
    pub fn upload(device: &ID3D11Device, mesh: &Mesh) -> Result<Self> {
        let colors = mesh.colors.as_deref();
        let verts: Vec<Vertex> = (0..mesh.positions.len())
            .map(|i| {
                let c = colors.map_or(crate::model::DEFAULT_COLOR, |c| c[i]);
                Vertex {
                    pos: mesh.positions[i],
                    nrm: mesh.normals[i],
                    col: [
                        srgb_to_linear(c[0]),
                        srgb_to_linear(c[1]),
                        srgb_to_linear(c[2]),
                        1.0,
                    ],
                }
            })
            .collect();
        let g = grid(mesh);
        Ok(Self {
            vb: if verts.is_empty() {
                None
            } else {
                Some(buffer(device, &verts)?)
            },
            count: verts.len() as u32,
            path: match &mesh.path {
                Some(p) => Some(path_buffer(device, &p.segs)?),
                None => None,
            },
            grid: if g.is_empty() {
                None
            } else {
                Some((buffer(device, &g)?, g.len() as u32))
            },
        })
    }
}

/// What to draw this frame.
pub struct ModelDraw<'a> {
    pub mesh: &'a GpuMesh,
    /// Toolpaths: how many segments to draw (up to a layer).
    pub limit: Option<u32>,
    pub view_proj: [[f32; 4]; 4],
    pub eye: [f32; 3],
    /// Direction towards the key light.
    pub light: [f32; 3],
    /// Lines of the triangles over the surface.
    pub wire: bool,
    /// Section: everything above this height is cut away.
    pub section: Option<f32>,
}

struct Targets {
    size: (u32, u32),
    color: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    dsv: ID3D11DepthStencilView,
}

pub struct ModelRenderer {
    vs: ID3D11VertexShader,
    vs_path: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    cbuffer: ID3D11Buffer,
    raster: ID3D11RasterizerState,
    /// Wireframe, pulled a little towards the eye so it wins over the
    /// surface it lies on.
    wire: ID3D11RasterizerState,
    depth_write: ID3D11DepthStencilState,
    depth_read: ID3D11DepthStencilState,
    blend: ID3D11BlendState,
    targets: Option<Targets>,
}

impl ModelRenderer {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        // SAFETY: descriptors and bytecode outlive the calls.
        unsafe {
            let mut vs = None;
            device.CreateVertexShader(VS, None, Some(&mut vs))?;
            let mut vs_path = None;
            device.CreateVertexShader(VS_PATH, None, Some(&mut vs_path))?;
            let mut ps = None;
            device.CreatePixelShader(PS, None, Some(&mut ps))?;
            let elements = [
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("POSITION"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32B32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 0,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("NORMAL"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32B32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 12,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("COLOR"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 24,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
            ];
            let mut layout = None;
            device.CreateInputLayout(&elements, VS, Some(&mut layout))?;
            let mut cbuffer = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<Constants>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut cbuffer),
            )?;
            let rasterizer = |fill, bias: i32, slope: f32| -> Result<ID3D11RasterizerState> {
                let mut r = None;
                device.CreateRasterizerState(
                    &D3D11_RASTERIZER_DESC {
                        FillMode: fill,
                        CullMode: D3D11_CULL_NONE,
                        DepthBias: bias,
                        SlopeScaledDepthBias: slope,
                        DepthClipEnable: true.into(),
                        MultisampleEnable: true.into(),
                        AntialiasedLineEnable: false.into(),
                        ..Default::default()
                    },
                    Some(&mut r),
                )?;
                r.context("rasterizer")
            };
            let depth = |write: bool| -> Result<ID3D11DepthStencilState> {
                let mut s = None;
                device.CreateDepthStencilState(
                    &D3D11_DEPTH_STENCIL_DESC {
                        DepthEnable: true.into(),
                        DepthWriteMask: if write {
                            D3D11_DEPTH_WRITE_MASK_ALL
                        } else {
                            D3D11_DEPTH_WRITE_MASK_ZERO
                        },
                        DepthFunc: D3D11_COMPARISON_LESS_EQUAL,
                        ..Default::default()
                    },
                    Some(&mut s),
                )?;
                s.context("depth state")
            };
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
            Ok(Self {
                vs: vs.context("vs")?,
                vs_path: vs_path.context("vs path")?,
                ps: ps.context("ps")?,
                layout: layout.context("layout")?,
                cbuffer: cbuffer.context("cbuffer")?,
                raster: rasterizer(D3D11_FILL_SOLID, 0, 0.0)?,
                wire: rasterizer(D3D11_FILL_WIREFRAME, -2000, -1.5)?,
                depth_write: depth(true)?,
                depth_read: depth(false)?,
                blend: blend.context("blend")?,
                targets: None,
            })
        }
    }

    fn targets(&mut self, device: &ID3D11Device, size: (u32, u32)) -> Result<&Targets> {
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = None;
            let tex = |format, bind: u32| -> Result<ID3D11Texture2D> {
                let mut t = None;
                // SAFETY: plain texture creation.
                unsafe {
                    device.CreateTexture2D(
                        &D3D11_TEXTURE2D_DESC {
                            Width: size.0,
                            Height: size.1,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: format,
                            SampleDesc: DXGI_SAMPLE_DESC {
                                Count: SAMPLES,
                                Quality: 0,
                            },
                            Usage: D3D11_USAGE_DEFAULT,
                            BindFlags: bind,
                            ..Default::default()
                        },
                        None,
                        Some(&mut t),
                    )?;
                }
                t.context("texture")
            };
            let color = tex(
                DXGI_FORMAT_B8G8R8A8_TYPELESS,
                D3D11_BIND_RENDER_TARGET.0 as u32,
            )?;
            let depth = tex(DXGI_FORMAT_D32_FLOAT, D3D11_BIND_DEPTH_STENCIL.0 as u32)?;
            // SAFETY: views of the textures just created.
            let (rtv, dsv) = unsafe {
                let mut rtv = None;
                device.CreateRenderTargetView(
                    &color,
                    Some(&D3D11_RENDER_TARGET_VIEW_DESC {
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
                        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2DMS,
                        ..Default::default()
                    }),
                    Some(&mut rtv),
                )?;
                let mut dsv = None;
                device.CreateDepthStencilView(
                    &depth,
                    Some(&D3D11_DEPTH_STENCIL_VIEW_DESC {
                        Format: DXGI_FORMAT_D32_FLOAT,
                        ViewDimension: D3D11_DSV_DIMENSION_TEXTURE2DMS,
                        ..Default::default()
                    }),
                    Some(&mut dsv),
                )?;
                (rtv.context("rtv")?, dsv.context("dsv")?)
            };
            self.targets = Some(Targets {
                size,
                color,
                rtv,
                dsv,
            });
        }
        self.targets.as_ref().context("targets")
    }

    /// Draws the model over `clear` into `back` (the swap chain buffer).
    pub fn draw(
        &mut self,
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        back: &ID3D11Texture2D,
        size: (u32, u32),
        clear: [f32; 4],
        d: &ModelDraw,
    ) -> Result<()> {
        let (vs, ps, layout, cbuffer, raster, dw, dr, blend) = (
            self.vs.clone(),
            self.ps.clone(),
            self.layout.clone(),
            self.cbuffer.clone(),
            self.raster.clone(),
            self.depth_write.clone(),
            self.depth_read.clone(),
            self.blend.clone(),
        );
        let vs_path = self.vs_path.clone();
        let wire = self.wire.clone();
        let t = self.targets(device, size)?;
        let stride = std::mem::size_of::<Vertex>() as u32;
        let offset = 0u32;
        let constants = |mode: u32| Constants {
            view_proj: d.view_proj,
            light: [d.light[0], d.light[1], d.light[2], 0.0],
            eye: [d.eye[0], d.eye[1], d.eye[2], 1.0],
            mode,
            pad: [0; 3],
            section: match d.section {
                Some(h) => [1.0, h, 0.0, 0.0],
                None => [0.0; 4],
            },
        };
        // SAFETY: all resources belong to this device and outlive the calls.
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(t.rtv.clone())]), &t.dsv);
            ctx.ClearRenderTargetView(&t.rtv, &clear);
            ctx.ClearDepthStencilView(&t.dsv, D3D11_CLEAR_DEPTH.0, 1.0, 0);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: size.0 as f32,
                Height: size.1 as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.RSSetState(&raster);
            ctx.IASetInputLayout(&layout);
            ctx.VSSetShader(&vs, None);
            ctx.PSSetShader(&ps, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(cbuffer.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(cbuffer.clone())]));

            // The model first (writes depth), then the grid behind it.
            ctx.OMSetBlendState(None, None, 0xffff_ffff);
            ctx.OMSetDepthStencilState(&dw, 0);
            ctx.UpdateSubresource(
                &cbuffer,
                0,
                None,
                std::ptr::from_ref(&constants(0)).cast(),
                0,
                0,
            );
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            if let Some(vb) = &d.mesh.vb {
                ctx.IASetVertexBuffers(0, 1, Some(&Some(vb.clone())), Some(&stride), Some(&offset));
                ctx.Draw(d.mesh.count, 0);
                if d.wire {
                    ctx.RSSetState(&wire);
                    ctx.OMSetBlendState(&blend, None, 0xffff_ffff);
                    ctx.OMSetDepthStencilState(&dr, 0);
                    ctx.UpdateSubresource(
                        &cbuffer,
                        0,
                        None,
                        std::ptr::from_ref(&constants(2)).cast(),
                        0,
                        0,
                    );
                    ctx.Draw(d.mesh.count, 0);
                    ctx.RSSetState(&raster);
                    ctx.OMSetBlendState(None, None, 0xffff_ffff);
                    ctx.OMSetDepthStencilState(&dw, 0);
                }
            }
            // A toolpath: one bar (18 vertices) per segment, up to `limit`.
            if let Some((_, srv, n)) = &d.mesh.path {
                let count = d.limit.unwrap_or(*n).min(*n);
                ctx.IASetInputLayout(None);
                ctx.VSSetShader(&vs_path, None);
                ctx.VSSetShaderResources(0, Some(&[Some(srv.clone())]));
                ctx.DrawInstanced(18, count, 0, 0);
                ctx.VSSetShaderResources(0, Some(&[None]));
                ctx.VSSetShader(&vs, None);
                ctx.IASetInputLayout(&layout);
            }

            if let Some((grid, n)) = &d.mesh.grid {
                ctx.OMSetBlendState(&blend, None, 0xffff_ffff);
                ctx.OMSetDepthStencilState(&dr, 0);
                ctx.UpdateSubresource(
                    &cbuffer,
                    0,
                    None,
                    std::ptr::from_ref(&constants(1)).cast(),
                    0,
                    0,
                );
                ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_LINELIST);
                ctx.IASetVertexBuffers(
                    0,
                    1,
                    Some(&Some(grid.clone())),
                    Some(&stride),
                    Some(&offset),
                );
                ctx.Draw(*n, 0);
            }
            ctx.OMSetDepthStencilState(None, 0);
            ctx.OMSetRenderTargets(None, None);
            ctx.ResolveSubresource(back, 0, &t.color, 0, DXGI_FORMAT_B8G8R8A8_UNORM);
            ctx.IASetInputLayout(None);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::{Adapter, Gpu};
    use crate::model::Builder;
    use crate::model::raster::Orbit;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_USAGE_STAGING,
    };

    fn cube() -> Mesh {
        let mut b = Builder::new(false);
        let v = |x: f32, y: f32, z: f32| [x, y, z];
        let quads = [
            [v(0., 0., 1.), v(1., 0., 1.), v(1., 1., 1.), v(0., 1., 1.)],
            [v(1., 0., 0.), v(0., 0., 0.), v(0., 1., 0.), v(1., 1., 0.)],
            [v(0., 1., 1.), v(1., 1., 1.), v(1., 1., 0.), v(0., 1., 0.)],
            [v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 1.), v(0., 0., 1.)],
            [v(1., 0., 1.), v(1., 0., 0.), v(1., 1., 0.), v(1., 1., 1.)],
            [v(0., 0., 0.), v(0., 0., 1.), v(0., 1., 1.), v(0., 1., 0.)],
        ];
        for q in quads {
            b.triangle([q[0], q[1], q[2]], None);
            b.triangle([q[0], q[2], q[3]], None);
        }
        b.finish().unwrap()
    }

    /// A 20 × 20 mm zig-zag of infill drawn as a toolpath: the frame shows
    /// red lines; with a limit of one segment almost nothing.
    #[test]
    fn draws_a_toolpath() {
        let mut g = String::from("G90\nM82\nG92 E0\nG1 Z0.2\n;TYPE:Sparse infill\n");
        let mut e = 0.0;
        for i in 0..50 {
            let y = i as f32 * 0.4;
            let (x0, x1) = if i % 2 == 0 { (0.0, 20.0) } else { (20.0, 0.0) };
            e += 1.0;
            g.push_str(&format!("G1 X{x0} Y{y}\nG1 X{x1} Y{y} E{e}\n"));
        }
        let mesh = crate::model::gcode::parse(g.as_bytes()).unwrap();
        let all = red_pixels(&mesh, None, None);
        let one = red_pixels(&mesh, Some(1), None);
        assert!(all > 2000, "{all}");
        assert!(one < all / 10, "{one} vs {all}");
    }

    /// A cube cut at half its height shows its inside in the section colour.
    #[test]
    fn section_shows_the_inside() {
        let m = cube();
        assert_eq!(red_pixels(&m, None, None), 0);
        let cut = red_pixels(&m, None, Some(0.5));
        assert!(cut > 300, "{cut}");
    }

    /// Draws `mesh` with WARP (160 × 120) and counts the clearly red pixels.
    fn red_pixels(mesh: &Mesh, limit: Option<u32>, section: Option<f32>) -> u32 {
        let gpu = Gpu::new(Adapter::Warp, 64, 64).expect("warp");
        let gm = GpuMesh::upload(&gpu.device, mesh).unwrap();
        let cam = Orbit::fit(mesh);
        let (w, h) = (160u32, 120u32);
        let tex = |usage, bind: u32, cpu: u32| {
            let mut t = None;
            unsafe {
                gpu.device
                    .CreateTexture2D(
                        &D3D11_TEXTURE2D_DESC {
                            Width: w,
                            Height: h,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            SampleDesc: DXGI_SAMPLE_DESC {
                                Count: 1,
                                Quality: 0,
                            },
                            Usage: usage,
                            BindFlags: bind,
                            CPUAccessFlags: cpu,
                            ..Default::default()
                        },
                        None,
                        Some(&mut t),
                    )
                    .unwrap();
            }
            t.unwrap()
        };
        let target = tex(D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0);
        let staging = tex(D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32);
        let mut r = ModelRenderer::new(&gpu.device).unwrap();
        let d = ModelDraw {
            mesh: &gm,
            view_proj: cam.view_proj(w as f32 / h as f32, mesh.radius()),
            eye: cam.eye(),
            light: [0.3, 0.8, 0.5],
            limit,
            wire: false,
            section,
        };
        r.draw(
            &gpu.device,
            &gpu.context,
            &target,
            (w, h),
            [0.0, 0.0, 0.0, 1.0],
            &d,
        )
        .unwrap();
        unsafe {
            gpu.context.CopyResource(&staging, &target);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .unwrap();
            let mut n = 0;
            for y in 0..h {
                for x in 0..w {
                    let p = (m.pData as *const u8).add((y * m.RowPitch + x * 4) as usize);
                    let (b, g, r) = (*p, *p.add(1), *p.add(2));
                    if r > 60 && u32::from(r) > u32::from(g) * 2 && u32::from(r) > u32::from(b) * 2
                    {
                        n += 1;
                    }
                }
            }
            gpu.context.Unmap(&staging, 0);
            n
        }
    }

    /// Draws a cube with WARP into an off-screen texture and reads it back.
    #[test]
    fn draws_a_lit_model() {
        let gpu = Gpu::new(Adapter::Warp, 64, 64).expect("warp");
        let mesh = cube();
        let gm = GpuMesh::upload(&gpu.device, &mesh).unwrap();
        let cam = Orbit::fit(&mesh);
        let (w, h) = (160u32, 120u32);
        let tex = |usage, bind: u32, cpu: u32| {
            let mut t = None;
            unsafe {
                gpu.device
                    .CreateTexture2D(
                        &D3D11_TEXTURE2D_DESC {
                            Width: w,
                            Height: h,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            SampleDesc: DXGI_SAMPLE_DESC {
                                Count: 1,
                                Quality: 0,
                            },
                            Usage: usage,
                            BindFlags: bind,
                            CPUAccessFlags: cpu,
                            ..Default::default()
                        },
                        None,
                        Some(&mut t),
                    )
                    .unwrap();
            }
            t.unwrap()
        };
        let target = tex(D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0);
        let mut r = ModelRenderer::new(&gpu.device).unwrap();
        let d = ModelDraw {
            mesh: &gm,
            view_proj: cam.view_proj(w as f32 / h as f32, mesh.radius()),
            eye: cam.eye(),
            light: [0.3, 0.8, 0.5],
            limit: None,
            wire: false,
            section: None,
        };
        r.draw(
            &gpu.device,
            &gpu.context,
            &target,
            (w, h),
            [0.0, 0.0, 0.0, 1.0],
            &d,
        )
        .unwrap();
        let staging = tex(D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32);
        unsafe {
            gpu.context.CopyResource(&staging, &target);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .unwrap();
            let px = |x: u32, y: u32| {
                let p = (m.pData as *const u8).add((y * m.RowPitch + x * 4) as usize);
                std::slice::from_raw_parts(p, 4).to_vec()
            };
            let centre = px(w / 2, h / 2);
            let corner = px(1, 1);
            gpu.context.Unmap(&staging, 0);
            // The cube covers the centre, lit (not black); the corner is
            // the clear colour.
            assert!(centre[0] > 40 && centre[3] == 255, "{centre:?}");
            assert_eq!(&corner[..3], &[0, 0, 0], "{corner:?}");
        }
    }
}

#[cfg(test)]
mod preview {
    use super::*;
    use crate::gfx::{Adapter, Gpu};
    use crate::model::raster::Orbit;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_USAGE_STAGING,
    };

    /// Renders the generated sample models to PNGs in
    /// LIGHTBOX_PREVIEW_OUT (a folder) for a look at the shading.
    #[test]
    #[ignore = "writes preview images"]
    fn render_samples() {
        let Ok(out) = std::env::var("LIGHTBOX_PREVIEW_OUT") else {
            return;
        };
        let gpu = Gpu::new(Adapter::Warp, 64, 64).expect("warp");
        let (w, h) = (800u32, 600u32);
        let names = ["torus.stl", "cube.obj", "boxes.3mf"];
        // Each plain and with the tools: wireframe and a cut at half height.
        for (name, tools) in names.into_iter().flat_map(|n| [(n, false), (n, true)]) {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("testdata")
                .join(name);
            let Ok(mesh) = crate::model::load(&path) else {
                continue;
            };
            let gm = GpuMesh::upload(&gpu.device, &mesh).unwrap();
            let cam = Orbit::fit(&mesh);
            let make = |usage, bind: u32, cpu: u32| {
                let mut t = None;
                unsafe {
                    gpu.device
                        .CreateTexture2D(
                            &D3D11_TEXTURE2D_DESC {
                                Width: w,
                                Height: h,
                                MipLevels: 1,
                                ArraySize: 1,
                                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                                SampleDesc: DXGI_SAMPLE_DESC {
                                    Count: 1,
                                    Quality: 0,
                                },
                                Usage: usage,
                                BindFlags: bind,
                                CPUAccessFlags: cpu,
                                ..Default::default()
                            },
                            None,
                            Some(&mut t),
                        )
                        .unwrap();
                }
                t.unwrap()
            };
            let target = make(D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0);
            let mut r = ModelRenderer::new(&gpu.device).unwrap();
            let (x, y, z) = cam.axes();
            let light = crate::model::normalize([
                -0.45 * x[0] + 0.8 * y[0] + 0.55 * z[0],
                -0.45 * x[1] + 0.8 * y[1] + 0.55 * z[1],
                -0.45 * x[2] + 0.8 * y[2] + 0.55 * z[2],
            ]);
            let d = ModelDraw {
                mesh: &gm,
                view_proj: cam.view_proj(w as f32 / h as f32, mesh.radius()),
                eye: cam.eye(),
                light,
                limit: None,
                wire: tools,
                section: tools.then(|| mesh.center()[1]),
            };
            // The windowed background (#2B2B2B).
            r.draw(
                &gpu.device,
                &gpu.context,
                &target,
                (w, h),
                [0.0242, 0.0242, 0.0242, 1.0],
                &d,
            )
            .unwrap();
            let staging = make(D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32);
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            unsafe {
                gpu.context.CopyResource(&staging, &target);
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                gpu.context
                    .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                    .unwrap();
                for yy in 0..h {
                    let row = std::slice::from_raw_parts(
                        (m.pData as *const u8).add((yy * m.RowPitch) as usize),
                        (w * 4) as usize,
                    );
                    for p in row.as_chunks::<4>().0 {
                        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
                    }
                }
                gpu.context.Unmap(&staging, 0);
            }
            let file = std::fs::File::create(
                std::path::Path::new(&out)
                    .join(format!("{name}{}.png", if tools { "-tools" } else { "" })),
            )
            .unwrap();
            let mut e = png::Encoder::new(std::io::BufWriter::new(file), w, h);
            e.set_color(png::ColorType::Rgb);
            e.write_header().unwrap().write_image_data(&rgb).unwrap();
        }
    }
}
