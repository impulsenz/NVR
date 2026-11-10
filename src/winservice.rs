//! Windows: run GAZE as a Windows service, so it records from the moment
//! the computer starts (before anyone signs in) and keeps going when they
//! sign out. Set up from the tray icon ("Run as a Windows service").
//!
//! - `nvr.exe --install-service <config>` (run elevated by the tray): waits
//!   for the running GAZE to stop, creates the "GAZE" service (automatic
//!   start, restarted after a crash, with the user's PATH so ffmpeg is
//!   found) and starts it.
//! - `nvr.exe --service <config>`: what the service runs.
//! - `nvr.exe --uninstall-service` / `--restart-service` (elevated).
//! - Signed-in users get a small tray icon ("companion") that opens GAZE
//!   and can restart the service or turn it back into a normal program.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;

pub const NAME: &str = "GAZE";

/// How this process was started.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    /// Run by the Windows service manager.
    Service,
    /// Tray icon only (GAZE itself runs as a service).
    Companion,
    Install,
    Uninstall,
    RestartService,
}

/// Mode and settings file from the command line.
pub fn parse_args() -> (Mode, PathBuf, Option<PathBuf>) {
    let mut mode = Mode::Normal;
    let mut config = None;
    let mut dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            // Folder GAZE works in (where relative paths like "recordings" are).
            "--dir" => dir = args.next().map(PathBuf::from),
            "--service" => mode = Mode::Service,
            "--companion" => mode = Mode::Companion,
            "--install-service" => mode = Mode::Install,
            "--uninstall-service" => mode = Mode::Uninstall,
            "--restart-service" => mode = Mode::RestartService,
            s if !s.starts_with("--") && config.is_none() => config = Some(PathBuf::from(s)),
            _ => {}
        }
    }
    (
        mode,
        config.unwrap_or_else(|| PathBuf::from("nvr.toml")),
        dir,
    )
}

#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
pub fn installed() -> bool {
    false
}

#[cfg(windows)]
mod win {
    use super::NAME;
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicPtr, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, NO_ERROR};
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, REG_MULTI_SZ, RegSetKeyValueW};
    use windows_sys::Win32::System::Services::*;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW, SW_HIDE,
    };

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn msg(text: &str, error: bool) {
        let (t, c) = (w(text), w("GAZE"));
        unsafe {
            MessageBoxW(
                null_mut(),
                t.as_ptr(),
                c.as_ptr(),
                MB_OK
                    | if error {
                        MB_ICONERROR
                    } else {
                        MB_ICONINFORMATION
                    },
            );
        }
    }

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
        let n = w(NAME);
        let h = unsafe { OpenServiceW(scm.0, n.as_ptr(), access) };
        (!h.is_null()).then_some(Handle(h))
    }

    fn state(s: &Handle) -> u32 {
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        unsafe { QueryServiceStatus(s.0, &mut st) };
        st.dwCurrentState
    }

    /// Is GAZE set up as a Windows service?
    pub fn installed() -> bool {
        manager(SC_MANAGER_CONNECT)
            .and_then(|m| service(&m, SERVICE_QUERY_STATUS))
            .is_some()
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

    /// Run `nvr.exe <args>` as administrator (Windows asks). `wait`: until it
    /// finishes, returning its exit code. None when the user said no.
    pub fn elevate(args: &str, wait: bool) -> Option<u32> {
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
                if wait {
                    WaitForSingleObject(sei.hProcess, INFINITE);
                    GetExitCodeProcess(sei.hProcess, &mut code);
                }
                CloseHandle(sei.hProcess);
            }
        }
        Some(code)
    }

    /// `--install-service`: create and start the service (elevated).
    pub fn install(config: &Path, dir: &Path, listen: &str) -> i32 {
        let exe = std::env::current_exe().unwrap_or_default();
        let config = std::path::absolute(config).unwrap_or(config.to_path_buf());
        // The normal GAZE is stopping: wait until its web address is free.
        let t = Instant::now();
        while std::net::TcpListener::bind(listen).is_err() && t.elapsed() < Duration::from_secs(90)
        {
            std::thread::sleep(Duration::from_millis(500));
        }
        let Some(scm) = manager(SC_MANAGER_ALL_ACCESS) else {
            msg(
                "GAZE could not set itself up as a Windows service: no permission.",
                true,
            );
            return 1;
        };
        let cmd = w(&format!(
            "\"{}\" --service \"{}\" --dir \"{}\"",
            exe.display(),
            config.display(),
            dir.display()
        ));
        let (n, display) = (w(NAME), w("GAZE NVR"));
        let mut s = service(&scm, SERVICE_ALL_ACCESS);
        if s.is_none() {
            let h = unsafe {
                CreateServiceW(
                    scm.0,
                    n.as_ptr(),
                    display.as_ptr(),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    cmd.as_ptr(),
                    null(),
                    null_mut(),
                    null(),
                    null(),
                    null(),
                )
            };
            if h.is_null() {
                msg(
                    &format!(
                        "GAZE could not create its Windows service ({}).",
                        std::io::Error::last_os_error()
                    ),
                    true,
                );
                return 1;
            }
            s = Some(Handle(h));
        }
        let s = s.unwrap();
        unsafe {
            let mut d = w(
                "GAZE network video recorder: records your cameras and serves the GAZE web page.",
            );
            let desc = SERVICE_DESCRIPTIONW {
                lpDescription: d.as_mut_ptr(),
            };
            ChangeServiceConfig2W(
                s.0,
                SERVICE_CONFIG_DESCRIPTION,
                &desc as *const _ as *const c_void,
            );
            // Restart after a crash: 5 s, 5 s, then 1 min; forget after a day.
            let mut actions = [
                SC_ACTION {
                    Type: SC_ACTION_RESTART,
                    Delay: 5000,
                },
                SC_ACTION {
                    Type: SC_ACTION_RESTART,
                    Delay: 5000,
                },
                SC_ACTION {
                    Type: SC_ACTION_RESTART,
                    Delay: 60000,
                },
            ];
            let fail = SERVICE_FAILURE_ACTIONSW {
                dwResetPeriod: 86400,
                lpRebootMsg: null_mut(),
                lpCommand: null_mut(),
                cActions: actions.len() as u32,
                lpsaActions: actions.as_mut_ptr(),
            };
            ChangeServiceConfig2W(
                s.0,
                SERVICE_CONFIG_FAILURE_ACTIONS,
                &fail as *const _ as *const c_void,
            );
            // The service runs as the system account: give it this user's
            // PATH so ffmpeg (and nvidia-smi, rclone) are found.
            if let Ok(path) = std::env::var("PATH") {
                let key = w(&format!(r"SYSTEM\CurrentControlSet\Services\{NAME}"));
                let name = w("Environment");
                let mut data: Vec<u16> = format!("PATH={path}").encode_utf16().collect();
                data.extend([0, 0]);
                RegSetKeyValueW(
                    HKEY_LOCAL_MACHINE,
                    key.as_ptr(),
                    name.as_ptr(),
                    REG_MULTI_SZ,
                    data.as_ptr() as *const c_void,
                    (data.len() * 2) as u32,
                );
            }
            if state(&s) != SERVICE_RUNNING && StartServiceW(s.0, 0, null()) == 0 {
                msg(
                    &format!(
                        "The GAZE service was created but did not start ({}). See the log file in nvr-data.",
                        std::io::Error::last_os_error()
                    ),
                    true,
                );
                return 1;
            }
        }
        0
    }

    fn stop(s: &Handle) {
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        unsafe { ControlService(s.0, SERVICE_CONTROL_STOP, &mut st) };
        wait_state(s, SERVICE_STOPPED, Duration::from_secs(60));
    }

    /// `--uninstall-service` (elevated): stop and remove the service.
    pub fn uninstall() -> i32 {
        let Some(scm) = manager(SC_MANAGER_ALL_ACCESS) else {
            return 1;
        };
        let Some(s) = service(&scm, SERVICE_ALL_ACCESS) else {
            return 0;
        };
        stop(&s);
        if unsafe { DeleteService(s.0) } == 0 {
            msg(
                &format!(
                    "GAZE could not remove its Windows service ({}).",
                    std::io::Error::last_os_error()
                ),
                true,
            );
            return 1;
        }
        0
    }

    /// `--restart-service` (elevated).
    pub fn restart() -> i32 {
        let Some(scm) = manager(SC_MANAGER_ALL_ACCESS) else {
            return 1;
        };
        let Some(s) = service(&scm, SERVICE_ALL_ACCESS) else {
            return 1;
        };
        stop(&s);
        if unsafe { StartServiceW(s.0, 0, null()) } == 0 {
            msg(
                &format!(
                    "The GAZE service did not start ({}).",
                    std::io::Error::last_os_error()
                ),
                true,
            );
            return 1;
        }
        0
    }

    // ---------- the service itself ----------

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
            dwControlsAccepted: if state == SERVICE_RUNNING {
                SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
            } else {
                0
            },
            dwWin32ExitCode: NO_ERROR,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: wait_ms,
        };
        unsafe { SetServiceStatus(h, &st) };
    }

    unsafe extern "system" fn handler(
        control: u32,
        _t: u32,
        _d: *mut c_void,
        _c: *mut c_void,
    ) -> u32 {
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
        let n = w(NAME);
        let h = unsafe { RegisterServiceCtrlHandlerExW(n.as_ptr(), Some(handler), null()) };
        STATUS.store(h, Ordering::SeqCst);
        report(SERVICE_START_PENDING, 10_000);
        report(SERVICE_RUNNING, 0);
        if let Some((run, _)) = RUN.get() {
            run();
        }
        report(SERVICE_STOPPED, 0);
    }

    /// Hand this process to the Windows service manager: `run` is GAZE
    /// (returns when it has stopped), `stop` asks it to stop.
    pub fn run_service(
        run: impl Fn() + Send + Sync + 'static,
        stop: impl Fn() + Send + Sync + 'static,
    ) -> bool {
        let _ = RUN.set((Box::new(run), Box::new(stop)));
        let mut n = w(NAME);
        let table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: n.as_mut_ptr(),
                lpServiceProc: Some(service_main),
            },
            SERVICE_TABLE_ENTRYW {
                lpServiceName: null_mut(),
                lpServiceProc: None,
            },
        ];
        unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) != 0 }
    }
}
