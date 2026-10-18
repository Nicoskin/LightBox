//! Windows Imaging Component: the fallback for formats with a system codec
//! — HEIC/HEIF (HEVC Video Extensions), AVIF (AV1 Video Extension) if our
//! own decoder fails, JPEG XR, RAW via the Raw Image Extension, and
//! anything else a codec pack installs.

use std::path::Path;

use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapFrameDecode, IWICColorContext,
    IWICImagingFactory, IWICPalette, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
    WICColorContextProfile, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantClear, PropVariantToUInt16,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::{HSTRING, w};

use super::{DecodeError, Raw, RawImage};
use crate::orientation::Orientation;

fn err(e: windows::core::Error) -> DecodeError {
    DecodeError::Other(format!("WIC: {}", e.message()))
}

pub fn decode(path: &Path) -> Result<RawImage, DecodeError> {
    // SAFETY: COM calls on this thread (initialised here, MTA, harmless if
    // already initialised); buffers passed to CopyPixels / GetProfileBytes
    // are sized from the values WIC reports.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        let decoder = factory
            .CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(err)?;
        let frame = decoder.GetFrame(0).map_err(err)?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).map_err(err)?;
        if w == 0 || h == 0 {
            return Err(DecodeError::Unsupported);
        }

        let converter = factory.CreateFormatConverter().map_err(err)?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None::<&IWICPalette>,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(err)?;
        let stride = w as usize * 4;
        let mut buf = vec![0u8; stride * h as usize];
        converter
            .CopyPixels(std::ptr::null(), stride as u32, &mut buf)
            .map_err(err)?;

        let mut img = RawImage::new(w, h, Raw::Rgba8(buf));
        img.orientation = orientation(&frame);
        img.icc = icc(&factory, &frame);
        crate::bench::mark("decoded (wic)");
        Ok(img)
    }
}

/// EXIF-style orientation from the frame's metadata, if any.
unsafe fn orientation(frame: &IWICBitmapFrameDecode) -> Orientation {
    // SAFETY: plain COM calls; the PROPVARIANT is cleared after use.
    unsafe {
        let Ok(reader) = frame.GetMetadataQueryReader() else {
            return Orientation::IDENTITY;
        };
        let mut value = PROPVARIANT::default();
        let o = reader
            .GetMetadataByName(w!("System.Photo.Orientation"), &mut value)
            .ok()
            .and_then(|()| PropVariantToUInt16(&value).ok());
        let _ = PropVariantClear(&mut value);
        o.map_or(Orientation::IDENTITY, |v| {
            Orientation::from_exif(u32::from(v))
        })
    }
}

/// Embedded ICC profile, if the codec exposes one.
unsafe fn icc(factory: &IWICImagingFactory, frame: &IWICBitmapFrameDecode) -> Option<Vec<u8>> {
    // SAFETY: plain COM calls with correctly sized buffers.
    unsafe {
        let ctx = factory.CreateColorContext().ok()?;
        let mut contexts: [Option<IWICColorContext>; 1] = [Some(ctx)];
        let mut count = 0u32;
        frame.GetColorContexts(&mut contexts, &mut count).ok()?;
        if count == 0 {
            return None;
        }
        let ctx = contexts[0].as_ref()?;
        if ctx.GetType().ok()? != WICColorContextProfile {
            return None;
        }
        let mut size = 0u32;
        ctx.GetProfileBytes(&mut [], &mut size).ok()?;
        let mut bytes = vec![0u8; size as usize];
        ctx.GetProfileBytes(&mut bytes, &mut size).ok()?;
        Some(bytes)
    }
}
