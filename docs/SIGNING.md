# Подпись релизов (SignPath Foundation)

Релизы Lightbox собирает GitHub Actions (`.github/workflows/release.yml`) по
тегу `v*`. Если настроен SignPath, workflow подписывает сначала `lightbox.exe`
(до того, как установщик его вошьёт), потом `lightbox-setup.exe`. Пока
секрета и переменных нет, релиз собирается **без подписи** и в прогоне
появляется предупреждение «Unsigned release».

SignPath Foundation бесплатно выдаёт сертификат открытым проектам. Подписать
можно только то, что собрано на GitHub-hosted раннерах из публичного
репозитория — поэтому сборка и идёт через Actions, а не локально.

---

## 1. Подготовка репозитория (до заявки)

Требования фонда (<https://signpath.org/terms>):

- лицензия из списка OSI, без коммерческой двойной лицензии — MIT подходит;
- никаких закрытых компонентов (системные библиотеки Windows можно);
- проект поддерживается и **уже выпущен** в том виде, который будет
  подписываться — то есть на странице Releases уже есть хотя бы один релиз
  с `lightbox-setup.exe`;
- что делает программа, описано на странице загрузки (README подходит);
- у всех участников включена двухфакторная аутентификация — и в GitHub, и
  потом в SignPath;
- на главной странице проекта и на странице загрузки есть раздел или ссылка
  **«Code signing policy»**.

Раздел для README (на английском, его будут читать проверяющие):

```markdown
## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/),
certificate by [SignPath Foundation](https://signpath.org/).

Team roles:
- Committers and reviewers: [Nicoskin](https://github.com/Nicoskin)
- Approvers: [Nicoskin](https://github.com/Nicoskin)

Release builds are made by GitHub Actions from this repository
(`.github/workflows/release.yml`); only those builds are signed.

Privacy policy: Lightbox does not send any data anywhere, with one
exception. Unless turned off in Settings, it checks this repository's
GitHub Releases for a newer version once a day (a request to
api.github.com; GitHub's privacy statement applies) and downloads the
installer only when you confirm the update.
```

Ещё полезно (проверяющие часто об этом спрашивают): `LICENSE` в корне — уже
есть, инструкция по сборке в README — уже есть.

## 2. Заявка

1. Откройте <https://signpath.org/apply> и заполните форму (по-английски,
   коротко):
   - **Repository URL** — `https://github.com/Nicoskin/LightBox`;
   - **License** — MIT (файл `LICENSE` в корне);
   - **Download URL** — `https://github.com/Nicoskin/LightBox/releases`;
   - **Description** — что это: «Fast image viewer for Windows (Rust, Win32,
     Direct3D 11) in the spirit of Picasa Photo Viewer»; что подписывается:
     два PE-файла, `lightbox.exe` и установщик `lightbox-setup.exe`, оба
     собираются GitHub Actions;
   - контактный e-mail, который вы читаете.
2. Ждите письма. Могут задать вопросы — отвечайте в том же письме. После
   одобрения придёт приглашение в организацию SignPath (её создаёт фонд,
   сертификат уже лежит там).
3. Войдите в <https://app.signpath.io> по приглашению и сразу включите 2FA.

## 3. Настройка в SignPath

Обычно фонд уже создаёт проект и две политики подписи; проверьте и
досоздайте, чего нет.

1. **Trusted build system.** *Trusted Build Systems* → добавьте
   предопределённую систему **GitHub.com** в организацию и привяжите её к
   проекту. Так SignPath проверяет, что файл собран именно этим
   репозиторием на раннере GitHub. По желанию поставьте SignPath GitHub App
   на репозиторий (для журнала аудита).
2. **Project.** *Projects* → проект Lightbox, репозиторий
   `https://github.com/Nicoskin/LightBox`. Запомните **slug** проекта.
3. **Artifact configuration.** В проекте → *Artifact Configurations* →
   создайте (или отредактируйте имеющуюся) и вставьте содержимое
   [`.github/signpath/artifact-configuration.xml`](../.github/signpath/artifact-configuration.xml).
   Сделайте её конфигурацией **по умолчанию** (*Set as default*) — workflow
   не передаёт slug конфигурации. Workflow отправляет каждый exe отдельным
   ZIP-архивом GitHub Actions, поэтому корень конфигурации — `<zip-file>`, а
   `*.exe` подходит обоим файлам.
4. **Signing policies.** Должны быть две:
   - `release-signing` — релизный сертификат, *Approvers*: вы (каждый запрос
     надо одобрить вручную);
   - `test-signing` — тестовый сертификат, без одобрения.

   У обеих: *Trusted build system* — GitHub.com, *Submitters* — пользователь,
   чей API-токен будет в GitHub (пункт 5).
5. **API-токен.** *Users* → создайте CI-пользователя (или возьмите свой) →
   *Generate API Token*. Добавьте этого пользователя в *Submitters* обеих
   политик. Токен показывается один раз — сразу скопируйте.
6. **Organization ID** — на странице *Settings* организации (GUID) или в
   адресе: `app.signpath.io/Web/<organization-id>/...`.

## 4. Секрет и переменные в GitHub

Репозиторий → *Settings* → *Secrets and variables* → *Actions*.

| Где | Имя | Значение |
|---|---|---|
| **Secrets** | `SIGNPATH_API_TOKEN` | токен из п. 3.5 |
| Variables | `SIGNPATH_ORGANIZATION_ID` | GUID организации |
| Variables | `SIGNPATH_PROJECT_SLUG` | slug проекта (если не задан — имя репозитория, `LightBox`) |
| Variables | `SIGNPATH_SIGNING_POLICY_SLUG` | необязательно, по умолчанию `release-signing` |
| Variables | `SIGNPATH_TEST_SIGNING_POLICY_SLUG` | необязательно, по умолчанию `test-signing` |

Подпись включается, когда заданы **оба** — `SIGNPATH_API_TOKEN` и
`SIGNPATH_ORGANIZATION_ID`. Только токен — секрет; остальное не тайна.

Через GitHub CLI то же самое:

```powershell
gh secret set SIGNPATH_API_TOKEN --repo Nicoskin/LightBox        # вставить токен
gh variable set SIGNPATH_ORGANIZATION_ID --repo Nicoskin/LightBox --body "<GUID>"
gh variable set SIGNPATH_PROJECT_SLUG --repo Nicoskin/LightBox --body "<slug>"
```

## 5. Выпуск релиза

1. Поднимите версию в `Cargo.toml` (`version = "0.3.0"`), закоммитьте,
   запушьте в `main`, дождитесь зелёного CI.
2. Поставьте тег с той же версией и запушьте его:

   ```powershell
   git tag v0.3.0
   git push origin v0.3.0
   ```

   Тег без дефиса обязан совпадать с версией в `Cargo.toml`, иначе workflow
   остановится сразу.
3. *Actions* → прогон «Release». Когда он дойдёт до «Sign lightbox.exe»,
   SignPath пришлёт запрос на одобрение — откройте его в app.signpath.io и
   нажмите *Approve*. Потом то же для «Sign installer» (второй запрос:
   установщик собирается уже с подписанным `lightbox.exe` внутри). Каждый
   запрос ждёт одобрения до часа.
4. Workflow создаёт **черновик** релиза с `lightbox-setup-<версия>.exe`,
   его `.sha256` и `lightbox-portable-<версия>.zip`. Откройте *Releases*, проверьте текст
   (сгенерирован по коммитам) и нажмите *Publish release*. Только после
   публикации релиз становится `latest` и его видит обновлятор программы.

Повторный запуск того же тега заменяет файлы в черновике; опубликованный
релиз workflow не трогает.

### Пробный прогон

Тег с дефисом (`v0.0.0-test`) — пре-релиз: версия может не совпадать с
`Cargo.toml`, подпись идёт политикой `test-signing` (без одобрения,
тестовым сертификатом — Windows ему не доверяет, это нормально), черновик
помечается как pre-release. После проверки удалите и черновик, и тег:

```powershell
gh release delete v0.0.0-test --repo Nicoskin/LightBox --yes
git push origin :refs/tags/v0.0.0-test
git tag -d v0.0.0-test
```

### Без SignPath

Пока секрета нет, тот же тег даёт неподписанный черновик (с предупреждением в
прогоне). Локальная сборка по-прежнему — `tools/release.ps1 -Repo
Nicoskin/LightBox`; она ничего не подписывает.
