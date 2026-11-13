//! `opendevicecontrol-sensors`: OpenDeviceControl's hardware sensor service for Windows.
//!
//! OpenDeviceControl runs unprivileged in the tray; reading the CPU's thermal
//! registers (and later the motherboard's Super I/O chip) needs the PawnIO
//! driver, which only administrators may open. This service runs as
//! LocalSystem and hands its readings to OpenDeviceControl over a read-only local
//! pipe (see `opendevicecontrol_core::system::service`).
//!
//! ```text
//! opendevicecontrol-sensors --install     install or update (administrator): copies the
//!                               service to %ProgramFiles%\OpenDeviceControl\Sensors,
//!                               installs PawnIO if needed, starts the service
//! opendevicecontrol-sensors --uninstall   stop and remove the service (administrator)
//! opendevicecontrol-sensors --status      what the running service reads
//! opendevicecontrol-sensors --console     serve in this console (administrator; testing)
//! opendevicecontrol-sensors --service     entry point used by the Service Control Manager
//! ```

#[cfg(not(windows))]
fn main() {
    eprintln!("opendevicecontrol-sensors is for Windows. On Linux, OpenDeviceControl reads the kernel's hwmon sensors directly.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    win::main();
}

#[cfg(windows)]
mod win {
    use opendevicecontrol_core::system::service::{self, SERVICE_DISPLAY_NAME, SERVICE_NAME};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo,
        ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    const EXE: &str = "opendevicecontrol-sensors.exe";
    const PAWNIO_SETUP: &str = "PawnIO_setup.exe";

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let r = match args.first().map(String::as_str) {
            Some("--service") => {
                log_to_file();
                service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(|e| format!("not started by the Service Control Manager: {e}"))
            }
            Some("--install") => install(),
            Some("--uninstall") => uninstall(),
            Some("--status") => status(),
            Some("--console") => console(),
            _ => {
                println!("{}", include_str!("opendevicecontrol-sensors.txt"));
                Ok(())
            }
        };
        if let Err(e) = r {
            eprintln!("opendevicecontrol-sensors: {e}");
            std::process::exit(1);
        }
    }

    // ---- the service ----------------------------------------------------

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            log::error!("service failed: {e}");
        }
    }

    fn run_service() -> windows_service::Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = service_control_handler::register(SERVICE_NAME, move |c| match c {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                stop2.store(true, Ordering::Relaxed);
                service::wake();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })?;
        let status = |state, accept| ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(3),
            process_id: None,
        };
        handle.set_service_status(status(ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN))?;
        log::info!("OpenDeviceControl sensor service {} started", env!("CARGO_PKG_VERSION"));
        for line in service::snapshot().status {
            log::info!("{line}");
        }
        while !stop.load(Ordering::Relaxed) {
            if let Err(e) = service::serve(&stop) {
                log::error!("pipe: {e}");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
        log::info!("stopped");
        handle.set_service_status(status(ServiceState::Stopped, ServiceControlAccept::empty()))?;
        Ok(())
    }

    /// The service logs to `%ProgramData%\OpenDeviceControl\sensors.log` (one run).
    fn log_to_file() {
        struct FileLog(std::sync::Mutex<std::fs::File>);
        impl log::Log for FileLog {
            fn enabled(&self, m: &log::Metadata) -> bool {
                m.level() <= log::Level::Info
            }
            fn log(&self, r: &log::Record) {
                if self.enabled(r.metadata()) {
                    use std::io::Write;
                    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    if let Ok(mut f) = self.0.lock() {
                        let _ = writeln!(f, "{secs} {} {}", r.level(), r.args());
                    }
                }
            }
            fn flush(&self) {}
        }
        let dir = PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into())).join("OpenDeviceControl");
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(f) = std::fs::File::create(dir.join("sensors.log")) {
            let _ = log::set_logger(Box::leak(Box::new(FileLog(std::sync::Mutex::new(f)))));
            log::set_max_level(log::LevelFilter::Info);
        }
    }

    // ---- install / uninstall ----------------------------------------------

    /// `%ProgramFiles%\OpenDeviceControl\Sensors`: only administrators can change it,
    /// which matters for a program that runs as LocalSystem.
    fn install_dir() -> PathBuf {
        PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into())).join("OpenDeviceControl").join("Sensors")
    }

    fn manager(access: ServiceManagerAccess) -> Result<ServiceManager, String> {
        ServiceManager::local_computer(None::<&str>, access).map_err(|e| match e {
            windows_service::Error::Winapi(io) if io.raw_os_error() == Some(5) => {
                "administrator rights are needed (run it from an elevated prompt)".to_string()
            }
            e => format!("could not open the Service Control Manager: {e}"),
        })
    }

    fn pawnio_installed() -> bool {
        let pf = std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into());
        PathBuf::from(pf).join("PawnIO").join("PawnIOLib.dll").exists()
    }

    fn stop_and_wait(svc: &windows_service::service::Service) {
        if svc.query_status().is_ok_and(|s| s.current_state != ServiceState::Stopped) {
            let _ = svc.stop();
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline && svc.query_status().is_ok_and(|s| s.current_state != ServiceState::Stopped) {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    /// Copy a file, retrying while a just-stopped service still holds it.
    fn copy(from: &Path, to: &Path) -> Result<(), String> {
        let mut last = None;
        for _ in 0..25 {
            match std::fs::copy(from, to) {
                Ok(_) => return Ok(()),
                Err(e) => last = Some(e),
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(format!("copying {} to {}: {}", from.display(), to.display(), last.map_or_else(String::new, |e| e.to_string())))
    }

    fn install() -> Result<(), String> {
        let mgr = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
        let src = std::env::current_exe().map_err(|e| e.to_string())?;
        let src_dir = src.parent().ok_or("no exe directory")?.to_path_buf();
        let dest = install_dir();

        // An older copy is replaced in place: stop it first.
        let access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START | ServiceAccess::CHANGE_CONFIG;
        let existing = mgr.open_service(SERVICE_NAME, access).ok();
        if let Some(svc) = &existing {
            stop_and_wait(svc);
        }

        std::fs::create_dir_all(dest.join("pawnio")).map_err(|e| format!("creating {}: {e}", dest.display()))?;
        let exe = dest.join(EXE);
        if src != exe {
            copy(&src, &exe)?;
            // The signed PawnIO modules, from next to this exe.
            if let Ok(rd) = std::fs::read_dir(src_dir.join("pawnio")) {
                for e in rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("bin"))) {
                    copy(&e.path(), &dest.join("pawnio").join(e.file_name()))?;
                }
            }
        }
        println!("Installed {}", exe.display());

        if pawnio_installed() {
            println!("PawnIO is installed");
        } else {
            let setup = src_dir.join(PAWNIO_SETUP);
            if !setup.exists() {
                return Err(format!(
                    "PawnIO is not installed and {PAWNIO_SETUP} is not next to opendevicecontrol-sensors.exe; install it from https://pawnio.eu"
                ));
            }
            println!("Installing PawnIO…");
            let st = std::process::Command::new(&setup).args(["-install", "-silent"]).status().map_err(|e| format!("running {PAWNIO_SETUP}: {e}"))?;
            if !pawnio_installed() {
                return Err(format!("PawnIO setup did not complete ({st})"));
            }
            println!("PawnIO installed");
        }

        let info = ServiceInfo {
            name: SERVICE_NAME.into(),
            display_name: SERVICE_DISPLAY_NAME.into(),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe.clone(),
            launch_arguments: vec!["--service".into()],
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        };
        let svc = match existing {
            Some(svc) => {
                svc.change_config(&info).map_err(|e| format!("updating the service: {e}"))?;
                svc
            }
            None => mgr.create_service(&info, access).map_err(|e| format!("creating the service: {e}"))?,
        };
        let _ = svc.set_description(
            "Reads CPU and motherboard sensors for OpenDeviceControl through the PawnIO driver. Read-only; OpenDeviceControl gets the readings over a local pipe.",
        );
        svc.start::<&str>(&[]).or_else(|e| match &e {
            // Already running.
            windows_service::Error::Winapi(io) if io.raw_os_error() == Some(1056) => Ok(()),
            _ => Err(format!("starting the service: {e}")),
        })?;
        println!("Service {SERVICE_NAME} running");
        remove_legacy(&mgr);
        Ok(())
    }

    /// Remove the service installed under the project's previous name.
    fn remove_legacy(mgr: &ServiceManager) {
        use opendevicecontrol_core::system::service::{LEGACY_INSTALL_DIR, LEGACY_SERVICE_NAME};
        if let Ok(svc) = mgr.open_service(LEGACY_SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE) {
            stop_and_wait(&svc);
            match svc.delete() {
                Ok(()) => println!("Removed the old service {LEGACY_SERVICE_NAME}"),
                Err(e) => println!("Couldn't remove the old service {LEGACY_SERVICE_NAME}: {e}"),
            }
        }
        let old = PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into())).join(LEGACY_INSTALL_DIR);
        for _ in 0..25 {
            if !old.exists() || std::fs::remove_dir_all(&old).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if let Some(parent) = old.parent() {
            let _ = std::fs::remove_dir(parent); // only if now empty
        }
    }

    fn uninstall() -> Result<(), String> {
        let mgr = manager(ServiceManagerAccess::CONNECT)?;
        match mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE) {
            Ok(svc) => {
                stop_and_wait(&svc);
                svc.delete().map_err(|e| format!("removing the service: {e}"))?;
                println!("Service {SERVICE_NAME} removed");
            }
            Err(windows_service::Error::Winapi(io)) if io.raw_os_error() == Some(1060) => println!("Service {SERVICE_NAME} was not installed"),
            Err(e) => return Err(format!("opening the service: {e}")),
        }
        let dest = install_dir();
        if dest.exists() {
            // The exe may stay locked for a moment after the service stops.
            let mut ok = false;
            for _ in 0..25 {
                if std::fs::remove_dir_all(&dest).is_ok() {
                    ok = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if ok {
                if let Some(parent) = dest.parent() {
                    let _ = std::fs::remove_dir(parent); // only if empty
                }
                println!("Removed {}", dest.display());
            } else {
                println!("Could not remove {} (remove it by hand)", dest.display());
            }
        }
        // PawnIO stays: other tools (LibreHardwareMonitor, FanControl,
        // OpenRGB) may use it. It has its own uninstaller.
        Ok(())
    }

    fn status() -> Result<(), String> {
        match service::fetch() {
            Ok(snap) => {
                println!("OpenDeviceControl sensor service {}", snap.version);
                for l in &snap.status {
                    println!("  {l}");
                }
                for t in snap.trees {
                    let t = t.into_tree();
                    for h in &t.hardware {
                        println!("  [{}] {}", t.source, h.name);
                        for s in &h.sensors {
                            println!("      {:<28} {:>9.2} {}", s.label, s.value, s.kind.unit());
                        }
                    }
                }
                Ok(())
            }
            Err(service::FetchError::Absent) => Err("the service is not running".into()),
            Err(service::FetchError::Failed(e)) => Err(e),
        }
    }

    /// Serve from this console (an elevated one), for testing.
    fn console() -> Result<(), String> {
        let snap = service::snapshot();
        for l in &snap.status {
            println!("{l}");
        }
        println!("Serving on {} (Ctrl+C to stop)", service::PIPE);
        let stop = AtomicBool::new(false);
        service::serve(&stop).map_err(|e| e.to_string())
    }
}
