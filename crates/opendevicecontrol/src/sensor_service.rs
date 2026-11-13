//! Installing and removing the sensor service (`opendevicecontrol-sensors.exe`, next
//! to opendevicecontrol.exe) from Settings. Windows asks for administrator rights.

/// Run `opendevicecontrol-sensors.exe <arg>` elevated and wait for it. Returns a line
/// for the Settings page.
#[cfg(windows)]
pub fn run_elevated(arg: &str) -> String {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let Some(exe) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("opendevicecontrol-sensors.exe"))) else {
        return "Can't find OpenDeviceControl's folder.".into();
    };
    if !exe.exists() {
        return format!("{} is missing; reinstall OpenDeviceControl.", exe.display());
    }
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file, params) = (wide("runas"), wide(&exe.to_string_lossy()), wide(arg));
    // SAFETY: plain Win32 calls; the strings outlive the call and the
    // process handle is closed.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
            return "Cancelled (administrator rights are needed).".into();
        }
        let mut code = 1u32;
        if WaitForSingleObject(info.hProcess, 120_000) == WAIT_OBJECT_0 {
            GetExitCodeProcess(info.hProcess, &mut code);
        }
        CloseHandle(info.hProcess);
        match (arg, code) {
            ("--install", 0) => "Sensor service installed and running.".into(),
            ("--uninstall", 0) => "Sensor service removed.".into(),
            (_, c) => format!("opendevicecontrol-sensors {arg} failed (exit code {c}); see %ProgramData%\\OpenDeviceControl\\sensors.log or run it from an elevated prompt."),
        }
    }
}

#[cfg(not(windows))]
pub fn run_elevated(_arg: &str) -> String {
    "The sensor service is for Windows.".into()
}
