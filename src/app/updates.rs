//! Update checks at start-up, in the settings, and from the tray.
//! Asks before downloading; the installer then replaces the
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

enum UpdateEvent {
    /// Check finished: a newer release, or none; `manual` reports either.
    Checked(Result<Option<Release>, String>),
    /// Installer downloaded and verified.
    Downloaded(Result<PathBuf, String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateTarget {
    Automatic,
    Settings,
    About,
}

pub(super) struct UpdateRx {
    receiver: Receiver<UpdateEvent>,
    target: UpdateTarget,
}

pub(super) struct About {
    pub(super) status: String,
    pub(super) release: Option<Release>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl App {
    pub(super) fn open_about(&mut self) -> Result<()> {
        if self.about.is_some() {
            self.bring_to_front();
            return Ok(());
        }
        self.open_tray_card(Some(About {
            status: tr("Проверка обновлений…").into(),
            release: None,
        }))?;
        self.check_updates(UpdateTarget::About)
    }

    pub(super) fn check_updates_from_card(&mut self) -> Result<()> {
        if self.standalone_card && self.about.is_none() {
            return self.open_about();
        }
        let Some(about) = &mut self.about else {
            return self.check_updates(UpdateTarget::Settings);
        };
        if let Some(release) = about.release.take() {
            self.download_update(release, UpdateTarget::About)
        } else {
            self.check_updates(UpdateTarget::About)
        }
    }

    /// At start-up: check if it is time to.
    pub(super) fn auto_update_check(&mut self) {
        if update::REPO.is_none()
            || !self.settings.check_updates
            || self.pinned
            || now().saturating_sub(self.settings.last_update_check) < DAY
        {
            return;
        }
        if let Err(e) = self.check_updates(UpdateTarget::Automatic) {
            tracing::error!("update check: {e:#}");
        }
    }

    /// Starts a check in the background.
    fn check_updates(&mut self, target: UpdateTarget) -> Result<()> {
        if let Some(pending) = &mut self.update_rx {
            if target != UpdateTarget::Automatic {
                pending.target = target;
            }
            return self.update_status(
                target,
                tr("Проверка или загрузка обновления уже выполняется"),
            );
        }
        let Some(repo) = update::REPO else {
            return self
                .update_status(target, tr("Проверка обновлений не настроена в этой сборке"));
        };
        let (tx, rx) = channel();
        let hwnd = self.hwnd.0 as isize;
        let spawned = thread::Builder::new().name("update".into()).spawn(move || {
            let result = update::check(repo).map_err(|e| format!("{e:#}"));
            if tx.send(UpdateEvent::Checked(result)).is_ok() {
                post(hwnd, WM_APP_UPDATE);
            }
        });
        if let Err(e) = spawned {
            return if target != UpdateTarget::Automatic {
                self.update_status(target, &t!("Не удалось проверить обновления: {}", e))
            } else {
                Err(e.into())
            };
        }
        self.update_rx = Some(UpdateRx {
            receiver: rx,
            target,
        });
        self.settings.last_update_check = now();
        self.settings.save();
        self.update_status(target, tr("Проверка обновлений…"))
    }

    fn update_status(&mut self, target: UpdateTarget, message: &str) -> Result<()> {
        match target {
            UpdateTarget::About => {
                if let Some(about) = &mut self.about {
                    message.clone_into(&mut about.status);
                    self.render()?;
                }
            }
            UpdateTarget::Settings if !self.hidden => {
                self.notice(message);
                self.render()?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn on_update(&mut self) -> Result<()> {
        let Some(pending) = &self.update_rx else {
            return Ok(());
        };
        let Ok(event) = pending.receiver.try_recv() else {
            return Ok(());
        };
        let target = pending.target;
        self.update_rx = None;
        match event {
            UpdateEvent::Checked(result) => match result {
                Ok(Some(release)) if target == UpdateTarget::About => {
                    if let Some(about) = &mut self.about {
                        about.status = t!("Доступна версия {}", release.version);
                        about.release = Some(release);
                        self.render()?;
                    }
                    Ok(())
                }
                Ok(Some(release)) => self.offer_update(release),
                Ok(None) => self.update_status(
                    target,
                    &t!("Установлена последняя версия ({})", update::VERSION),
                ),
                Err(e) => self.update_status(target, &t!("Не удалось проверить обновления: {}", e)),
            },
            UpdateEvent::Downloaded(Ok(setup)) => {
                let reopen = (!self.hidden && !self.current.as_os_str().is_empty())
                    .then(|| self.current.clone());
                if let Err(e) = update::run_installer(&setup, reopen.as_deref()) {
                    return self.update_status(
                        target,
                        &t!("Не удалось запустить установщик: {}", format!("{e:#}")),
                    );
                }
                // The installer closes this window itself; leave now.
                self.quit();
                Ok(())
            }
            UpdateEvent::Downloaded(Err(e)) => {
                self.update_status(target, &t!("Обновление не загружено: {}", e))
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
        self.download_update(release, UpdateTarget::Settings)
    }

    fn download_update(&mut self, release: Release, target: UpdateTarget) -> Result<()> {
        self.leave_current();
        let (tx, rx) = channel();
        let hwnd = self.hwnd.0 as isize;
        let spawned = thread::Builder::new().name("update".into()).spawn(move || {
            let r = update::download(&release).map_err(|e| format!("{e:#}"));
            if tx.send(UpdateEvent::Downloaded(r)).is_ok() {
                post(hwnd, WM_APP_UPDATE);
            }
        });
        if let Err(e) = spawned {
            return self.update_status(target, &t!("Обновление не загружено: {}", e));
        }
        self.update_rx = Some(UpdateRx {
            receiver: rx,
            target,
        });
        self.update_status(target, tr("Загрузка обновления…"))
    }
}
