//! The watchdog's Windows parts, so it is independent of GAZE:
//!
//! - **its own Windows service** ("GazeWatchdog": starts with the computer,
//!   before anyone signs in, and Windows restarts it if it ever stops);
//! - **its own icon** in the notification area (next to the clock): status
//!   at a glance, and a menu to open GAZE, restart it, pause watching, see
//!   the log. It's the green-tick GAZE icon, not GAZE's.
//!
//! Not Windows: the same names exist and do nothing.
#![cfg_attr(not(windows), allow(dead_code, unused_variables))]

use std::path::{Path, PathBuf};

pub const SERVICE: &str = "GazeWatchdog";

/// What the icon needs from the rest of the program.
pub struct TrayCfg {
    pub data: PathBuf,
    /// The watchdog itself runs as a service: this is only the icon.
    pub companion: bool,
    /// Leave a command for the watchdog.
    pub command: fn(&Path, &str),
    /// Switch watching on or off.
    pub toggle_pause: fn(&Path),
    /// GAZE's web address on this computer.
    pub gaze_url: fn(&Path) -> String,
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn is_admin() -> bool {
        false
    }
    pub fn elevate_and_wait(_args: &str) -> Option<u32> {
        None
    }
    pub fn service_installed() -> bool {
        false
    }
    pub fn service_install(_exe: &Path, _data: &Path) -> Result<(), String> {
        Err("The watchdog service is for Windows".into())
    }
    pub fn service_uninstall() -> Result<(), String> {
        Ok(())
    }
    pub fn run_service(_run: impl Fn() + Send + Sync + 'static, _stop: impl Fn() + Send + Sync + 'static) -> bool {
        false
    }
    pub fn tray_run(_cfg: TrayCfg) {}
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, NO_ERROR, POINT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Services::*;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        IsUserAnAdmin, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, SEE_MASK_NOCLOSEPROCESS,
        SHELLEXECUTEINFOW, Shell_NotifyIconW, ShellExecuteExW, ShellExecuteW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn copy(dst: &mut [u16], s: &str) {
        let n = dst.len() - 1;
        for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(n)) {
            *d = c;
        }
    }

    pub fn is_admin() -> bool {
        unsafe { IsUserAnAdmin() != 0 }
    }

    /// Run this program again as administrator (Windows asks) and wait for
    /// it: its exit code, or None when the user said no.
    pub fn elevate_and_wait(args: &str) -> Option<u32> {
        let exe = std::env::current_exe().ok()?;
        let (verb, file, params) = (w("runas"), w(&exe.to_string_lossy()), w(args));
        let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        sei.fMask = SEE_MASK_NOCLOSEPROCESS;
        sei.lpVerb = verb.as_ptr();
        sei.lpFile = file.as_ptr();
        sei.lpParameters = params.as_ptr();
        sei.nShow = SW_HIDE;
        if unsafe { ShellExecuteExW(&mut sei) } == 0 {
            return None;
        }
        let mut code = 0u32;
        if !sei.hProcess.is_null() {
            unsafe {
                WaitForSingleObject(sei.hProcess, INFINITE);
                GetExitCodeProcess(sei.hProcess, &mut code);
                CloseHandle(sei.hProcess);
            }
        }
        Some(code)
    }

    // ------------------------------------------------------------ the service

    struct Handle(SC_HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseServiceHandle(self.0) };
            }
        }
    }

    fn manager(access: u32) -> Option<Handle> {
        let h = unsafe { OpenSCManagerW(null(), null(), access) };
        (!h.is_null()).then_some(Handle(h))
    }

    fn service(scm: &Handle, access: u32) -> Option<Handle> {
        let n = w(SERVICE);
        let h = unsafe { OpenServiceW(scm.0, n.as_ptr(), access) };
        (!h.is_null()).then_some(Handle(h))
    }

    fn state(s: &Handle) -> u32 {
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        unsafe { QueryServiceStatus(s.0, &mut st) };
        st.dwCurrentState
    }

    fn wait_state(s: &Handle, want: u32, max: Duration) -> bool {
        let t = Instant::now();
        while t.elapsed() < max {
            if state(s) == want {
                return true;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        false
    }

    pub fn service_installed() -> bool {
        manager(SC_MANAGER_CONNECT).and_then(|m| service(&m, SERVICE_QUERY_STATUS)).is_some()
    }

    /// Create the service (automatic start, restarted by Windows if it ever
    /// stops) and start it. Needs administrator rights.
    pub fn service_install(exe: &Path, data: &Path) -> Result<(), String> {
        let scm = manager(SC_MANAGER_ALL_ACCESS).ok_or("no permission to set up a Windows service (run as administrator)")?;
        let cmd = w(&format!("\"{}\" --service --data \"{}\"", exe.display(), data.display()));
        let (n, display) = (w(SERVICE), w("GAZE Watchdog"));
        let mut s = service(&scm, SERVICE_ALL_ACCESS);
        if s.is_none() {
            let h = unsafe {
                CreateServiceW(scm.0, n.as_ptr(), display.as_ptr(), SERVICE_ALL_ACCESS, SERVICE_WIN32_OWN_PROCESS, SERVICE_AUTO_START, SERVICE_ERROR_NORMAL, cmd.as_ptr(), null(), null_mut(), null(), null(), null())
            };
            if h.is_null() {
                return Err(format!("could not create the Windows service ({})", std::io::Error::last_os_error()));
            }
            s = Some(Handle(h));
        }
        let s = s.unwrap();
        unsafe {
            let mut d = w("Keeps GAZE running: opens it again if it crashes, hangs or is closed.");
            let desc = SERVICE_DESCRIPTIONW { lpDescription: d.as_mut_ptr() };
            ChangeServiceConfig2W(s.0, SERVICE_CONFIG_DESCRIPTION, &desc as *const _ as *const c_void);
            // Windows restarts the watchdog itself if it ever stops: 5 s, 5 s, then 1 min.
            let mut actions = [SC_ACTION { Type: SC_ACTION_RESTART, Delay: 5000 }, SC_ACTION { Type: SC_ACTION_RESTART, Delay: 5000 }, SC_ACTION { Type: SC_ACTION_RESTART, Delay: 60000 }];
            let fail = SERVICE_FAILURE_ACTIONSW { dwResetPeriod: 86400, lpRebootMsg: null_mut(), lpCommand: null_mut(), cActions: actions.len() as u32, lpsaActions: actions.as_mut_ptr() };
            ChangeServiceConfig2W(s.0, SERVICE_CONFIG_FAILURE_ACTIONS, &fail as *const _ as *const c_void);
            if state(&s) != SERVICE_RUNNING && StartServiceW(s.0, 0, null()) == 0 {
                return Err(format!("the service was created but did not start ({})", std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    pub fn service_uninstall() -> Result<(), String> {
        let Some(scm) = manager(SC_MANAGER_ALL_ACCESS) else { return Err("no permission to remove the Windows service (run as administrator)".into()) };
        let Some(s) = service(&scm, SERVICE_ALL_ACCESS) else { return Ok(()) };
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        unsafe { ControlService(s.0, SERVICE_CONTROL_STOP, &mut st) };
        wait_state(&s, SERVICE_STOPPED, Duration::from_secs(30));
        if unsafe { DeleteService(s.0) } == 0 {
            return Err(format!("could not remove the Windows service ({})", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    type Runner = Box<dyn Fn() + Send + Sync>;
    static RUN: OnceLock<(Runner, Runner)> = OnceLock::new();
    static STATUS: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

    fn report(state: u32, wait_ms: u32) {
        let h = STATUS.load(Ordering::SeqCst);
        if h.is_null() {
            return;
        }
        let st = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: state,
            dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 },
            dwWin32ExitCode: NO_ERROR,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: wait_ms,
        };
        unsafe { SetServiceStatus(h, &st) };
    }

    unsafe extern "system" fn handler(control: u32, _t: u32, _d: *mut c_void, _c: *mut c_void) -> u32 {
        match control {
            SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
                report(SERVICE_STOP_PENDING, 30_000);
                if let Some((_, stop)) = RUN.get() {
                    stop();
                }
                NO_ERROR
            }
            _ => NO_ERROR,
        }
    }

    unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
        let n = w(SERVICE);
        let h = unsafe { RegisterServiceCtrlHandlerExW(n.as_ptr(), Some(handler), null()) };
        STATUS.store(h, Ordering::SeqCst);
        report(SERVICE_START_PENDING, 10_000);
        report(SERVICE_RUNNING, 0);
        if let Some((run, _)) = RUN.get() {
            run();
        }
        report(SERVICE_STOPPED, 0);
    }

    /// Hand this process to the Windows service manager: `run` is the
    /// watchdog (returns when it has stopped), `stop` asks it to stop.
    pub fn run_service(run: impl Fn() + Send + Sync + 'static, stop: impl Fn() + Send + Sync + 'static) -> bool {
        let _ = RUN.set((Box::new(run), Box::new(stop)));
        let mut n = w(SERVICE);
        let table = [
            SERVICE_TABLE_ENTRYW { lpServiceName: n.as_mut_ptr(), lpServiceProc: Some(service_main) },
            SERVICE_TABLE_ENTRYW { lpServiceName: null_mut(), lpServiceProc: None },
        ];
        unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) != 0 }
    }

    // --------------------------------------------------------------- the icon

    const WM_TRAY: u32 = WM_APP + 1;
    const ID_OPEN: usize = 1;
    const ID_SETTINGS: usize = 2;
    const ID_RESTART: usize = 3;
    const ID_PAUSE: usize = 4;
    const ID_LOG: usize = 5;
    const ID_EXIT: usize = 6;
    const TIMER: usize = 1;

    static CFG: OnceLock<TrayCfg> = OnceLock::new();
    static ICON: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

    /// What the status file says: (a line for the icon's tip and menu, is it fine).
    fn status_line(data: &Path) -> String {
        let st: Option<serde_json::Value> = std::fs::read_to_string(super::super::dir_of(data).join("status.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
        let fresh = st.as_ref().and_then(|s| s["updated"].as_i64()).is_some_and(|t| chrono::Utc::now().timestamp_millis() - t < 20_000);
        let Some(st) = st.filter(|_| fresh) else { return "The watchdog is not running".into() };
        match st["state"].as_str().unwrap_or("") {
            "watching" => "Watching GAZE — it's running and answering".into(),
            "starting" => "Opening GAZE…".into(),
            "waiting" => "GAZE isn't running: opening it in a moment".into(),
            "stopped" => "GAZE was closed on purpose (left closed)".into(),
            "cooldown" => "Too many restarts: waiting before trying again".into(),
            "paused" => "Watching is switched off".into(),
            "updating" => "Updating the watchdog…".into(),
            s => s.to_string(),
        }
    }

    fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
        let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = hwnd;
        d.uID = 1;
        d
    }

    unsafe fn add_icon(hwnd: HWND) {
        let mut d = icon_data(hwnd);
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = ICON.load(Ordering::SeqCst);
        if let Some(c) = CFG.get() {
            copy(&mut d.szTip, &format!("GAZE Watchdog — {}", status_line(&c.data)));
        }
        unsafe { Shell_NotifyIconW(NIM_ADD, &d) };
    }

    unsafe fn refresh_tip(hwnd: HWND) {
        let Some(c) = CFG.get() else { return };
        let mut d = icon_data(hwnd);
        d.uFlags = NIF_TIP;
        copy(&mut d.szTip, &format!("GAZE Watchdog — {}", status_line(&c.data)));
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) };
    }

    fn open(target: &str) {
        let (op, t) = (w("open"), w(target));
        unsafe {
            ShellExecuteW(null_mut(), op.as_ptr(), t.as_ptr(), null(), null(), SW_SHOWNORMAL);
        }
    }

    fn ask(hwnd: HWND, text: &str, title: &str) -> bool {
        let (t, c) = (w(text), w(title));
        unsafe { MessageBoxW(hwnd, t.as_ptr(), c.as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES }
    }

    unsafe fn menu(hwnd: HWND) {
        let Some(cfg) = CFG.get() else { return };
        let enabled = super::super::load_settings(&cfg.data).enabled;
        unsafe {
            let m = CreatePopupMenu();
            let add = |id: usize, text: &str, flags: u32| {
                let t = w(text);
                AppendMenuW(m, MF_STRING | flags, id, t.as_ptr());
            };
            add(ID_OPEN, "Open GAZE", 0);
            add(ID_SETTINGS, "Watchdog settings…", 0);
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            add(0, &status_line(&cfg.data), MF_GRAYED);
            add(ID_RESTART, "Restart GAZE now", 0);
            add(ID_PAUSE, "Watch GAZE (switch off to pause)", if enabled { MF_CHECKED } else { 0 });
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            add(ID_LOG, "Open the watchdog log", 0);
            add(ID_EXIT, if cfg.companion { "Hide this icon" } else { "Exit the watchdog" }, 0);
            SetMenuDefaultItem(m, ID_OPEN as u32, 0);
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            // Needed so the menu closes when clicking elsewhere.
            SetForegroundWindow(hwnd);
            let id = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, p.x, p.y, 0, hwnd, null()) as usize;
            DestroyMenu(m);
            let url = (cfg.gaze_url)(&cfg.data);
            match id {
                ID_OPEN => open(&url),
                ID_SETTINGS => open(&format!("{url}#settings/watchdog")),
                ID_RESTART => {
                    if ask(hwnd, "Close GAZE and open it again now?", "GAZE Watchdog") {
                        (cfg.command)(&cfg.data, "restart-gaze");
                    }
                }
                ID_PAUSE => (cfg.toggle_pause)(&cfg.data),
                ID_LOG => open(&super::super::dir_of(&cfg.data).join("watchdog.log").to_string_lossy()),
                ID_EXIT => {
                    if cfg.companion || ask(hwnd, "Stop the watchdog? GAZE is then no longer watched, and not opened again if it stops.", "GAZE Watchdog") {
                        Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
                        if !cfg.companion {
                            let _ = std::fs::remove_file(super::super::dir_of(&cfg.data).join("watchdog.lock"));
                        }
                        std::process::exit(0);
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
                            if let Some(c) = CFG.get() {
                                open(&(c.gaze_url)(&c.data));
                            }
                        }
                        WM_RBUTTONUP | WM_CONTEXTMENU => menu(hwnd),
                        _ => {}
                    }
                    0
                }
                WM_TIMER => {
                    refresh_tip(hwnd);
                    0
                }
                WM_DESTROY => {
                    Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
                    PostQuitMessage(0);
                    0
                }
                // Explorer restarted: put the icon back.
                m if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => {
                    add_icon(hwnd);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    /// Show the icon and handle it (returns when the window is closed).
    pub fn tray_run(cfg: TrayCfg) {
        if CFG.set(cfg).is_err() {
            return;
        }
        unsafe {
            let inst = GetModuleHandleW(null());
            let class = w("GAZE.watchdog.tray");
            let wc = WNDCLASSW { style: 0, lpfnWndProc: Some(wndproc), cbClsExtra: 0, cbWndExtra: 0, hInstance: inst, hIcon: null_mut(), hCursor: null_mut(), hbrBackground: null_mut(), lpszMenuName: null(), lpszClassName: class.as_ptr() };
            RegisterClassW(&wc);
            let tc = w("TaskbarCreated");
            TASKBAR_CREATED.store(RegisterWindowMessageW(tc.as_ptr()), Ordering::SeqCst);
            let title = w("GAZE Watchdog");
            let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0, 0, 0, 0, 0, null_mut(), null_mut(), inst, null());
            if hwnd.is_null() {
                return;
            }
            let png = include_bytes!("../../assets/watchdog-tray.png");
            let size = GetSystemMetrics(SM_CXSMICON).max(16);
            let icon = CreateIconFromResourceEx(png.as_ptr(), png.len() as u32, 1, 0x0003_0000, size, size, LR_DEFAULTCOLOR);
            ICON.store(icon, Ordering::SeqCst);
            add_icon(hwnd);
            SetTimer(hwnd, TIMER, 4000, None);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

pub use imp::*;
