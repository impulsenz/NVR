//! Windows: GAZE runs without a console window and shows an icon in the
//! system tray (next to the clock) instead.
//!
//! Left-click opens GAZE in the browser; right-click shows a menu: open,
//! recordings folder, log file, start with Windows, restart, quit.
//! When started from a terminal the log still appears there.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::sync::OnceLock;

pub struct TrayInfo {
    /// Web address to open, e.g. http://localhost:8080/
    pub url: String,
    pub recordings: PathBuf,
    pub log: PathBuf,
    /// The settings file (passed again when starting with Windows).
    pub config: PathBuf,
}

/// Called from the tray when the user picks Quit (false) or Restart (true).
type OnQuit = Box<dyn Fn(bool) + Send + Sync>;

static INFO: OnceLock<(TrayInfo, OnQuit)> = OnceLock::new();

#[cfg(windows)]
pub use win::{attach_console, fatal, remove, start};

#[cfg(not(windows))]
pub fn attach_console() {}
#[cfg(not(windows))]
pub fn start(_info: TrayInfo, _on_quit: OnQuit) {}
#[cfg(not(windows))]
pub fn remove() {}
#[cfg(not(windows))]
pub fn fatal(_msg: &str) {}

#[cfg(windows)]
mod win {
    use super::{INFO, OnQuit, TrayInfo};
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
        Shell_NotifyIconW, ShellExecuteW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const WM_TRAY: u32 = WM_APP + 1;
    const ID_OPEN: usize = 1;
    const ID_FOLDER: usize = 2;
    const ID_LOG: usize = 3;
    const ID_AUTOSTART: usize = 4;
    const ID_RESTART: usize = 5;
    const ID_QUIT: usize = 6;
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    static HWND_: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
    static ICON: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn copy(dst: &mut [u16], s: &str) {
        let n = dst.len() - 1;
        for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(n)) {
            *d = c;
        }
    }

    /// Show the log in the terminal GAZE was started from, if any.
    pub fn attach_console() {
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }

    /// GAZE can't start: say why in a message box (there's no console).
    pub fn fatal(msg: &str) {
        let (t, c) = (w(msg), w("GAZE could not start"));
        unsafe {
            MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR);
        }
    }

    pub fn start(info: TrayInfo, on_quit: OnQuit) {
        if INFO.set((info, on_quit)).is_err() {
            return;
        }
        std::thread::Builder::new()
            .name("tray".into())
            .spawn(|| unsafe { run() })
            .ok();
    }

    /// Take the icon away (GAZE is stopping).
    pub fn remove() {
        let h = HWND_.load(Ordering::SeqCst);
        if !h.is_null() {
            unsafe {
                SendMessageW(h, WM_CLOSE, 0, 0);
            }
        }
    }

    fn open(target: &str) {
        let (op, t) = (w("open"), w(target));
        unsafe {
            ShellExecuteW(
                null_mut(),
                op.as_ptr(),
                t.as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            );
        }
    }

    fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
        let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = hwnd;
        d.uID = 1;
        d
    }

    unsafe fn add_icon(hwnd: HWND, greet: bool) {
        let mut d = icon_data(hwnd);
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = ICON.load(Ordering::SeqCst);
        copy(&mut d.szTip, "GAZE — recording your cameras");
        if greet {
            d.uFlags |= NIF_INFO;
            copy(&mut d.szInfoTitle, "GAZE is running");
            copy(
                &mut d.szInfo,
                "Click this icon to open GAZE. Right-click for more.",
            );
        }
        unsafe { Shell_NotifyIconW(NIM_ADD, &d) };
    }

    fn autostart_on() -> bool {
        let (k, v) = (w(RUN_KEY), w("GAZE"));
        let mut len = 0u32;
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                k.as_ptr(),
                v.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                &mut len,
            ) == 0
        }
    }

    fn set_autostart(on: bool) {
        let (k, v) = (w(RUN_KEY), w("GAZE"));
        unsafe {
            if on {
                let Some((info, _)) = INFO.get() else { return };
                let exe = std::env::current_exe().unwrap_or_default();
                let cfg = std::path::absolute(&info.config).unwrap_or(info.config.clone());
                let cmd = w(&format!("\"{}\" \"{}\"", exe.display(), cfg.display()));
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    k.as_ptr(),
                    v.as_ptr(),
                    REG_SZ,
                    cmd.as_ptr() as *const c_void,
                    (cmd.len() * 2) as u32,
                );
            } else {
                RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr());
            }
        }
    }

    unsafe fn menu(hwnd: HWND) {
        let Some((info, on_quit)) = INFO.get() else {
            return;
        };
        unsafe {
            let m = CreatePopupMenu();
            let add = |id: usize, text: &str, flags: u32| {
                let t = w(text);
                AppendMenuW(m, MF_STRING | flags, id, t.as_ptr());
            };
            add(ID_OPEN, "Open GAZE", 0);
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            add(ID_FOLDER, "Open recordings folder", 0);
            add(ID_LOG, "Open log file", 0);
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            add(
                ID_AUTOSTART,
                "Start with Windows",
                if autostart_on() { MF_CHECKED } else { 0 },
            );
            add(ID_RESTART, "Restart GAZE", 0);
            add(ID_QUIT, "Quit GAZE", 0);
            SetMenuDefaultItem(m, ID_OPEN as u32, 0);
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            // Needed so the menu closes when clicking elsewhere.
            SetForegroundWindow(hwnd);
            let id = TrackPopupMenu(
                m,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                p.x,
                p.y,
                0,
                hwnd,
                null(),
            ) as usize;
            DestroyMenu(m);
            match id {
                ID_OPEN => open(&info.url),
                ID_FOLDER => {
                    let _ = std::fs::create_dir_all(&info.recordings);
                    open(&info.recordings.to_string_lossy())
                }
                ID_LOG => open(&info.log.to_string_lossy()),
                ID_AUTOSTART => set_autostart(!autostart_on()),
                ID_RESTART => on_quit(true),
                ID_QUIT => {
                    let (t, c) = (
                        w(
                            "Stop GAZE?\n\nCameras will not be recorded until GAZE is started again.",
                        ),
                        w("Quit GAZE"),
                    );
                    if MessageBoxW(
                        hwnd,
                        t.as_ptr(),
                        c.as_ptr(),
                        MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
                    ) == IDYES
                    {
                        on_quit(false);
                    }
                }
                _ => {}
            }
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_TRAY => {
                    match lp as u32 {
                        WM_LBUTTONUP => {
                            if let Some((info, _)) = INFO.get() {
                                open(&info.url);
                            }
                        }
                        WM_RBUTTONUP | WM_CONTEXTMENU => menu(hwnd),
                        _ => {}
                    }
                    0
                }
                WM_CLOSE => {
                    DestroyWindow(hwnd);
                    0
                }
                WM_DESTROY => {
                    Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
                    HWND_.store(null_mut(), Ordering::SeqCst);
                    PostQuitMessage(0);
                    0
                }
                // Explorer restarted: put the icon back.
                m if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => {
                    add_icon(hwnd, false);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    unsafe fn run() {
        unsafe {
            let inst = GetModuleHandleW(null());
            let class = w("GAZE.tray");
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: null_mut(),
                hCursor: null_mut(),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
            let tc = w("TaskbarCreated");
            TASKBAR_CREATED.store(RegisterWindowMessageW(tc.as_ptr()), Ordering::SeqCst);
            let title = w("GAZE");
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                inst,
                null(),
            );
            if hwnd.is_null() {
                return;
            }
            HWND_.store(hwnd, Ordering::SeqCst);
            let png = include_bytes!("../static/tray.png");
            let size = GetSystemMetrics(SM_CXSMICON).max(16);
            let icon = CreateIconFromResourceEx(
                png.as_ptr(),
                png.len() as u32,
                1,
                0x0003_0000,
                size,
                size,
                LR_DEFAULTCOLOR,
            );
            ICON.store(icon, Ordering::SeqCst);
            add_icon(hwnd, true);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
