//! Slideshow (S or F5): fullscreen, the next photo every few seconds,
//! round and round; Esc, S or F5 stop it.

use anyhow::Result;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::{App, Mode, SLIDE_TIMER};
use crate::i18n::tr;
use crate::t;

impl App {
    pub(super) fn toggle_slideshow(&mut self) -> Result<()> {
        if self.slideshow {
            self.stop_slideshow();
            return self.render();
        }
        if self.files.len() < 2 || self.preview() {
            self.notice(tr("Для слайд-шоу нужна папка с несколькими фото"));
            return self.render();
        }
        self.slideshow = true;
        if self.mode == Mode::Windowed {
            self.go_fullscreen()?;
        }
        let secs = u32::from(self.settings.slideshow_secs.max(1));
        // SAFETY: own window; a periodic timer until killed.
        unsafe { SetTimer(Some(self.hwnd), SLIDE_TIMER, secs * 1000, None) };
        self.notice(&t!("Слайд-шоу · {} с · Esc — стоп", secs));
        self.render()
    }

    pub(super) fn stop_slideshow(&mut self) {
        if self.slideshow {
            self.slideshow = false;
            // SAFETY: own window and timer.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), SLIDE_TIMER);
            }
            self.notice(tr("Слайд-шоу остановлено"));
        }
    }

    pub(super) fn slide_timer(&mut self) -> Result<()> {
        if !self.slideshow || self.files.is_empty() {
            self.stop_slideshow();
            return Ok(());
        }
        let next = (self.index + 1) % self.files.len();
        self.fast_nav = false;
        self.go_to(next)
    }
}
