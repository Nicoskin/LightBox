//! AVIF: the container is parsed by `avif-parse`, the AV1 bitstream decoded
//! by `rav1d` (the Rust port of dav1d, with its SIMD assembly), YUV → RGB
//! here. Alpha comes from the auxiliary alpha image.

use std::ptr::NonNull;
use std::thread;

use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_MC_BT709, DAV1D_MC_BT2020_NCL, DAV1D_MC_IDENTITY, DAV1D_PIXEL_LAYOUT_I400,
    DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};

use super::{DecodeError, Raw, RawImage};

/// `-EAGAIN`: the decoder needs more data / has a picture pending.
const AGAIN: i32 = -11;

/// One decoded plane set, copied out of the decoder.
struct Frame {
    w: usize,
    h: usize,
    bpc: u32,
    /// 0 = 4:0:0, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4.
    layout: u32,
    matrix: u32,
    full_range: bool,
    /// Samples as u16 (8-bit data widened), planes Y, U, V.
    planes: [Vec<u16>; 3],
    plane_w: [usize; 3],
}

/// Decodes one AV1 still picture.
fn decode_av1(obu: &[u8]) -> Result<Frame, DecodeError> {
    let err = |what: &str, r: i32| DecodeError::Other(format!("AV1: {what} ({r})"));
    // SAFETY: the dav1d C API is used as documented: settings are
    // initialised by dav1d_default_settings, the context is closed and the
    // picture / data unreferenced on every path, and picture planes are read
    // within their reported dimensions and strides.
    unsafe {
        let mut s = std::mem::MaybeUninit::<Dav1dSettings>::uninit();
        dav1d_default_settings(NonNull::new_unchecked(s.as_mut_ptr()));
        let mut s = s.assume_init();
        s.n_threads = thread::available_parallelism().map_or(4, |n| n.get().min(16)) as i32;
        s.max_frame_delay = 1;

        let mut ctx: Option<Dav1dContext> = None;
        let r = dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(&mut s)));
        if r.0 != 0 || ctx.is_none() {
            return Err(err("open", r.0));
        }
        let close = |mut c: Option<Dav1dContext>| dav1d_close(Some(NonNull::from(&mut c)));

        let mut data = Dav1dData::default();
        let buf = dav1d_data_create(Some(NonNull::from(&mut data)), obu.len());
        if buf.is_null() {
            close(ctx);
            return Err(err("alloc", 0));
        }
        std::ptr::copy_nonoverlapping(obu.as_ptr(), buf, obu.len());

        let mut pic = Dav1dPicture::default();
        let mut got = false;
        for _ in 0..64 {
            if data.sz > 0 {
                let r = dav1d_send_data(ctx, Some(NonNull::from(&mut data)));
                if r.0 != 0 && r.0 != AGAIN {
                    dav1d_data_unref(Some(NonNull::from(&mut data)));
                    close(ctx);
                    return Err(err("send", r.0));
                }
            }
            let r = dav1d_get_picture(ctx, Some(NonNull::from(&mut pic)));
            if r.0 == 0 {
                got = true;
                break;
            }
            if r.0 != AGAIN {
                break;
            }
        }
        if data.sz > 0 {
            dav1d_data_unref(Some(NonNull::from(&mut data)));
        }
        if !got {
            close(ctx);
            return Err(err("no picture", 0));
        }

        let (w, h) = (pic.p.w.max(0) as usize, pic.p.h.max(0) as usize);
        let bpc = pic.p.bpc as u32;
        let layout = match pic.p.layout {
            DAV1D_PIXEL_LAYOUT_I400 => 0,
            DAV1D_PIXEL_LAYOUT_I420 => 1,
            DAV1D_PIXEL_LAYOUT_I422 => 2,
            _ => 3,
        };
        let (matrix, full_range) = match pic.seq_hdr {
            Some(sh) => (sh.as_ref().mtrx, sh.as_ref().color_range != 0),
            None => (DAV1D_MC_BT709, false),
        };
        let cw = if layout == 1 || layout == 2 {
            w.div_ceil(2)
        } else {
            w
        };
        let ch = if layout == 1 { h.div_ceil(2) } else { h };
        let dims = [(w, h), (cw, ch), (cw, ch)];
        let mut planes: [Vec<u16>; 3] = Default::default();
        for (i, plane) in planes.iter_mut().enumerate() {
            if layout == 0 && i > 0 {
                break;
            }
            let Some(base) = pic.data[i] else { continue };
            let stride = pic.stride[usize::from(i > 0)];
            let (pw, ph) = dims[i];
            plane.reserve(pw * ph);
            for y in 0..ph {
                let row = base.as_ptr().cast::<u8>().offset(y as isize * stride);
                if bpc == 8 {
                    let s = std::slice::from_raw_parts(row, pw);
                    plane.extend(s.iter().map(|&v| u16::from(v)));
                } else {
                    let s = std::slice::from_raw_parts(row.cast::<u16>(), pw);
                    plane.extend_from_slice(s);
                }
            }
        }
        dav1d_picture_unref(Some(NonNull::from(&mut pic)));
        close(ctx);
        Ok(Frame {
            w,
            h,
            bpc,
            layout,
            matrix,
            full_range,
            planes,
            plane_w: [w, cw, cw],
        })
    }
}

/// YUV → straight RGBA16 (scaled to 16 bits), in bands across threads.
fn to_rgba16(f: &Frame, alpha: Option<&Frame>, premultiplied: bool) -> Vec<u16> {
    let (kr, kb) = match f.matrix {
        DAV1D_MC_BT709 => (0.2126, 0.0722),
        DAV1D_MC_BT2020_NCL => (0.2627, 0.0593),
        _ => (0.299, 0.114), // BT.601 and unspecified
    };
    let kg = 1.0 - kr - kb;
    let max = ((1u32 << f.bpc) - 1) as f32;
    let s = (1u32 << (f.bpc - 8)) as f32;
    let (y_off, y_scale, c_scale) = if f.full_range {
        (0.0, 1.0 / max, 1.0 / max)
    } else {
        (16.0 * s, 1.0 / (219.0 * s), 1.0 / (224.0 * s))
    };
    let half = (1u32 << (f.bpc - 1)) as f32;
    let identity = f.matrix == DAV1D_MC_IDENTITY;
    let (sx, sy) = match f.layout {
        1 => (1, 1),
        2 => (1, 0),
        _ => (0, 0),
    };
    let mut out = vec![0u16; f.w * f.h * 4];
    let threads = thread::available_parallelism().map_or(4, |n| n.get().min(16));
    let rows = f.h.div_ceil(threads).max(1);
    thread::scope(|scope| {
        for (bi, band) in out.chunks_mut(rows * f.w * 4).enumerate() {
            scope.spawn(move || {
                for (r, line) in band.chunks_mut(f.w * 4).enumerate() {
                    let y = bi * rows + r;
                    for x in 0..f.w {
                        let yv = f32::from(f.planes[0][y * f.plane_w[0] + x]);
                        let (u, v) = if f.layout == 0 {
                            (half, half)
                        } else {
                            let ci = (y >> sy) * f.plane_w[1] + (x >> sx);
                            (f32::from(f.planes[1][ci]), f32::from(f.planes[2][ci]))
                        };
                        let (rr, gg, bb) = if identity {
                            // GBR stored in Y, U, V.
                            (v / max, yv / max, u / max)
                        } else {
                            let yy = (yv - y_off) * y_scale;
                            let cb = (u - half) * c_scale;
                            let cr = (v - half) * c_scale;
                            let rr = yy + (2.0 - 2.0 * kr) * cr;
                            let bb = yy + (2.0 - 2.0 * kb) * cb;
                            (rr, (yy - kr * rr - kb * bb) / kg, bb)
                        };
                        let a = alpha.map_or(1.0, |a| {
                            let v = f32::from(a.planes[0][y * a.plane_w[0] + x]);
                            let amax = ((1u32 << a.bpc) - 1) as f32;
                            if a.full_range {
                                v / amax
                            } else {
                                let s = (1u32 << (a.bpc - 8)) as f32;
                                (v - 16.0 * s) / (219.0 * s)
                            }
                            .clamp(0.0, 1.0)
                        });
                        let un = |c: f32| {
                            let c = if premultiplied && a > 0.0 { c / a } else { c };
                            (c.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
                        };
                        let o = x * 4;
                        line[o] = un(rr);
                        line[o + 1] = un(gg);
                        line[o + 2] = un(bb);
                        line[o + 3] = (a * 65535.0 + 0.5) as u16;
                    }
                }
            });
        }
    });
    out
}

pub fn decode(data: &[u8]) -> Result<RawImage, DecodeError> {
    let avif = avif_parse::read_avif(&mut std::io::Cursor::new(data))
        .map_err(|e| DecodeError::Other(format!("AVIF: {e}")))?;
    let color = decode_av1(&avif.primary_item)?;
    let alpha = match &avif.alpha_item {
        Some(a) => decode_av1(a)
            .ok()
            .filter(|a| a.w == color.w && a.h == color.h),
        None => None,
    };
    let rgba = to_rgba16(&color, alpha.as_ref(), avif.premultiplied_alpha);
    let (w, h) = (color.w as u32, color.h as u32);
    let data = if color.bpc == 8 && alpha.as_ref().is_none_or(|a| a.bpc == 8) {
        Raw::Rgba8(rgba.iter().map(|&v| (v >> 8) as u8).collect())
    } else {
        Raw::Rgba16(rgba)
    };
    crate::bench::mark("decoded (avif)");
    Ok(RawImage::new(w, h, data))
}
