//! System integration: files handed over by new launches (single
//! instance), the resident mode (hidden window + tray icon, start with
//! Windows) and QuickLook (Space in Explorer previews the selected image).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread;

use anyhow::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IServiceProvider,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_SPACE,
};
use windows::Win32::UI::Shell::{
    IFolderView, IShellBrowser, IShellItemArray, IShellWindows, IWebBrowserApp, NIF_ICON,
    NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, SID_STopLevelBrowser,
    SIGDN_FILESYSPATH, SVGIO_SELECTION, SWC_DESKTOP, SWFO_NEEDDISPATCH, Shell_NotifyIconW,
    ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, FindWindowExW, GUITHREADINFO, GetClassNameW,
    GetForegroundWindow, GetGUIThreadInfo, GetMessageW, GetParent, GetWindowThreadProcessId, HHOOK,
    IsIconic, IsWindowVisible, KBDLLHOOKSTRUCT, MSG, PostMessageW, RegisterWindowMessageW, SW_HIDE,
    SW_RESTORE, SW_SHOW, SetForegroundWindow, SetWindowsHookExW, ShowWindow, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP,
};
use windows::core::{Interface as _, w};

use super::{App, Mode, WM_APP_OPEN, WM_APP_QUICKLOOK, WM_APP_TRAY, post};
use crate::i18n::tr;
use crate::settings::OpenMode;
use crate::win::monitor::Monitor;
use crate::win::{dialogs, menu, window};
use crate::{APP_NAME, cache, decode, ipc, library};

/// Marks key events this program sends back (QuickLook declined a Space).
const INJECTED: usize = 0x4C42_5351;
/// Tray menu ids.
const TRAY_OPEN: u32 = 1;
const TRAY_EXIT: u32 = 2;

/// Window the keyboard hook posts to.
static HOOK_TARGET: AtomicIsize = AtomicIsize::new(0);

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: valid buffer.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

impl App {
    /// Starts the pipe server (single instance).
    pub(super) fn start_ipc(&mut self) {
        let hwnd = self.hwnd.0 as isize;
        self.open_rx = Some(ipc::serve(move || post(hwnd, WM_APP_OPEN)));
    }

    /// `--background`: everything ready, nothing shown.
    pub fn start_hidden(&mut self) -> Result<()> {
        self.hidden = true;
        self.render()?;
        self.compositor.set_content(&self.gpu.swap_chain)?;
        self.compositor.commit()
    }

    pub(super) fn on_open(&mut self) -> Result<()> {
        let mut last = None;
        while let Some(p) = self.open_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            last = Some(p);
        }
        match last {
            Some(p) if p.as_os_str() == ipc::SHOW_HOME => self.show_home(),
            Some(p) => {
                self.quicklook = false;
                self.open_external(&p)
            }
            None => Ok(()),
        }
    }

    /// Shows `path`: in the open window (its folder is browsed), or — when
    /// hidden in the tray — in a window that appears once it is decoded.
    pub(super) fn open_external(&mut self, path: &Path) -> Result<()> {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        // From the start screen the window opens like from the tray: sized
        // to the photo once it is decoded.
        let from_home = self.home();
        if !self.hidden && !from_home {
            if let Some(i) = self.files.iter().position(|p| library::same_path(p, &path)) {
                self.go_to(i)?;
                self.bring_to_front();
                return Ok(());
            }
            self.leave_current();
        }
        self.current = path.clone();
        self.all_files = vec![path.clone()];
        self.files = vec![path];
        self.index = 0;
        self.strip_pos = 0.0;
        self.angle = 0.0;
        self.error = None;
        if self.hidden || from_home {
            self.shown = None;
            self.pending_show = true;
        }
        self.update_title();
        self.rescan_folder();
        if self.show_current()? {
            if self.pending_show {
                self.reveal()?;
            } else {
                self.render()?;
            }
        }
        self.request_thumbs();
        self.schedule();
        if !self.pending_show {
            self.bring_to_front();
        }
        Ok(())
    }

    /// The image waited for by a hidden window is ready: show the window
    /// on the monitor under the cursor, in the default mode.
    pub(super) fn reveal(&mut self) -> Result<()> {
        self.pending_show = false;
        self.hidden = false;
        self.monitor = Monitor::under_cursor();
        // A preview always opens as a window sized to the photo.
        let windowed = self.quicklook || self.settings.open_mode == OpenMode::Windowed;
        match windowed {
            true => self.start_windowed()?,
            false => {
                self.mode = Mode::Fullscreen;
                window::set_mode(self.hwnd, false, self.monitor.rect)?;
                window::set_topmost(self.hwnd, false);
                let (w, h) = self.monitor.size();
                self.gpu.resize(w, h)?;
                self.dim = self.settings.dim();
                if let Some(v) = self.fit_view() {
                    self.view = v;
                }
                self.fitted = true;
            }
        }
        self.start_open_anim();
        self.render()?;
        // SAFETY: own window.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
        }
        self.bring_to_front();
        self.restart_anim_clock();
        Ok(())
    }

    /// Activates the window, also when another program is in front (the
    /// launcher allowed it; for QuickLook the input of Explorer's thread is
    /// shared for the moment).
    pub(super) fn bring_to_front(&self) {
        bring_window_to_front(self.hwnd);
    }

    /// Close in resident mode: hide and free the images, keep the device.
    /// Returns `false` if the window should really close.
    pub(super) fn hide_instead_of_close(&mut self) -> bool {
        if !self.settings.resident || self.quitting || self.pinned || self.peek {
            return false;
        }
        self.leave_current();
        // SAFETY: own window.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.hidden = true;
        self.pending_show = false;
        self.quicklook = false;
        self.anim = None;
        self.shown = None;
        self.detail = None;
        self.textures.clear();
        self.cache.clear();
        self.menu = None;
        self.card_open = false;
        true
    }

    /// Quits for real (tray "Exit", or closing without resident mode).
    pub(super) fn quit(&mut self) {
        self.quitting = true;
        self.close_window();
    }

    /// Resident mode on / off: tray icon and the QuickLook hook.
    pub(super) fn apply_resident(&mut self) {
        if self.settings.resident {
            self.add_tray();
        } else {
            self.remove_tray();
        }
        self.apply_quick_look();
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: window::small_icon(),
            ..Default::default()
        };
        for (d, s) in nid.szTip.iter_mut().zip(APP_NAME.encode_utf16()) {
            *d = s;
        }
        nid
    }

    pub(super) fn add_tray(&mut self) {
        if self.tray {
            return;
        }
        // SAFETY: the struct is fully initialised and outlives the call.
        self.tray = unsafe { Shell_NotifyIconW(NIM_ADD, &self.tray_data()) }.as_bool();
    }

    pub(super) fn remove_tray(&mut self) {
        if self.tray {
            // SAFETY: as above.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray_data());
            }
            self.tray = false;
        }
    }

    /// Explorer restarted (taskbar re-created): add the icon again.
    pub(super) fn on_taskbar_created(&mut self) {
        if self.tray {
            self.tray = false;
            self.add_tray();
        }
    }

    pub(super) fn on_tray(&mut self, event: u32) -> Result<()> {
        match event {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                if self.hidden && !self.current.as_os_str().is_empty() {
                    let p = self.current.clone();
                    return self.open_external(&p);
                }
                if self.hidden {
                    return self.open_dialog();
                }
                self.bring_to_front();
                Ok(())
            }
            WM_RBUTTONUP => {
                let items = [
                    menu::Item::Command(TRAY_OPEN, tr("Открыть файл…").into(), true),
                    menu::Item::Separator,
                    menu::Item::Command(TRAY_EXIT, tr("Выход").into(), true),
                ];
                match menu::popup(self.hwnd, &items, window::cursor_pos()) {
                    Some(TRAY_OPEN) => self.open_dialog(),
                    Some(TRAY_EXIT) => {
                        self.quit();
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
            _ => Ok(()),
        }
    }

    pub(super) fn open_dialog(&mut self) -> Result<()> {
        let spec: String = decode::all_extensions()
            .map(|e| format!("*.{e}"))
            .collect::<Vec<_>>()
            .join(";");
        let start = self.current.parent().map(Path::to_path_buf);
        match dialogs::open(
            None,
            tr("Открыть файл"),
            false,
            Some((tr("Изображения и 3D-модели"), &spec)),
            start.as_deref(),
        ) {
            Some(p) => self.open_external(&p),
            None => Ok(()),
        }
    }
}

// QuickLook.
impl App {
    /// Installs or removes the keyboard hook to match the settings.
    pub(super) fn apply_quick_look(&mut self) {
        let want = self.settings.resident && self.settings.quick_look;
        HOOK_TARGET.store(self.hwnd.0 as isize, Ordering::Release);
        if want && self.hook_thread.is_none() {
            self.hook_thread = start_hook_thread();
        } else if !want && let Some(t) = self.hook_thread.take() {
            // SAFETY: posting WM_QUIT to the hook thread's queue.
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                    t,
                    windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    /// Space in Explorer (posted by the hook, which swallowed the key):
    /// preview the selected image, or give the key back.
    pub(super) fn on_quick_look(&mut self, explorer: HWND) -> Result<()> {
        let Some(path) = selected_file(explorer) else {
            resend_space(explorer);
            return Ok(());
        };
        let ours = decode::is_supported(&path) || decode::is_document(&path);
        // Other types: the system preview (Word, Excel…) in its own window.
        let system = !ours && crate::docview::handler_for(&path).is_some();
        if !ours && !system {
            resend_space(explorer);
            return Ok(());
        }
        let key = cache::key(&path);
        // A preview in its own process: Space again on its file closes it,
        // another file replaces it.
        if let Some((shown, pid)) = self.peek_child.take()
            && let Some(w) = window_of_process(pid)
        {
            post_close(w);
            if shown == key {
                return Ok(());
            }
        }
        if system {
            return self.peek(&path, "--document");
        }
        // Space again on the same file while it is shown: close.
        if !self.hidden && self.quicklook && key == cache::key(&self.current) {
            self.close_window();
            return Ok(());
        }
        // The viewer shows the user's own photo and folder: leave them be
        // and preview beside it.
        if !self.hidden && !self.preview() {
            return self.peek(&path, "--peek");
        }
        self.quicklook = true;
        self.open_external(&path)
    }

    /// Starts a preview process (`--peek` or `--document`) for `path` and
    /// brings its window to the front.
    fn peek(&mut self, path: &Path, mode: &str) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        let exe = std::env::current_exe()?;
        let child = std::process::Command::new(exe)
            .arg(mode)
            .arg(path)
            .spawn()?;
        let pid = child.id();
        // SAFETY: the child's handle is valid while `child` lives; its
        // message loop starts right after its first frame is on screen.
        unsafe {
            let _ = windows::Win32::System::Threading::WaitForInputIdle(
                windows::Win32::Foundation::HANDLE(child.as_raw_handle()),
                2000,
            );
        }
        // This process may take the foreground (Explorer's input is shared
        // for the moment); a freshly started one may not.
        if let Some(w) = window_of_process(pid) {
            bring_window_to_front(w);
        }
        self.peek_child = Some((cache::key(path), pid));
        Ok(())
    }
}

/// The single selected file of an Explorer window or the desktop.
fn selected_file(explorer: HWND) -> Option<PathBuf> {
    // SAFETY: COM calls on the UI thread (initialised here if needed); all
    // interfaces are released when dropped.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
        let class = class_name(explorer);
        let mut views = Vec::new();
        if class == "Progman" || class == "WorkerW" {
            let mut hwnd = 0;
            let loc = VARIANT::from(0i32); // CSIDL_DESKTOP
            if let Ok(d) = windows.FindWindowSW(
                &loc,
                &VARIANT::default(),
                SWC_DESKTOP,
                &mut hwnd,
                SWFO_NEEDDISPATCH,
            ) {
                views.push(d);
            }
        } else {
            for i in 0..windows.Count().ok()? {
                let Ok(d) = windows.Item(&VARIANT::from(i)) else {
                    continue;
                };
                let Ok(app) = d.cast::<IWebBrowserApp>() else {
                    continue;
                };
                if app.HWND().ok().is_some_and(|h| h.0 == explorer.0 as isize) {
                    views.push(d);
                }
            }
        }
        // Several tabs share the window (Windows 11). The active tab is the
        // first "ShellTabWindowClass" child, and a tab's browser reports
        // that window; inactive tabs may still look visible, so the
        // visibility test is only a fallback for older Explorers.
        let active_tab = FindWindowExW(Some(explorer), None, w!("ShellTabWindowClass"), None).ok();
        for d in views {
            let Ok(sp) = d.cast::<IServiceProvider>() else {
                continue;
            };
            let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else {
                continue;
            };
            if let Some(tab) = active_tab
                && browser.GetWindow().ok() != Some(tab)
            {
                continue;
            }
            let Ok(view) = browser.QueryActiveShellView() else {
                continue;
            };
            if active_tab.is_none()
                && let Ok(w) = view.GetWindow()
                && !IsWindowVisible(w).as_bool()
            {
                continue;
            }
            let fv: IFolderView = view.cast().ok()?;
            let items: IShellItemArray = fv.Items(SVGIO_SELECTION).ok()?;
            if items.GetCount().ok()? != 1 {
                return None;
            }
            let item = items.GetItemAt(0).ok()?;
            let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = PathBuf::from(p.to_string().ok()?);
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
            return Some(path);
        }
        None
    }
}

/// Gives a declined Space back to `explorer`, only while it is still in front (the
/// key must never land in another program).
fn resend_space(explorer: HWND) {
    // SAFETY: plain query.
    if unsafe { GetForegroundWindow() } != explorer {
        return;
    }
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_SPACE,
                dwFlags: flags,
                dwExtraInfo: INJECTED,
                ..Default::default()
            },
        },
    };
    let inputs = [key(Default::default()), key(KEYEVENTF_KEYUP)];
    // SAFETY: valid input array.
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}

/// `true` when the keyboard focus is in the item view of an Explorer window
/// or the desktop (not in the address bar, search box or a rename field).
fn explorer_items_focused(fg: HWND) -> bool {
    let class = class_name(fg);
    if !matches!(
        class.as_str(),
        "CabinetWClass" | "ExploreWClass" | "Progman" | "WorkerW"
    ) {
        return false;
    }
    // SAFETY: plain queries with valid out-structures.
    unsafe {
        let thread = GetWindowThreadProcessId(fg, None);
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread, &mut info).is_err() {
            return false;
        }
        // The item list lives in a SHELLDLL_DefView window (the address
        // bar and search box do not); its rename field is an "Edit".
        let focus = info.hwndFocus;
        if focus.is_invalid() || class_name(focus) == "Edit" {
            return false;
        }
        let mut h = focus;
        for _ in 0..6 {
            if class_name(h) == "SHELLDLL_DefView" {
                return true;
            }
            match GetParent(h) {
                Ok(p) if !p.is_invalid() => h = p,
                _ => break,
            }
        }
        false
    }
}

/// Low-level keyboard hook: a plain Space in Explorer's item view is taken
/// (and handled on the UI thread); everything else passes through.
extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_KEYDOWN {
        // SAFETY: for WH_KEYBOARD_LL, lParam points to a KBDLLHOOKSTRUCT.
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let down = |vk: u16| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
        let plain = ![VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
            .iter()
            .any(|k| down(k.0));
        if kb.vkCode == u32::from(VK_SPACE.0) && kb.dwExtraInfo != INJECTED && plain {
            // SAFETY: no preconditions.
            let fg = unsafe { GetForegroundWindow() };
            let target = HOOK_TARGET.load(Ordering::Acquire);
            if target != 0 && explorer_items_focused(fg) {
                // SAFETY: posting to our own window.
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(target as *mut _)),
                        WM_APP_QUICKLOOK,
                        WPARAM(fg.0 as usize),
                        LPARAM(0),
                    );
                }
                return LRESULT(1);
            }
        }
    }
    // SAFETY: standard chaining.
    unsafe { CallNextHookEx(None::<HHOOK>, code, wparam, lparam) }
}

/// Runs the hook on its own thread with its own message loop, so that a
/// busy UI thread never delays the user's typing elsewhere. Returns the
/// thread id (to stop it with WM_QUIT).
fn start_hook_thread() -> Option<u32> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = thread::Builder::new()
        .name("quicklook".into())
        .spawn(move || {
            // SAFETY: the hook procedure is a plain function of this module;
            // it is removed before the thread ends.
            unsafe {
                let Ok(hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0)
                else {
                    let _ = tx.send(None);
                    return;
                };
                let _ = tx.send(Some(GetCurrentThreadId()));
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let _ = UnhookWindowsHookEx(hook);
            }
        });
    match spawned {
        Ok(_) => rx.recv().ok().flatten(),
        Err(e) => {
            tracing::error!("cannot start the QuickLook thread: {e}");
            None
        }
    }
}

/// "TaskbarCreated": sent to top-level windows when Explorer restarts.
pub(super) fn taskbar_created_message() -> u32 {
    // SAFETY: registering a message name has no preconditions.
    unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }
}

impl App {
    /// Writes / removes the "start with Windows" entry.
    pub(super) fn set_autostart(&self, on: bool) {
        let exe = std::env::current_exe().unwrap_or_default();
        if let Err(e) = crate::win::registry::set_autostart(&exe, on) {
            tracing::warn!("autostart: {e:#}");
        }
    }
}

// Preview window (Space in Explorer).
impl App {
    /// Pin: the photo moves to its own window (a separate process at the
    /// same place) and stays there; the tray viewer is free for the next
    /// Space.
    pub(super) fn pin_preview(&mut self) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        self.leave_current();
        let r = window::rect(self.hwnd);
        let exe = std::env::current_exe()?;
        let child = std::process::Command::new(exe)
            .arg("--pinned")
            .arg(format!("{},{},{},{}", r.left, r.top, r.right, r.bottom))
            .arg(&self.current)
            .spawn()?;
        // SAFETY: the child's handle is valid while `child` lives; its
        // message loop starts right after its first frame is on screen.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(child.id());
            let _ = windows::Win32::System::Threading::WaitForInputIdle(
                windows::Win32::Foundation::HANDLE(child.as_raw_handle()),
                2000,
            );
        }
        self.quicklook = false;
        self.close_window();
        Ok(())
    }

    /// A preview becomes the full viewer: the folder, the filmstrip and
    /// the default mode.
    pub(super) fn open_full(&mut self) -> Result<()> {
        self.quicklook = false;
        self.pinned = false;
        self.rescan_folder();
        if self.settings.open_mode == OpenMode::Fullscreen {
            self.go_fullscreen()?;
        } else {
            self.render()?;
        }
        self.request_thumbs();
        self.schedule();
        Ok(())
    }
}

/// "Quit for real" (also from the tray): the installer and uninstaller
/// send it before replacing or removing the program.
pub fn quit_message() -> u32 {
    // SAFETY: registering a message name has no preconditions.
    unsafe { RegisterWindowMessageW(w!("Lightbox.Quit")) }
}

/// Activates `hwnd` (ours or a `--peek` child's), also when another
/// program is in front: our thread borrows the input queue of the
/// foreground window's thread for the moment.
fn bring_window_to_front(hwnd: HWND) {
    // SAFETY: plain window calls; attaching input queues is undone right
    // away.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attach = fg_thread != 0 && fg_thread != me;
        if attach {
            let _ = AttachThreadInput(me, fg_thread, true);
        }
        let _ = SetForegroundWindow(hwnd);
        if attach {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
}

/// The visible top-level window of process `pid` (a preview we started),
/// if it is still running.
fn window_of_process(pid: u32) -> Option<HWND> {
    let mut after = None;
    // SAFETY: plain enumeration of top-level windows.
    unsafe {
        while let Ok(w) = FindWindowExW(None, after, None, None) {
            after = Some(w);
            let mut p = 0;
            GetWindowThreadProcessId(w, Some(&mut p));
            if p == pid && IsWindowVisible(w).as_bool() {
                return Some(w);
            }
        }
    }
    None
}

fn post_close(hwnd: HWND) {
    // SAFETY: posting a close request to a window of ours.
    unsafe {
        let _ = PostMessageW(
            Some(hwnd),
            windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}
