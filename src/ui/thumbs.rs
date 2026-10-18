//! Thumbnails for the filmstrip.
//!
//! Taken from the Windows thumbnail cache first (`IShellItemImageFactory`):
//! Explorer has usually generated them already, so this is nearly free.
//! On a miss, the image is decoded with our own (DCT-scaled) decoders.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_RESIZETOFIT,
    SIIGBF_THUMBNAILONLY,
};
use windows::core::HSTRING;

use crate::cache;
use crate::decode::{self, SizeHint, scale};
use crate::encode;
use crate::gfx::Thumb;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct State {
    queue: VecDeque<PathBuf>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

pub struct Thumbnails {
    shared: Arc<Shared>,
    results: Receiver<(String, Option<Thumb>)>,
    /// `None`: no thumbnail could be made for the file.
    done: HashMap<String, Option<Rc<Thumb>>>,
}

impl Thumbnails {
    /// `size`: longest side in pixels. `notify` wakes the UI thread.
    pub fn new(size: u32, notify: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                shutdown: false,
            }),
            wake: Condvar::new(),
        });
        let (tx, results) = channel();
        let worker_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || worker(&worker_shared, &tx, size, &notify));
        if let Err(e) = spawned {
            tracing::error!("cannot start thumbnail thread: {e}");
        }
        Self {
            shared,
            results,
            done: HashMap::new(),
        }
    }

    /// Asks for thumbnails of `paths` (most important first), replacing
    /// earlier requests that have not started yet.
    pub fn request(&self, paths: &[PathBuf]) {
        let Ok(mut st) = self.shared.state.lock() else {
            return;
        };
        st.queue = paths
            .iter()
            .filter(|p| !self.done.contains_key(&cache::key(p)))
            .cloned()
            .collect();
        drop(st);
        self.shared.wake.notify_all();
    }

    /// Takes finished thumbnails; `true` if there were any.
    pub fn receive(&mut self) -> bool {
        let mut any = false;
        while let Ok((key, thumb)) = self.results.try_recv() {
            self.done.insert(key, thumb.map(Rc::new));
            any = true;
        }
        any
    }

    /// Forgets a thumbnail so it is made again (the file changed).
    pub fn forget(&mut self, path: &Path) {
        self.done.remove(&cache::key(path));
    }

    pub fn get(&self, path: &Path) -> Option<Rc<Thumb>> {
        self.done.get(&cache::key(path)).cloned().flatten()
    }
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        if let Ok(mut st) = self.shared.state.lock() {
            st.shutdown = true;
        }
        self.shared.wake.notify_all();
    }
}

fn worker(shared: &Shared, tx: &Sender<(String, Option<Thumb>)>, size: u32, notify: &dyn Fn()) {
    // SAFETY: initialises COM for this thread only.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    loop {
        let path = {
            let Ok(mut st) = shared.state.lock() else {
                return;
            };
            loop {
                if st.shutdown {
                    return;
                }
                if let Some(p) = st.queue.pop_front() {
                    break p;
                }
                st = match shared.wake.wait(st) {
                    Ok(s) => s,
                    Err(_) => return,
                };
            }
        };
        let thumb = shell_thumbnail(&path, size).or_else(|| decoded_thumbnail(&path, size));
        if tx.send((cache::key(&path), thumb)).is_err() {
            return;
        }
        notify();
    }
}

/// Thumbnail from the Windows cache (generating it if the shell can).
fn shell_thumbnail(path: &Path, size: u32) -> Option<Thumb> {
    // SAFETY: COM is initialised on this thread; the HBITMAP is owned by us
    // and deleted below.
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
        let hbmp = factory
            .GetImage(
                SIZE {
                    cx: size as i32,
                    cy: size as i32,
                },
                SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK | SIIGBF_THUMBNAILONLY,
            )
            .ok()?;
        let thumb = bitmap_pixels(hbmp);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        thumb
    }
}

/// Copies a 32-bpp HBITMAP into premultiplied BGRA, top-down.
unsafe fn bitmap_pixels(hbmp: HBITMAP) -> Option<Thumb> {
    let mut bm = BITMAP::default();
    // SAFETY: valid bitmap handle and out-pointer of the right size.
    let got = unsafe {
        GetObjectW(
            HGDIOBJ(hbmp.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(std::ptr::from_mut(&mut bm).cast()),
        )
    };
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth as u32, bm.bmHeight as u32);
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    // SAFETY: the buffer holds exactly h rows of w 32-bit pixels.
    let lines = unsafe {
        let dc = GetDC(None);
        let n = GetDIBits(
            dc,
            hbmp,
            0,
            h,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        n
    };
    if lines != h as i32 {
        return None;
    }
    // Bitmaps without an alpha channel come back with alpha = 0.
    if bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        for p in bgra.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
    }
    Some(Thumb {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        width: w,
        height: h,
        bgra,
    })
}

/// Fallback: decode a reduced image ourselves, shrink and orient it.
fn decoded_thumbnail(path: &Path, size: u32) -> Option<Thumb> {
    let hint = SizeHint {
        box_width: size,
        box_height: size,
    };
    let img = decode::decode_file(path, Some(hint)).ok()?;
    let (tw, th) = scale::fit_size(img.width, img.height, size);
    let small = scale::rgba8(&img.to_straight_rgba8(), img.width, img.height, tw, th);
    let (rgba, w, h) = encode::orient_pixels(&small, tw as usize, th as usize, 4, img.orientation);
    // D2D wants premultiplied BGRA.
    let mut bgra = rgba;
    for p in bgra.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        let pm = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
        *p = [pm(p[2]), pm(p[1]), pm(p[0]), p[3]];
    }
    Some(Thumb {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        width: w as u32,
        height: h as u32,
        bgra,
    })
}
