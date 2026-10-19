# Builds a release: the viewer, the installer that embeds it, and the
# checksum the viewer's updater verifies.
#
#   pwsh tools/release.ps1 -Repo owner/name
#
# -Repo (or $env:LIGHTBOX_UPDATE_REPO) is the GitHub repository whose
# Releases the viewer checks for updates; without it updates are off.
# Output in dist/: lightbox-<version>-setup.exe and its .sha256. Publish
# with the command printed at the end (GitHub CLI).

param([string]$Repo = $env:LIGHTBOX_UPDATE_REPO)
$ErrorActionPreference = "Stop"

$root = Split-Path $PSScriptRoot -Parent
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
if ($Repo) { $env:LIGHTBOX_UPDATE_REPO = $Repo } else { Remove-Item Env:LIGHTBOX_UPDATE_REPO -ErrorAction SilentlyContinue }

# Separate target folders: a viewer running from target\release keeps its
# exe locked.
cargo build --release --manifest-path "$root\Cargo.toml" --target-dir "$root\target\dist"
if ($LASTEXITCODE) { throw "viewer build failed" }

$env:LIGHTBOX_EXE = "$root\target\dist\release\lightbox.exe"
$env:LIGHTBOX_VERSION = $version
cargo build --release --manifest-path "$root\installer\Cargo.toml" --target-dir "$root\target\dist-setup"
if ($LASTEXITCODE) { throw "installer build failed" }

$dist = New-Item -ItemType Directory -Force "$root\dist"
$setup = Join-Path $dist "lightbox-$version-setup.exe"
Copy-Item "$root\target\dist-setup\release\lightbox-setup.exe" $setup -Force
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLower()
Set-Content -Path "$setup.sha256" -Value "$hash  lightbox-$version-setup.exe" -NoNewline -Encoding ascii

Write-Host ""
Write-Host "Lightbox $version" -ForegroundColor Green
Write-Host "  $setup ($([math]::Round((Get-Item $setup).Length / 1MB, 1)) MB)"
Write-Host "  sha256 $hash"
if ($Repo) {
    Write-Host ""
    Write-Host "Publish:"
    Write-Host "  gh release create v$version `"$setup`" `"$setup.sha256`" --repo $Repo --title `"Lightbox $version`" --generate-notes"
} else {
    Write-Host "  (built without -Repo: this build does not check for updates)" -ForegroundColor Yellow
}
