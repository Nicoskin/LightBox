<#
.SYNOPSIS
    Launch-to-photo-on-screen time for Lightbox, IrfanView, XnView MP and Windows Photos.

.DESCRIPTION
    For every viewer and file: grab the whole desktop as a baseline, start the viewer with
    the file, then poll the screen every ~5 ms. The viewer's largest visible window is found
    by PID (Photos: by process name, it is activated through ms-photos:), and a 32x32 grid
    over the central 40 % of that window is checked. The photo counts as visible when most
    grid points have the colours of the test photo (sky, ground, ring of
    gen_testdata's photo_rgb) and were not already like that in the baseline; the time is
    that of the first such poll, once the next one confirms it. The viewer is then killed.
    Each viewer/file pair gets warm-up runs and measured runs; the median and the minimum
    are printed. A low-resolution preview counts as "visible", which is what the user sees.
    The screen itself refreshes once per frame, so the resolution is ~7-16 ms.

    XnView MP shows a setup wizard on its first start; %APPDATA%\XnViewMP\xnview.ini with
    "[Start]" / "startupType=0" skips it.

    Runs only while the session is unlocked, the user has been idle for -IdleSec, nothing
    is in exclusive fullscreen and no game from -Games is running; it waits up to
    -WaitMinutes for that, and stops at once if the user touches the mouse or keyboard
    (results so far are kept in the CSV; -Resume continues). It never sends input.

    Test images come from `cargo run --release --example gen_testdata`; the 24 MP TIFF
    (uncompressed, 8-bit RGB) is made from photo_24mp.png on the first run.

.EXAMPLE
    pwsh tools/bench_open.ps1 -Lightbox target\bench\release\lightbox.exe
#>
param(
    [string]$DataDir = (Join-Path $PSScriptRoot '..\testdata'),
    [string]$Lightbox = '',
    [string]$IrfanView = 'C:\Program Files\IrfanView\i_view64.exe',
    [string]$XnView = 'C:\Program Files\XnViewMP\xnviewmp.exe',
    [string[]]$Viewers = @('Lightbox', 'LightboxNoAnim', 'IrfanView', 'XnView', 'Photos'),
    [string[]]$Files = @('photo_24mp.jpg', 'photo_24mp.png', 'photo_24mp.tif'),
    [int]$Warmup = 2,
    [int]$Runs = 5,
    [int]$PauseMs = 2000,
    [int]$TimeoutMs = 20000,
    [double]$Threshold = 0.6,
    [int]$IdleSec = 60,
    [int]$WaitMinutes = 120,
    [string[]]$Games = @('dota2', 'cs2'),
    [string]$Csv = (Join-Path $PSScriptRoot '..\target\bench_open.csv'),
    [string]$SnapDir = '',
    [switch]$Resume
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading;

public static class OpenBench {
    [DllImport("user32.dll")] static extern IntPtr SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int i);
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr o);
    [DllImport("gdi32.dll")] static extern bool DeleteDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern bool GdiFlush();
    [DllImport("gdi32.dll")] static extern bool BitBlt(IntPtr d, int x, int y, int w, int h, IntPtr s, int sx, int sy, uint rop);
    [DllImport("gdi32.dll")] static extern IntPtr CreateDIBSection(IntPtr dc, ref BMIH bi, uint usage, out IntPtr bits, IntPtr sec, uint off);
    [DllImport("user32.dll")] static extern bool GetLastInputInfo(ref LII l);
    [DllImport("shell32.dll")] static extern int SHQueryUserNotificationState(out int state);
    [DllImport("winmm.dll")] static extern uint timeBeginPeriod(uint p);
    [DllImport("winmm.dll")] static extern uint timeEndPeriod(uint p);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int size);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int size);

    [StructLayout(LayoutKind.Sequential)] struct BMIH {
        public uint biSize; public int biWidth, biHeight; public ushort biPlanes, biBitCount;
        public uint biCompression, biSizeImage; public int biXPels, biYPels; public uint biClrUsed, biClrImportant;
    }
    [StructLayout(LayoutKind.Sequential)] struct LII { public uint cb; public uint time; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }

    const uint SRCCOPY_CAPTUREBLT = 0x00CC0020 | 0x40000000;

    // A screen area copied into a 32-bit top-down DIB.
    sealed class Grab : IDisposable {
        public int X, Y, W, H; public IntPtr Bits; IntPtr dc, bmp, old;
        public Grab(int w, int h) {
            W = w; H = h;
            var bi = new BMIH { biSize = 40, biWidth = w, biHeight = -h, biPlanes = 1, biBitCount = 32 };
            IntPtr screen = GetDC(IntPtr.Zero);
            dc = CreateCompatibleDC(screen);
            ReleaseDC(IntPtr.Zero, screen);
            bmp = CreateDIBSection(dc, ref bi, 0, out Bits, IntPtr.Zero, 0);
            if (bmp == IntPtr.Zero) throw new Exception("CreateDIBSection failed");
            old = SelectObject(dc, bmp);
        }
        public void Take(int x, int y) {
            X = x; Y = y;
            IntPtr screen = GetDC(IntPtr.Zero);
            BitBlt(dc, 0, 0, W, H, screen, x, y, SRCCOPY_CAPTUREBLT);
            GdiFlush(); // GDI batches: the bits are read right after
            ReleaseDC(IntPtr.Zero, screen);
        }
        // Pixel at absolute screen coordinates (BGRA).
        public int At(int sx, int sy) { return Marshal.ReadInt32(Bits, ((sy - Y) * W + (sx - X)) * 4); }
        public void Dispose() { SelectObject(dc, old); DeleteObject(bmp); DeleteDC(dc); }
    }

    static int vx, vy, vw, vh;
    static Grab baseline;

    public static string Init() {
        SetProcessDpiAwarenessContext(new IntPtr(-4)); // per-monitor v2: physical pixels
        vx = GetSystemMetrics(76); vy = GetSystemMetrics(77); vw = GetSystemMetrics(78); vh = GetSystemMetrics(79);
        baseline = new Grab(vw, vh);
        return vx + "," + vy + " " + vw + "x" + vh;
    }

    public static uint IdleMs() {
        var l = new LII { cb = 8 }; GetLastInputInfo(ref l);
        return (uint)Environment.TickCount - l.time;
    }
    static uint LastInput() { var l = new LII { cb = 8 }; GetLastInputInfo(ref l); return l.time; }

    // 2 = busy (fullscreen), 3 = D3D exclusive fullscreen, 4 = presentation mode.
    public static int NotificationState() { int s; SHQueryUserNotificationState(out s); return s; }

    // Colours of gen_testdata's photo_rgb: sky, ground, ring. Window chrome is grey.
    static bool PhotoLike(int p) {
        int b = p & 255, g = (p >> 8) & 255, r = (p >> 16) & 255;
        bool sky = b >= 150 && b - r >= 30 && g - r >= 15 && b >= g;
        bool ground = b <= 75 && r <= 140 && g >= 60 && r + g >= 150;
        bool ring = r >= 225 && g >= 195 && b <= 150;
        return sky || ground || ring;
    }
    static int Diff(int a, int c) {
        return Math.Abs((a & 255) - (c & 255)) + Math.Abs(((a >> 8) & 255) - ((c >> 8) & 255))
             + Math.Abs(((a >> 16) & 255) - ((c >> 16) & 255));
    }

    static IntPtr FindWindow(uint pid, string name, Dictionary<uint, bool> owned, out RECT best) {
        IntPtr found = IntPtr.Zero; long area = 0; RECT pick = new RECT();
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h) || IsIconic(h)) return true;
            uint p; GetWindowThreadProcessId(h, out p);
            bool mine;
            if (!owned.TryGetValue(p, out mine)) {
                mine = p == pid;
                if (!mine && name != null) {
                    try { mine = Process.GetProcessById((int)p).ProcessName.Equals(name, StringComparison.OrdinalIgnoreCase); }
                    catch { mine = false; }
                }
                owned[p] = mine;
            }
            if (!mine) return true;
            int cloaked; DwmGetWindowAttribute(h, 14, out cloaked, 4);
            if (cloaked != 0) return true;
            RECT r; if (DwmGetWindowAttribute(h, 9, out r, 16) != 0) return true;
            long a = (long)(r.R - r.L) * (r.B - r.T);
            if (r.R - r.L >= 300 && r.B - r.T >= 200 && a > area) { area = a; found = h; pick = r; }
            return true;
        }, IntPtr.Zero);
        best = pick;
        return found;
    }

    // Starts the viewer and waits for the photo. Returns
    // "pid;window_ms;photo_ms;polls;fraction;status" (ms = -1 when not reached).
    // byName: match windows by process name (Photos) instead of the started PID.
    public static string Measure(string exe, string args, bool shell, string byName, string appData,
                                 int timeoutMs, double threshold, string snapPath) {
        baseline.Take(vx, vy);
        uint input = LastInput();
        var psi = new ProcessStartInfo(exe, args) { UseShellExecute = shell };
        if (!shell && appData != null) psi.Environment["APPDATA"] = appData;
        timeBeginPeriod(1);
        var owned = new Dictionary<uint, bool>();
        Grab region = null;
        RECT last = new RECT();
        int polls = 0; double frac = 0; long winMs = -1, photoMs = -1, seenMs = -1; uint pid = 0; string status = "timeout";
        try {
            var sw = Stopwatch.StartNew();
            using (var proc = Process.Start(psi)) {
                if (proc != null && !shell) pid = (uint)proc.Id;
                while (sw.ElapsedMilliseconds < timeoutMs) {
                    if (LastInput() != input) { status = "input"; break; }
                    polls++;
                    RECT r;
                    if (FindWindow(pid, byName, owned, out r) != IntPtr.Zero) {
                        if (winMs < 0) winMs = sw.ElapsedMilliseconds;
                        last = r;
                        int w = r.R - r.L, h = r.B - r.T;
                        int x0 = Math.Max(vx, r.L + w * 3 / 10), y0 = Math.Max(vy, r.T + h * 3 / 10);
                        int x1 = Math.Min(vx + vw, r.R - w * 3 / 10), y1 = Math.Min(vy + vh, r.B - h * 3 / 10);
                        if (x1 - x0 >= 32 && y1 - y0 >= 32) {
                            if (region == null || region.W != x1 - x0 || region.H != y1 - y0) {
                                if (region != null) region.Dispose();
                                region = new Grab(x1 - x0, y1 - y0);
                            }
                            region.Take(x0, y0);
                            int hits = 0;
                            for (int j = 0; j < 32; j++)
                                for (int i = 0; i < 32; i++) {
                                    int sx = x0 + (2 * i + 1) * (x1 - x0) / 64, sy = y0 + (2 * j + 1) * (y1 - y0) / 64;
                                    int now = region.At(sx, sy), was = baseline.At(sx, sy);
                                    if (PhotoLike(now) && !(PhotoLike(was) && Diff(now, was) < 24)) hits++;
                                }
                            frac = hits / 1024.0;
                            // Counts from the first poll that shows the photo, once the
                            // next poll confirms it.
                            if (frac >= threshold) {
                                if (seenMs >= 0) {
                                    photoMs = seenMs; status = "ok";
                                    if (snapPath != null) SaveBmp(region, snapPath + "-photo.bmp");
                                    break;
                                }
                                seenMs = sw.ElapsedMilliseconds;
                            } else {
                                seenMs = -1;
                            }
                        }
                    }
                    // Next poll on the next 5 ms boundary.
                    long next = (sw.ElapsedMilliseconds / 5 + 1) * 5;
                    while (sw.ElapsedMilliseconds < next) Thread.Sleep(1);
                }
                // The whole window too, to see what it shows.
                if (snapPath != null && last.R > last.L) {
                    int x0 = Math.Max(vx, last.L), y0 = Math.Max(vy, last.T);
                    int x1 = Math.Min(vx + vw, last.R), y1 = Math.Min(vy + vh, last.B);
                    using (var g = new Grab(x1 - x0, y1 - y0)) { g.Take(x0, y0); SaveBmp(g, snapPath + "-window.bmp"); }
                }
            }
        } finally {
            timeEndPeriod(1);
            if (region != null) region.Dispose();
        }
        return pid + ";" + winMs + ";" + photoMs + ";" + polls + ";" + frac.ToString("0.00", System.Globalization.CultureInfo.InvariantCulture)
            + ";" + status + ";" + (last.R - last.L) + "x" + (last.B - last.T);
    }

    static void SaveBmp(Grab g, string path) {
        int size = g.W * g.H * 4;
        var px = new byte[size];
        Marshal.Copy(g.Bits, px, 0, size);
        using (var f = new System.IO.BinaryWriter(System.IO.File.Create(path))) {
            f.Write((ushort)0x4D42); f.Write(54 + size); f.Write(0); f.Write(54);
            f.Write(40); f.Write(g.W); f.Write(-g.H); f.Write((ushort)1); f.Write((ushort)32);
            f.Write(0); f.Write(size); f.Write(2835); f.Write(2835); f.Write(0); f.Write(0);
            f.Write(px);
        }
    }
}
'@

function Test-Safe {
    if (Get-Process LogonUI -ErrorAction SilentlyContinue) { return 'screen locked' }
    $game = Get-Process -Name $Games -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($game) { return "$($game.Name) is running" }
    $state = [OpenBench]::NotificationState()
    if ($state -in 2, 3, 4) { return "fullscreen app (notification state $state)" }
    $idle = [OpenBench]::IdleMs()
    if ($idle -lt $IdleSec * 1000) { return "user active $([int]($idle / 1000)) s ago" }
    return $null
}

function Wait-Safe {
    $deadline = (Get-Date).AddMinutes($WaitMinutes)
    while ($true) {
        $why = Test-Safe
        if (-not $why) { return }
        if ((Get-Date) -gt $deadline) { Write-Host "gave up waiting: $why"; exit 2 }
        Write-Host "waiting: $why"
        Start-Sleep -Seconds 15
    }
}

$DataDir = (Resolve-Path $DataDir).Path
$root = Split-Path $PSScriptRoot
if (-not $Lightbox) {
    $Lightbox = @('target\bench\release\lightbox.exe', 'target\release\lightbox.exe') |
        ForEach-Object { Join-Path $root $_ } | Where-Object { Test-Path $_ } | Select-Object -First 1
}

# 24 MP TIFF, uncompressed 8-bit RGB, from the PNG.
$tif = Join-Path $DataDir 'photo_24mp.tif'
if (($Files -contains 'photo_24mp.tif') -and -not (Test-Path $tif)) {
    Add-Type -AssemblyName System.Drawing
    $src = [System.Drawing.Bitmap]::new((Join-Path $DataDir 'photo_24mp.png'))
    $rgb = $src.Clone([System.Drawing.Rectangle]::new(0, 0, $src.Width, $src.Height),
        [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object MimeType -EQ 'image/tiff'
    $ep = [System.Drawing.Imaging.EncoderParameters]::new(1)
    $ep.Param[0] = [System.Drawing.Imaging.EncoderParameter]::new([System.Drawing.Imaging.Encoder]::Compression,
        [long][System.Drawing.Imaging.EncoderValue]::CompressionNone)
    $rgb.Save($tif, $codec, $ep)
    $rgb.Dispose(); $src.Dispose()
    Write-Host "made $tif"
}

# A viewer left open by the user is not ours to kill.
foreach ($n in 'i_view64', 'xnviewmp', 'Photos') {
    if (Get-Process $n -ErrorAction SilentlyContinue) { Write-Host "$n is already running; close it first"; exit 3 }
}

# Lightbox gets its own settings folders: default settings, nothing touches the user's.
# LightboxNoAnim turns off the 200 ms opening animation (the photo fades in from 35 %
# opacity, so the detector fires midway through it).
$lbData = Join-Path ([IO.Path]::GetTempPath()) 'lightbox-bench-appdata'
$lbNoAnim = Join-Path ([IO.Path]::GetTempPath()) 'lightbox-bench-appdata-noanim'
New-Item -ItemType Directory -Force $lbData, (Join-Path $lbNoAnim 'Lightbox') | Out-Null
'animations = false' | Set-Content (Join-Path $lbNoAnim 'Lightbox\settings.toml')

$viewerDefs = @{
    Lightbox       = @{ Exe = $Lightbox; Name = $null; AppData = $lbData; Kill = $null }
    LightboxNoAnim = @{ Exe = $Lightbox; Name = $null; AppData = $lbNoAnim; Kill = $null }
    IrfanView = @{ Exe = $IrfanView; Name = $null; AppData = $null; Kill = 'i_view64' }
    XnView    = @{ Exe = $XnView; Name = $null; AppData = $null; Kill = 'xnviewmp' }
    Photos    = @{ Exe = $null; Name = 'Photos'; AppData = $null; Kill = 'Photos' }
}

$unknown = $Viewers | Where-Object { -not $viewerDefs.ContainsKey($_) }
if ($unknown) { Write-Host "unknown viewer: $unknown"; exit 1 }

Write-Host "virtual screen: $([OpenBench]::Init())"
Write-Host "lightbox: $Lightbox"
New-Item -ItemType Directory -Force (Split-Path $Csv) | Out-Null
if ($SnapDir) { New-Item -ItemType Directory -Force $SnapDir | Out-Null }
if (-not $Resume -or -not (Test-Path $Csv)) {
    'viewer,file,run,warmup,window_ms,photo_ms,polls,fraction,status,window' | Set-Content $Csv
}
$done = Import-Csv $Csv

foreach ($file in $Files) {
    $path = Join-Path $DataDir $file
    foreach ($v in $Viewers) {
        $def = $viewerDefs[$v]
        $total = $Warmup + $Runs
        $have = @($done | Where-Object { $_.viewer -eq $v -and $_.file -eq $file -and $_.status -eq 'ok' }).Count
        for ($run = $have + 1; $run -le $total; $run++) {
            Wait-Safe
            $attempt++
            $snap = if ($SnapDir) { Join-Path $SnapDir "$v-$file-$run-a$attempt" } else { $null }
            if ($def.Exe) {
                $r = [OpenBench]::Measure($def.Exe, "`"$path`"", $false, $null, $def.AppData, $TimeoutMs, $Threshold, $snap)
            } else {
                $r = [OpenBench]::Measure("ms-photos:viewer?fileName=$path", '', $true, $def.Name, $null, $TimeoutMs, $Threshold, $snap)
            }
            $f = $r.Split(';')
            if ($f[0] -ne '0') { Stop-Process -Id ([int]$f[0]) -Force -ErrorAction SilentlyContinue }
            if ($def.Kill) { Get-Process $def.Kill -ErrorAction SilentlyContinue | Stop-Process -Force }
            $warm = [int]($run -le $Warmup)
            "$v,$file,$run,$warm,$($f[1]),$($f[2]),$($f[3]),$($f[4]),$($f[5]),$($f[6])" | Add-Content $Csv
            Write-Host ("{0,-14} {1,-15} #{2} {3}window {4,5} ms  photo {5,5} ms  ({6} polls, {7}, {9}) {8}" -f
                $v, $file, $run, ('', 'warm-up ')[$warm], $f[1], $f[2], $f[3], $f[4], $f[5], $f[6])
            if ($f[5] -eq 'input') { Write-Host 'user input: stopped (rerun with -Resume)'; exit 4 }
            if ($f[5] -ne 'ok') { $run-- ; if (++$fails -ge 3) { Write-Host "$v / ${file}: 3 failures, skipped"; $fails = 0; break } }
            Start-Sleep -Milliseconds $PauseMs
        }
    }
}

# Median and minimum of the measured runs.
$rows = Import-Csv $Csv | Where-Object { $_.status -eq 'ok' -and $_.warmup -eq '0' }
"`n| viewer | file | median, ms | min, ms | runs |"
'|---|---|---:|---:|---:|'
foreach ($g in ($rows | Group-Object viewer, file)) {
    $ms = @($g.Group | ForEach-Object { [int]$_.photo_ms } | Sort-Object)
    $med = if ($ms.Count % 2) { $ms[[int][math]::Floor($ms.Count / 2)] } else { ($ms[$ms.Count / 2 - 1] + $ms[$ms.Count / 2]) / 2 }
    "| $($g.Group[0].viewer) | $($g.Group[0].file) | $med | $($ms[0]) | $($ms.Count) |"
}
