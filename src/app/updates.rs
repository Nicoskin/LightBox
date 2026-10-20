//! Update check (once a day at start-up, or "Проверить сейчас" in the
//! settings): asks before downloading; the installer then replaces the
//! program and starts it again.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

use super::{App, WM_APP_UPDATE, post};
use crate::i18n::tr;
use crate::t;
use crate::update::{self, Release};
use crate::win::dialogs;

/// Seconds between automatic checks.
const DAY: u64 = 24 * 60 * 60;
const ID_UPDATE: i32 = 1;

pub(super) enum UpdateEvent {
    /// Check finished: a newer release, or none; `manual` reports either.
    Checked {
        result: Result<Option<Release>, String>,
        manual: bool,
    },
    /// Installer downloaded and verified.
    Downloaded(Result<PathBuf, String>),
}

pub(super) type UpdateRx = Receiver<UpdateEvent>;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl App {
    /// At start-up: check if it is time to.
    pub(super) fn auto_update_check(&mut self) {
        if update::REPO.is_none()
            || !self.settings.check_updates
            || self.pinned
            || now().saturating_sub(self.settings.last_update_check) < DAY
        {
            return;
        }
        self.check_updates(false);
    }

    /// Starts a check in the background.
    pub(super) fn check_updates(&mut self, manual: bool) {
        let Some(repo) = update::REPO else {
            if manual {
                self.notice(tr("Проверка обновлений не настроена в этой сборке"));
            }
            return;
        };
        self.settings.last_update_check = now();
        self.settings.save();
        if manual {
            self.notice(tr("Проверка обновлений…"));
        }
        let (tx, rx) = channel();
        let hwnd = self.hwnd.0 as isize;
        let spawned = thread::Builder::new().name("update".into()).spawn(move || {
            let result = update::check(repo).map_err(|e| format!("{e:#}"));
            if tx.send(UpdateEvent::Checked { result, manual }).is_ok() {
                post(hwnd, WM_APP_UPDATE);
            }
        });
        if spawned.is_ok() {
            self.update_rx = Some(rx);
        }
    }

    pub(super) fn on_update(&mut self) -> Result<()> {
        let Some(event) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return Ok(());
        };
        self.update_rx = None;
        match event {
            UpdateEvent::Checked { result, manual } => match result {
                Ok(Some(release)) => self.offer_update(release),
                Ok(None) if manual => {
                    self.notice(&t!("Установлена последняя версия ({})", update::VERSION));
                    self.render()
                }
                Err(e) if manual => {
                    self.notice(&t!("Не удалось проверить обновления: {}", e));
                    self.render()
                }
                _ => Ok(()),
            },
            UpdateEvent::Downloaded(Ok(setup)) => {
                let reopen = (!self.hidden && !self.current.as_os_str().is_empty())
                    .then(|| self.current.clone());
                if let Err(e) = update::run_installer(&setup, reopen.as_deref()) {
                    self.notice(&t!("Не удалось запустить установщик: {}", format!("{e:#}")));
                    return self.render();
                }
                // The installer closes this window itself; leave now.
                self.quit();
                Ok(())
            }
            UpdateEvent::Downloaded(Err(e)) => {
                self.notice(&t!("Обновление не загружено: {}", e));
                self.render()
            }
        }
    }

    fn offer_update(&mut self, release: Release) -> Result<()> {
        let notes: String = release.notes.chars().take(600).collect();
        // The portable version is updated by hand: the download page.
        if crate::paths::portable() {
            let (choice, _) = dialogs::ask(
                self.hwnd,
                &t!("Доступна версия {}", release.version),
                &t!("Сейчас у вас {}.\n\n{}", update::VERSION, notes),
                &[(
                    ID_UPDATE,
                    tr(
                        "Открыть страницу загрузки\nПортативная версия обновляется вручную: замените файлы в папке",
                    ),
                )],
                None,
            );
            if choice == Some(ID_UPDATE)
                && let Some(repo) = update::REPO
            {
                crate::win::shell_open(&format!("https://github.com/{repo}/releases/latest"));
            }
            return Ok(());
        }
        let (choice, _) = dialogs::ask(
            self.hwnd,
            &t!("Доступна версия {}", release.version),
            &t!("Сейчас установлена {}.\n\n{}", update::VERSION, notes),
            &[(
                ID_UPDATE,
                tr("Обновить сейчас\nНастройки и ассоциации файлов сохранятся"),
            )],
            None,
        );
        if choice != Some(ID_UPDATE) {
            return Ok(());
        }
        self.leave_current();
        self.notice(tr("Загрузка обновления…"));
        let (tx, rx) = channel();
        let hwnd = self.hwnd.0 as isize;
        let spawned = thread::Builder::new().name("update".into()).spawn(move || {
            let r = update::download(&release).map_err(|e| format!("{e:#}"));
            if tx.send(UpdateEvent::Downloaded(r)).is_ok() {
                post(hwnd, WM_APP_UPDATE);
            }
        });
        if spawned.is_ok() {
            self.update_rx = Some(rx);
        }
        self.render()
    }
}
