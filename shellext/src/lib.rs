//! Explorer thumbnails for 3D models (STL, OBJ, 3MF): a COM thumbnail
//! handler, `lightbox_thumbs.dll`. Explorer hands the file over as a stream
//! (in its isolated thumbnail host process) and gets back a premultiplied
//! 32-bit bitmap: the preview a slicer saved in a 3MF, else the mesh drawn
//! by the viewer's own rasteriser.
//!
//! The installer registers it for the current user by calling
//! `DllRegisterServer` (`regsvr32` works too, without administrator
//! rights); the viewer's `--uninstall` calls `DllUnregisterServer`.
//!
//! Explorer creates the handler in its isolated host through RPCSS, which
//! (seen on Windows 11 26300) does not see a per-user class registered
//! after sign-in: thumbnails appear after signing in again. In-process
//! (`DisableProcessIsolation`) they work at once, but a bad file could then
//! take Explorer down, so isolation stays.
//!
//! An OBJ's colours from its material library are not shown: a stream has
//! no folder to find the .mtl in.

#[allow(
    dead_code,
    reason = "the viewer's model code: only parsing and the rasteriser are used here"
)]
#[path = "../../src/model/mod.rs"]
mod model;
mod register;
mod thumb;

use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_OUTOFMEMORY, E_POINTER,
    E_UNEXPECTED, ERROR_ALREADY_INITIALIZED, ERROR_FILE_TOO_LARGE, ERROR_TIMEOUT, HMODULE, S_FALSE,
    S_OK,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP,
};
use windows::Win32::System::Com::{
    CoTaskMemFree, IClassFactory, IClassFactory_Impl, IStream, STATFLAG_DEFAULT, STATSTG,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::Win32::UI::Shell::PropertiesSystem::{
    IInitializeWithStream, IInitializeWithStream_Impl,
};
use windows::Win32::UI::Shell::{
    IThumbnailProvider, IThumbnailProvider_Impl, WTS_ALPHATYPE, WTSAT_ARGB,
};
use windows::core::{
    BOOL, Error, GUID, HRESULT, IUnknown, Interface as _, PCWSTR, Ref, Result, implement,
};

/// Longest a thumbnail may take; Explorer shows the generic icon instead.
const TIMEOUT: Duration = Duration::from_secs(20);

/// COM objects, server locks and render threads alive: the DLL may be
/// unloaded only when there are none.
static ALIVE: AtomicUsize = AtomicUsize::new(0);

struct Alive;

impl Alive {
    fn new() -> Self {
        ALIVE.fetch_add(1, Ordering::SeqCst);
        Alive
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        ALIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs `f`; a panic becomes `E_UNEXPECTED` instead of unwinding into COM.
fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(E_UNEXPECTED.into()))
}

fn hresult(r: Result<()>) -> HRESULT {
    r.map_or_else(|e| e.code(), |()| S_OK)
}

fn win32(e: windows::Win32::Foundation::WIN32_ERROR) -> Error {
    HRESULT::from_win32(e.0).into()
}

#[implement(IInitializeWithStream, IThumbnailProvider)]
struct Provider {
    file: RefCell<Option<(Arc<[u8]>, thumb::Kind)>>,
    _alive: Alive,
}

impl IInitializeWithStream_Impl for Provider_Impl {
    fn Initialize(&self, stream: Ref<IStream>, _mode: u32) -> Result<()> {
        guard(|| {
            if self.file.borrow().is_some() {
                return Err(win32(ERROR_ALREADY_INITIALIZED));
            }
            let (data, name) = read_stream(stream.ok()?)?;
            let kind = thumb::Kind::detect(name.as_deref(), &data);
            *self.file.borrow_mut() = Some((data.into(), kind));
            Ok(())
        })
    }
}

impl IThumbnailProvider_Impl for Provider_Impl {
    fn GetThumbnail(
        &self,
        cx: u32,
        phbmp: *mut HBITMAP,
        pdwalpha: *mut WTS_ALPHATYPE,
    ) -> Result<()> {
        guard(|| {
            if phbmp.is_null() || pdwalpha.is_null() {
                return Err(E_POINTER.into());
            }
            let (data, kind) = self
                .file
                .borrow()
                .clone()
                .ok_or(Error::from(E_UNEXPECTED))?;
            let bmp = hbitmap(&render(data, kind, cx)?)?;
            // SAFETY: both pointers were checked; COM's out-parameters.
            unsafe {
                *phbmp = bmp;
                *pdwalpha = WTSAT_ARGB;
            }
            Ok(())
        })
    }
}

/// The whole file and its name (if the stream has one).
fn read_stream(stream: &IStream) -> Result<(Vec<u8>, Option<String>)> {
    let mut stat = STATSTG::default();
    // SAFETY: `pwcsName` is allocated by the stream and freed here.
    let name = unsafe {
        let ok = stream.Stat(&mut stat, STATFLAG_DEFAULT).is_ok();
        let name = (ok && !stat.pwcsName.is_null()).then(|| stat.pwcsName.to_string().ok());
        if !stat.pwcsName.is_null() {
            CoTaskMemFree(Some(stat.pwcsName.0 as *const c_void));
        }
        name.flatten()
    };
    if stat.cbSize > thumb::MAX_FILE {
        return Err(win32(ERROR_FILE_TOO_LARGE));
    }
    let mut data = Vec::with_capacity(stat.cbSize as usize);
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let mut read = 0u32;
        // SAFETY: reads into `chunk`, whose length is passed.
        unsafe {
            stream.Read(
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                Some(&mut read),
            )
        }
        .ok()?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..read as usize]);
        if data.len() as u64 > thumb::MAX_FILE {
            return Err(win32(ERROR_FILE_TOO_LARGE));
        }
    }
    Ok((data, name))
}

/// Renders on a worker thread so that a slow file cannot hang Explorer's
/// host past [`TIMEOUT`]; the thread keeps the DLL loaded until it ends.
fn render(data: Arc<[u8]>, kind: thumb::Kind, cx: u32) -> Result<thumb::Image> {
    let (tx, rx) = mpsc::channel();
    let alive = Alive::new();
    std::thread::Builder::new()
        .name("lightbox thumbnail".into())
        .spawn(move || {
            let _alive = alive;
            let r = catch_unwind(AssertUnwindSafe(|| thumb::render(&data, kind, cx)));
            let _ = tx.send(r.unwrap_or_else(|_| Err("panic".into())));
        })
        .map_err(|_| Error::from(E_OUTOFMEMORY))?;
    match rx.recv_timeout(TIMEOUT) {
        Ok(Ok(img)) => Ok(img),
        Ok(Err(_)) => Err(E_FAIL.into()),
        Err(_) => Err(win32(ERROR_TIMEOUT)),
    }
}

/// A top-down 32-bit DIB section holding `img` (premultiplied BGRA).
fn hbitmap(img: &thumb::Image) -> Result<HBITMAP> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: img.width as i32,
            biHeight: -(img.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: `info` describes `img.bgra` exactly (32-bit rows need no
    // padding); the copy stays within the section's pixels.
    unsafe {
        let bmp = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        if bits.is_null() {
            let _ = DeleteObject(bmp.into());
            return Err(E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(img.bgra.as_ptr(), bits.cast::<u8>(), img.bgra.len());
        Ok(bmp)
    }
}

#[implement(IClassFactory)]
struct Factory {
    _alive: Alive,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        guard(|| {
            if ppvobject.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: checked above.
            unsafe { *ppvobject = std::ptr::null_mut() };
            if !outer.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let provider: IUnknown = Provider {
                file: RefCell::new(None),
                _alive: Alive::new(),
            }
            .into();
            // SAFETY: COM's riid / out-pointer pair.
            unsafe { provider.query(riid, ppvobject) }.ok()
        })
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            ALIVE.fetch_add(1, Ordering::SeqCst);
        } else {
            let _ = ALIVE.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        }
        Ok(())
    }
}

/// COM entry point: the class factory for [`register::CLSID`].
///
/// # Safety
/// The pointers are COM's: `ppv` writable, the GUIDs readable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    hresult(guard(|| {
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: the caller's pointers, checked for null.
        unsafe {
            *ppv = std::ptr::null_mut();
            if clsid.is_null() || *clsid != register::CLSID {
                return Err(CLASS_E_CLASSNOTAVAILABLE.into());
            }
            let factory: IClassFactory = Factory {
                _alive: Alive::new(),
            }
            .into();
            factory.query(iid, ppv).ok()
        }
    }))
}

/// COM entry point: whether the DLL may be unloaded.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if ALIVE.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

/// Registers this DLL as the thumbnail handler (current user only).
#[unsafe(no_mangle)]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    hresult(guard(|| register::register(&dll_path()?)))
}

/// Removes the registration and restores the handlers it replaced.
#[unsafe(no_mangle)]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    hresult(guard(register::unregister))
}

/// Full path of this DLL.
fn dll_path() -> Result<String> {
    let mut module = HMODULE::default();
    // SAFETY: an address inside this module; no reference is taken.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(dll_path as *const () as *const u16),
            &mut module,
        )?;
    }
    let mut buf = vec![0u16; 32768];
    // SAFETY: the buffer is valid for the call.
    let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(Error::from_thread());
    }
    Ok(String::from_utf16_lossy(&buf[..n]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{DIBSECTION, GetObjectW};

    #[test]
    fn bitmap_holds_the_pixels() {
        let img = thumb::Image::from_rgba(
            2,
            2,
            &[
                255, 0, 0, 255, /**/ 0, 255, 0, 128, //
                0, 0, 255, 0, /*  */ 255, 255, 255, 255,
            ],
        );
        let bmp = hbitmap(&img).unwrap();
        let mut ds = DIBSECTION::default();
        // SAFETY: a DIBSECTION-sized buffer; the bitmap is ours to delete.
        unsafe {
            let n = GetObjectW(
                bmp.into(),
                size_of::<DIBSECTION>() as i32,
                Some((&raw mut ds).cast()),
            );
            assert_eq!(n as usize, size_of::<DIBSECTION>());
            assert_eq!((ds.dsBm.bmWidth, ds.dsBm.bmHeight), (2, 2));
            assert_eq!(ds.dsBm.bmBitsPixel, 32);
            // Top-down: the first row comes first.
            let bits = std::slice::from_raw_parts(ds.dsBm.bmBits.cast::<u8>(), 16);
            assert_eq!(
                bits,
                [
                    0, 0, 255, 255, /**/ 0, 128, 0, 128, //
                    0, 0, 0, 0, /*    */ 255, 255, 255, 255,
                ]
            );
            let _ = DeleteObject(bmp.into());
        }
    }

    #[test]
    fn class_object_only_for_our_clsid() {
        let mut ppv = std::ptr::null_mut();
        let other = GUID::from_u128(1);
        // SAFETY: valid pointers.
        unsafe {
            let hr = DllGetClassObject(&other, &IClassFactory::IID, &mut ppv);
            assert_eq!(hr, CLASS_E_CLASSNOTAVAILABLE);
            let hr = DllGetClassObject(&register::CLSID, &IClassFactory::IID, &mut ppv);
            assert_eq!(hr, S_OK);
            let factory = IClassFactory::from_raw(ppv);
            let provider: IThumbnailProvider = factory.CreateInstance(None).unwrap();
            drop(provider);
        }
    }
}
