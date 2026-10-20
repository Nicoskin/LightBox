//! Videos in the viewer: played by the media engine (`crate::video`) and
//! browsed like photos. Space pauses, Shift+←/→ seek 5 s, M mutes; the bar
//! above the panel shows the time and seeks on click.

use std::rc::Rc;

use anyhow::Result;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer, WM_LBUTTONDOWN, WM_LBUTTONUP};

use super::{App, VIDEO_TIMER, WM_APP_VIDEO, post_with};
use crate::cache;
use crate::gfx::{GpuImage, VideoBarView};
use crate::i18n::tr;
use crate::t;
use crate::video::{self, Event, Player};
use crate::view::Rect;

pub(super) struct Movie {
    pub key: String,
    player: Player,
}

/// Frame poll interval while playing, ms (below a 120 Hz frame).
const FRAME_MS: u32 = 8;

impl App {
    /// `true` while the current file is a video.
    pub(super) fn video_mode(&self) -> bool {
        video::is_video(&self.current)
    }

    fn movie(&self) -> Option<&Movie> {
        let key = cache::key(&self.current);
        self.movie.as_ref().filter(|m| m.key == key)
    }

    /// Shows the current file as a video, opening it if needed.
    pub(super) fn show_video(&mut self) -> Result<bool> {
        let key = cache::key(&self.current);
        if self.movie.as_ref().is_some_and(|m| m.key == key) {
            return Ok(true);
        }
        self.stop_video();
        self.stop_animation();
        self.shown = None;
        self.error = None;
        let hwnd = self.hwnd.0 as isize;
        match Player::open(&self.gpu.device, &self.current, move |e| {
            post_with(hwnd, WM_APP_VIDEO, e.code());
        }) {
            Ok(player) => {
                self.movie = Some(Movie { key, player });
                self.start_frames();
            }
            Err(e) => self.error = Some(t!("видео не воспроизводится: {}", format!("{e:#}"))),
        }
        Ok(true)
    }

    /// After a GPU switch: the engine writes frames on its own device, so
    /// the video is opened again on the new one, where it was.
    pub(super) fn reopen_video(&mut self) -> Result<()> {
        let Some(m) = self.movie.take() else {
            return Ok(());
        };
        let ((now, _), paused, muted) = (m.player.time(), m.player.paused(), m.player.muted());
        drop(m);
        let hwnd = self.hwnd.0 as isize;
        let player = Player::open(&self.gpu.device, &self.current, move |e| {
            post_with(hwnd, WM_APP_VIDEO, e.code());
        })?;
        if now > 0.0 {
            player.seek(now);
        }
        player.set_muted(muted);
        if paused {
            player.toggle_pause();
        }
        self.movie = Some(Movie {
            key: cache::key(&self.current),
            player,
        });
        self.start_frames();
        Ok(())
    }

    /// Leaving a video: stop its sound and threads.
    pub(super) fn stop_video(&mut self) {
        if self.movie.take().is_some() {
            // SAFETY: own window and timer.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), VIDEO_TIMER);
            }
        }
    }

    fn start_frames(&self) {
        // SAFETY: own window; re-arming a timer id resets it.
        unsafe { SetTimer(Some(self.hwnd), VIDEO_TIMER, FRAME_MS, None) };
    }

    /// The frame timer: copy a new frame, if any, and draw it.
    pub(super) fn video_tick(&mut self) -> Result<()> {
        let key = cache::key(&self.current);
        let Some(m) = self.movie.as_mut().filter(|m| m.key == key) else {
            self.stop_video();
            return Ok(());
        };
        let frame = m.player.frame(&self.gpu.device, &self.gpu.context)?;
        let paused = m.player.paused();
        if let Some((tex, srv, w, h)) = frame {
            let first = self.shown.is_none();
            self.shown = Some((key, Rc::new(GpuImage::from_texture(tex, srv, w, h))));
            if first {
                if self.preview() && !self.hidden && self.mode == super::Mode::Windowed {
                    self.fit_preview_window()?;
                }
                if let Some(v) = self.fit_view() {
                    self.view = v;
                    self.fitted = true;
                }
            }
            self.render()?;
        } else if paused {
            // Nothing moves until it plays again (or seeks).
            // SAFETY: own window and timer.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), VIDEO_TIMER);
            }
            self.render()?;
        }
        Ok(())
    }

    /// Events from the engine.
    pub(super) fn on_video(&mut self, code: usize) -> Result<()> {
        match Event::from_code(code) {
            Some(Event::Error) => {
                self.error =
                    Some(tr("видео не воспроизводится: нет кодека или файл повреждён").into());
                self.stop_video();
                self.render()
            }
            Some(Event::Loaded | Event::Ended) => {
                self.start_frames();
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Space: pause / play. `false` when the current file is not a video.
    pub(super) fn video_pause(&mut self) -> Result<bool> {
        let Some(m) = self.movie() else {
            return Ok(false);
        };
        m.player.toggle_pause();
        self.start_frames();
        self.render()?;
        Ok(true)
    }

    /// Shift+←/→: seek by `delta` seconds.
    pub(super) fn video_seek_by(&mut self, delta: f64) -> Result<bool> {
        let Some(m) = self.movie() else {
            return Ok(false);
        };
        let (now, len) = m.player.time();
        m.player.seek((now + delta).clamp(0.0, len.max(0.0)));
        self.start_frames();
        Ok(true)
    }

    /// M: sound on / off.
    pub(super) fn video_mute(&mut self) -> Result<bool> {
        let Some(m) = self.movie() else {
            return Ok(false);
        };
        let muted = !m.player.muted();
        m.player.set_muted(muted);
        self.notice(if muted {
            tr("Звук выключен")
        } else {
            tr("Звук включён")
        });
        self.render()?;
        Ok(true)
    }

    /// The bar above the panel: play button, track, time.
    fn video_bar(&self) -> Option<(Rect, Rect, Rect)> {
        self.movie()?;
        let s = self.ui_scale();
        let (w, _) = self.viewport();
        let bottom = self.viewport().1 - self.panel_height() - 8.0 * s;
        let h = 30.0 * s;
        let bar = Rect {
            x0: 16.0 * s,
            y0: bottom - h,
            x1: w - 16.0 * s,
            y1: bottom,
        };
        let button = Rect {
            x1: bar.x0 + h,
            ..bar
        };
        let track = Rect {
            x0: button.x1 + 10.0 * s,
            x1: bar.x1 - 130.0 * s,
            ..bar
        };
        Some((bar, button, track))
    }

    /// Clicks on the bar; `true` when consumed.
    pub(super) fn video_input(&mut self, msg: u32, p: (f64, f64)) -> Result<bool> {
        let Some((bar, button, track)) = self.video_bar() else {
            return Ok(false);
        };
        if !bar.contains(p) {
            return Ok(false);
        }
        if msg == WM_LBUTTONUP {
            if button.contains(p) {
                self.video_pause()?;
            } else if track.contains(p)
                && let Some(m) = self.movie()
            {
                let (_, len) = m.player.time();
                let f = ((p.0 - track.x0) / track.width()).clamp(0.0, 1.0);
                m.player.seek(f * len);
                self.start_frames();
            }
        }
        Ok(msg == WM_LBUTTONDOWN || msg == WM_LBUTTONUP)
    }

    pub(super) fn video_bar_view(&self) -> Option<VideoBarView> {
        let (bar, button, track) = self.video_bar()?;
        let m = self.movie()?;
        let (now, len) = m.player.time();
        Some(VideoBarView {
            bar,
            button,
            playing: !m.player.paused(),
            track,
            progress: if len > 0.0 {
                (now / len).clamp(0.0, 1.0)
            } else {
                0.0
            },
            time: video::time_text(now, len),
            muted: m.player.muted(),
        })
    }
}

impl App {
    /// The current video's size, once the engine knows it.
    pub(super) fn video_size(&self) -> Option<(f64, f64)> {
        let (w, h) = self.movie()?.player.size()?;
        Some((f64::from(w), f64::from(h)))
    }
}
