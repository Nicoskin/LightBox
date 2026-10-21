# Builds a release: the viewer, the Explorer thumbnail handler, the
# installer that embeds both, and the checksum the viewer's updater verifies.
#
#   pwsh tools/release.ps1 -Repo owner/name
#
# -Repo (or $env:LIGHTBOX_UPDATE_REPO) is the GitHub repository whose
# Releases the viewer checks for updates; without it updates are off.
# Output in dist/: lightbox-setup-<version>.exe and its .sha256, and
# lightbox-portable-<version>.zip (the version from Cargo.toml). Publish with
# the command printed at the end (GitHub CLI).

param([string]$Repo = $env:LIGHTBOX_UPDATE_REPO)
$ErrorActionPreference = "Stop"

$root = Split-Path $PSScriptRoot -Parent
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
if ($Repo) { $env:LIGHTBOX_UPDATE_REPO = $Repo } else { Remove-Item Env:LIGHTBOX_UPDATE_REPO -ErrorAction SilentlyContinue }

# Separate target folders: a viewer running from target\release keeps its
# exe locked.
# The Explorer thumbnail handler (shellext/): its own profile, where a
# panic unwinds to the COM boundary instead of aborting.
cargo build --profile shellext -p lightbox-thumbs --manifest-path "$root\Cargo.toml" --target-dir "$root\target\dist"
if ($LASTEXITCODE) { throw "thumbnail handler build failed" }

cargo build --release --manifest-path "$root\Cargo.toml" --target-dir "$root\target\dist"
if ($LASTEXITCODE) { throw "viewer build failed" }

$env:LIGHTBOX_EXE = "$root\target\dist\release\lightbox.exe"
$env:LIGHTBOX_THUMBS_DLL = "$root\target\dist\shellext\lightbox_thumbs.dll"
$env:LIGHTBOX_VERSION = $version
cargo build --release --manifest-path "$root\installer\Cargo.toml" --target-dir "$root\target\dist-setup"
if ($LASTEXITCODE) { throw "installer build failed" }

$dist = New-Item -ItemType Directory -Force "$root\dist"
$setupName = "lightbox-setup-$version.exe"
$setup = Join-Path $dist $setupName
Copy-Item "$root\target\dist-setup\release\lightbox-setup.exe" $setup -Force
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLower()
Set-Content -Path "$setup.sha256" -Value "$hash  $setupName" -NoNewline -Encoding ascii

# Portable: the viewer and the marker that keeps its settings beside it.
$stage = Join-Path $root "target\dist-portable\Lightbox"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item "$root\target\dist\release\lightbox.exe" $stage
Set-Content -Path "$stage\portable.txt" -Encoding utf8 -Value @"
Lightbox $version, портативная версия.
Настройки и кэш хранятся в папке data рядом с lightbox.exe; трей, автозапуск
и реестр не используются. Удалите этот файл, чтобы Lightbox работал как
установленный.

Lightbox $version, portable. Settings and caches live in the data folder next
to lightbox.exe; no tray, start-up entry or registry. Delete this file to
make Lightbox use the normal locations.
"@
$portable = Join-Path $dist "lightbox-portable-$version.zip"
Compress-Archive -Path $stage -DestinationPath $portable -Force

Write-Host ""
Write-Host "Lightbox $version" -ForegroundColor Green
Write-Host "  $setup ($([math]::Round((Get-Item $setup).Length / 1MB, 1)) MB)"
Write-Host "  sha256 $hash"
Write-Host "  $portable ($([math]::Round((Get-Item $portable).Length / 1MB, 1)) MB)"
if ($Repo) {
    Write-Host ""
    Write-Host "Publish:"
    Write-Host "  gh release create v$version `"$setup`" `"$setup.sha256`" `"$portable`" --repo $Repo --title `"Lightbox $version`" --generate-notes"
} else {
    Write-Host "  (built without -Repo: this build does not check for updates)" -ForegroundColor Yellow
}
