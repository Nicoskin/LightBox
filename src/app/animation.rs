//! Playback of animated GIF / WebP / APNG: frames are decoded in the
//! background once the first one is on screen, then swapped into the same
//! texture on a timer. Space pauses, "," and "." step frame by frame.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use anyhow::Result;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::{ANIM_TIMER, App, WM_APP_FRAMES, post};
use crate::cache;
use crate::decode::anim::{self, Animation};

pub(super) struct Playback {
    key: String,
    anim: Arc<Animation>,
    pub index: usize,
    pub playing: bool,
}

/// Frames being decoded for the file with this cache key.
pub(super) type FramesRx = (String, Receiver<Arc<Animation>>);

/// Only these can be animated; a quick look at the header avoids reading
/// every still PNG / WebP twice.
fn maybe_animated(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "gif" {
        return true;
    }
    if !matches!(ext.as_str(), "png" | "apng" | "webp") {
        return false;
    }
    let mut head = vec![0u8; 64 * 1024];
    let n = std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .unwrap_or(0);
    let head = &head[..n];
    let has = |tag: &[u8]| head.windows(tag.len()).any(|w| w == tag);
    has(b"acTL") || has(b"ANIM")
}

impl App {
    /// The current file is on screen: decode its other frames if it is an
    /// animation.
    pub(super) fn start_animation(&mut self) {
        let key = cache::key(&self.current);
        let known = self.playback.as_ref().is_some_and(|p| p.key == key)
            || self.frames_rx.as_ref().is_some_and(|(k, _)| *k == key);
        if known {
            return;
        }
        self.stop_animation();
        let path = self.current.clone();
        if !maybe_animated(&path) {
            return;
        }
        let (tx, rx) = channel();
        let hwnd = self.hwnd.0 as isize;
        let spawned = thread::Builder::new().name("frames".into()).spawn(move || {
            let result = std::fs::read(&path)
                .ok()
                .and_then(|d| anim::decode(&d).ok().flatten());
            if let Some(a) = result
                && tx.send(Arc::new(a)).is_ok()
            {
                post(hwnd, WM_APP_FRAMES);
            }
        });
        if spawned.is_ok() {
            self.frames_rx = Some((key, rx));
        }
    }

    pub(super) fn stop_animation(&mut self) {
        if self.playback.take().is_some() {
            // SAFETY: own window and timer.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
            }
        }
        self.frames_rx = None;
    }

    pub(super) fn on_frames(&mut self) -> Result<()> {
        let Some((key, rx)) = &self.frames_rx else {
            return Ok(());
        };
        let Ok(anim) = rx.try_recv() else {
            return Ok(());
        };
        let key = key.clone();
        self.frames_rx = None;
        if key != cache::key(&self.current) {
            return Ok(());
        }
        // The texture on screen must be this image at full size.
        let fits = self.shown.as_ref().is_some_and(|(k, t)| {
            *k == key && t.width == anim.width && t.height == anim.height && t.tiles.len() == 1
        });
        if !fits {
            return Ok(());
        }
        self.playback = Some(Playback {
            key,
            anim,
            index: 0,
            playing: true,
        });
        self.arm_frame_timer();
        self.render()
    }

    fn arm_frame_timer(&self) {
        if let Some(p) = &self.playback
            && p.playing
        {
            let ms = p.anim.frames[p.index].delay_ms;
            // SAFETY: own window; re-arming the id resets it.
            unsafe { SetTimer(Some(self.hwnd), ANIM_TIMER, ms, None) };
        }
    }

    /// Shows frame `index` of the running animation.
    fn show_frame(&mut self, index: usize) -> Result<()> {
        let Some(p) = &mut self.playback else {
            return Ok(());
        };
        p.index = index % p.anim.frames.len();
        let anim = Arc::clone(&p.anim);
        let i = p.index;
        if let Some((_, tex)) = &self.shown {
            tex.update(&self.gpu, &anim.frames[i].pixels)?;
        }
        self.render()
    }

    pub(super) fn frame_timer(&mut self) -> Result<()> {
        let Some(p) = &self.playback else {
            // SAFETY: own window and timer.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
            }
            return Ok(());
        };
        let next = p.index + 1;
        self.show_frame(next)?;
        self.arm_frame_timer();
        Ok(())
    }

    /// Space: pause / play. Returns `false` if nothing is animating.
    pub(super) fn toggle_playback(&mut self) -> Result<bool> {
        let Some(p) = &mut self.playback else {
            return Ok(false);
        };
        p.playing = !p.playing;
        let playing = p.playing;
        if playing {
            self.arm_frame_timer();
        } else {
            // SAFETY: own window and timer.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
            }
        }
        self.notice(if playing {
            "Воспроизведение"
        } else {
            "Пауза"
        });
        self.render()?;
        Ok(true)
    }

    /// "," / ".": one frame back / forward (pauses).
    pub(super) fn step_frame(&mut self, delta: isize) -> Result<()> {
        let Some(p) = &mut self.playback else {
            return Ok(());
        };
        p.playing = false;
        let n = p.anim.frames.len() as isize;
        let i = (p.index as isize + delta).rem_euclid(n) as usize;
        // SAFETY: own window and timer.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
        }
        self.show_frame(i)
    }

    /// "кадр 3 / 24" for the caption, while an animation is loaded.
    pub(super) fn frame_label(&self) -> Option<String> {
        let p = self.playback.as_ref()?;
        let pause = if p.playing { "" } else { " · пауза" };
        Some(format!(
            "кадр {} / {}{pause}",
            p.index + 1,
            p.anim.frames.len()
        ))
    }
}
