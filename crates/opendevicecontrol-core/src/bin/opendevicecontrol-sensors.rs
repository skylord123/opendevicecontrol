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
    use opendevicecontrol_core::smbus::pawnio::program_files;
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
        // This runs as SYSTEM or elevated, possibly from a folder the user
        // can write to: DLLs loaded by name come from System32 only, never
        // this exe's folder, the current directory or PATH.
        // SAFETY: plain Win32 call.
        unsafe {
            use windows_sys::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32};
            SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
        }
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

    /// The service logs to `sensors.log` next to its exe (one run), in
    /// `%ProgramFiles%\OpenDeviceControl\Sensors`. Not `%ProgramData%`: users
    /// may create folders there, and SYSTEM must not write through a path a
    /// user prepared.
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
        let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)) else { return };
        if let Ok(f) = std::fs::File::create(dir.join("sensors.log")) {
            let _ = log::set_logger(Box::leak(Box::new(FileLog(std::sync::Mutex::new(f)))));
            log::set_max_level(log::LevelFilter::Info);
        }
    }

    // ---- install / uninstall ----------------------------------------------
    //
    // `--install` runs elevated from wherever OpenDeviceControl is installed,
    // which in a per-user install is a folder the user can write to. So
    // nothing the service runs is used from there: the service and its
    // PawnIO modules are copied into %ProgramFiles%\OpenDeviceControl\Sensors
    // (only administrators can change it) and run from that copy, and the
    // bundled PawnIO installer is copied out of the user's reach and checked
    // against its pinned SHA-256 before it runs. %ProgramFiles% itself comes
    // from the machine's registry, not the (user-settable) environment.
    //
    // An update is staged next to the installed copy first; the running
    // service is only stopped once the new files are in place, and if the
    // new version doesn't start and answer, the previous one is put back.

    /// SHA-256 of the PawnIO installer the Windows package bundles. Keep in
    /// step with `packaging/windows/fetch-pawnio.ps1` (a test checks).
    pub(super) const PAWNIO_SETUP_SHA256: &str = "1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032";

    /// `%ProgramFiles%\OpenDeviceControl`.
    fn install_root() -> PathBuf {
        program_files().join("OpenDeviceControl")
    }

    /// `%ProgramFiles%\OpenDeviceControl\Sensors`: only administrators can change it,
    /// which matters for a program that runs as LocalSystem.
    fn install_dir() -> PathBuf {
        install_root().join("Sensors")
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
        opendevicecontrol_core::smbus::pawnio::lib_path().exists()
    }

    fn os_code(e: &windows_service::Error) -> Option<i32> {
        match e {
            windows_service::Error::Winapi(io) => io.raw_os_error(),
            _ => None,
        }
    }

    /// Stop a service and wait until it has stopped.
    fn stop_and_wait(svc: &windows_service::service::Service) -> Result<(), String> {
        let state = || svc.query_status().map(|s| s.current_state).map_err(|e| format!("querying the service: {e}"));
        if state()? == ServiceState::Stopped {
            return Ok(());
        }
        match svc.stop() {
            Ok(_) => {}
            // Not running (any more), or already stopping: wait below.
            Err(e) if matches!(os_code(&e), Some(1062) | Some(1061)) => {}
            Err(e) => return Err(format!("stopping the service: {e}")),
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if state()? == ServiceState::Stopped {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err("the service did not stop within 20 seconds".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Start a service, wait for it to run and for its pipe to answer as
    /// this version.
    fn start_and_verify(svc: &windows_service::service::Service) -> Result<(), String> {
        svc.start::<&str>(&[]).or_else(|e| match os_code(&e) {
            Some(1056) => Ok(()), // already running
            _ => Err(format!("starting the service: {e}")),
        })?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let s = svc.query_status().map_err(|e| format!("querying the service: {e}"))?;
            match s.current_state {
                ServiceState::Running => break,
                ServiceState::Stopped => return Err(format!("the service stopped right after starting ({:?})", s.exit_code)),
                _ if Instant::now() > deadline => return Err("the service did not start within 20 seconds".into()),
                _ => std::thread::sleep(Duration::from_millis(200)),
            }
        }
        // It reads every sensor once before it serves; give it time.
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last = String::new();
        while Instant::now() < deadline {
            match service::fetch_current() {
                Ok(snap) if snap.version == env!("CARGO_PKG_VERSION") => return Ok(()),
                Ok(snap) => last = format!("version {} answered, expected {}", snap.version, env!("CARGO_PKG_VERSION")),
                Err(service::FetchError::Absent) => last = "its pipe is not there".into(),
                Err(service::FetchError::Failed(e)) => last = e,
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(format!("the service is running but does not answer ({last})"))
    }

    /// Run `f`, retrying for a few seconds: a just-stopped service can hold
    /// its files for a moment.
    fn retry<T>(mut f: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
        let mut tries = 0;
        loop {
            match f() {
                Ok(v) => return Ok(v),
                Err(e) if tries >= 25 || e.kind() == std::io::ErrorKind::NotFound => return Err(e),
                Err(_) => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    fn remove_dir(dir: &Path) -> std::io::Result<()> {
        match retry(|| std::fs::remove_dir_all(dir)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }

    fn copy(from: &Path, to: &Path) -> Result<(), String> {
        retry(|| std::fs::copy(from, to)).map(|_| ()).map_err(|e| format!("copying {} to {}: {e}", from.display(), to.display()))
    }

    /// Copy the service (`exe`) and the PawnIO modules from `src_dir\pawnio`
    /// into a fresh `staging` folder. Returns how many modules it copied.
    pub(super) fn stage(exe: &Path, src_dir: &Path, staging: &Path) -> Result<usize, String> {
        remove_dir(staging).map_err(|e| format!("clearing {}: {e}", staging.display()))?;
        std::fs::create_dir_all(staging.join("pawnio")).map_err(|e| format!("creating {}: {e}", staging.display()))?;
        copy(exe, &staging.join(EXE))?;
        let mut modules = 0;
        if let Ok(rd) = std::fs::read_dir(src_dir.join("pawnio")) {
            for e in rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("bin"))) {
                copy(&e.path(), &staging.join("pawnio").join(e.file_name()))?;
                modules += 1;
            }
        }
        let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        if len(exe).is_none() || len(exe) != len(&staging.join(EXE)) {
            return Err(format!("the staged copy of {EXE} is incomplete"));
        }
        Ok(modules)
    }

    /// Put `staged` in place of `dest`, keeping the current `dest` (if any)
    /// as `backup`. On failure everything is as it was. Returns whether
    /// there was a previous version.
    pub(super) fn swap_in(staged: &Path, dest: &Path, backup: &Path) -> Result<bool, String> {
        remove_dir(backup).map_err(|e| format!("clearing {}: {e}", backup.display()))?;
        let had_old = dest.exists();
        if had_old {
            retry(|| std::fs::rename(dest, backup)).map_err(|e| format!("moving {} aside: {e}", dest.display()))?;
        }
        if let Err(e) = retry(|| std::fs::rename(staged, dest)) {
            if had_old {
                let _ = retry(|| std::fs::rename(backup, dest));
            }
            return Err(format!("moving the new version into {}: {e}", dest.display()));
        }
        Ok(had_old)
    }

    /// Undo [`swap_in`]: put `backup` back as `dest`.
    pub(super) fn swap_back(dest: &Path, backup: &Path) -> Result<(), String> {
        remove_dir(dest).map_err(|e| format!("removing {}: {e}", dest.display()))?;
        retry(|| std::fs::rename(backup, dest)).map_err(|e| format!("restoring {}: {e}", dest.display()))
    }

    /// SHA-256 of `data`, lowercase hex.
    pub(super) fn sha256_hex(data: &[u8]) -> Result<String, String> {
        use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
        let mut out = [0u8; 32];
        let len = u32::try_from(data.len()).map_err(|_| "file too large to hash".to_string())?;
        // SAFETY: the pseudo-handle needs no setup; the buffers are valid
        // for the lengths given.
        let st = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, std::ptr::null(), 0, data.as_ptr(), len, out.as_mut_ptr(), 32) };
        if st != 0 {
            return Err(format!("hashing failed (NTSTATUS {st:#x})"));
        }
        Ok(out.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Install PawnIO from the bundled installer if it isn't installed. The
    /// installer is copied into `work` (under %ProgramFiles%) and checked
    /// against its pinned hash there, so what runs is what was checked.
    fn ensure_pawnio(src_dir: &Path, work: &Path) -> Result<(), String> {
        if pawnio_installed() {
            println!("PawnIO is installed");
            return Ok(());
        }
        let setup = src_dir.join(PAWNIO_SETUP);
        if !setup.exists() {
            return Err(format!(
                "PawnIO is not installed and {PAWNIO_SETUP} is not next to opendevicecontrol-sensors.exe; install it from https://pawnio.eu"
            ));
        }
        remove_dir(work).map_err(|e| format!("clearing {}: {e}", work.display()))?;
        std::fs::create_dir_all(work).map_err(|e| format!("creating {}: {e}", work.display()))?;
        let trusted = work.join(PAWNIO_SETUP);
        let result = (|| {
            copy(&setup, &trusted)?;
            let bytes = std::fs::read(&trusted).map_err(|e| format!("reading {}: {e}", trusted.display()))?;
            let hash = sha256_hex(&bytes)?;
            if hash != PAWNIO_SETUP_SHA256 {
                return Err(format!("{PAWNIO_SETUP} is not the bundled PawnIO installer (SHA-256 {hash}); not running it"));
            }
            println!("Installing PawnIO…");
            let st = std::process::Command::new(&trusted)
                .args(["-install", "-silent"])
                .status()
                .map_err(|e| format!("running {PAWNIO_SETUP}: {e}"))?;
            if !pawnio_installed() {
                return Err(format!("PawnIO setup did not complete ({st})"));
            }
            println!("PawnIO installed");
            Ok(())
        })();
        let _ = remove_dir(work);
        result
    }

    fn same_dir(a: &Path, b: &Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    fn install() -> Result<(), String> {
        let mgr = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
        let src = std::env::current_exe().map_err(|e| e.to_string())?;
        let src_dir = src.parent().ok_or("no exe directory")?.to_path_buf();
        let root = install_root();
        let dest = install_dir();
        let exe = dest.join(EXE);
        let staging = root.join("Sensors.new");
        let backup = root.join("Sensors.old");
        // Run from the installed copy itself: its files are already in place.
        let in_place = same_dir(&src_dir, &dest);

        // 1. Everything that can fail without touching the running service.
        std::fs::create_dir_all(&root).map_err(|e| format!("creating {}: {e}", root.display()))?;
        if !in_place {
            let modules = stage(&src, &src_dir, &staging)?;
            if modules == 0 {
                println!("Note: no PawnIO modules next to {EXE}; the service will report what it can't read");
            }
        }
        if let Err(e) = ensure_pawnio(&src_dir, &root.join("Sensors.setup")) {
            let _ = remove_dir(&staging);
            return Err(e);
        }

        // 2. Stop the running copy; it must really stop before its files go.
        let access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START | ServiceAccess::CHANGE_CONFIG | ServiceAccess::DELETE;
        let existing = match mgr.open_service(SERVICE_NAME, access) {
            Ok(svc) => Some(svc),
            Err(e) if os_code(&e) == Some(1060) => None, // not installed
            Err(e) => {
                let _ = remove_dir(&staging);
                return Err(format!("opening the service: {e}"));
            }
        };
        if let Some(svc) = &existing {
            if let Err(e) = stop_and_wait(svc) {
                let _ = remove_dir(&staging);
                return Err(e);
            }
        }

        // 3. Swap the new files in.
        let had_old = if in_place {
            false
        } else {
            match swap_in(&staging, &dest, &backup) {
                Ok(had_old) => had_old,
                Err(e) => {
                    let _ = remove_dir(&staging);
                    if let Some(svc) = &existing {
                        let _ = start_and_verify(svc);
                    }
                    return Err(e);
                }
            }
        };
        println!("Installed {}", exe.display());

        // 4. Point the service at it, start it and check that it answers.
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
        let fresh = existing.is_none();
        let svc = match existing {
            Some(svc) => svc,
            None => match mgr.create_service(&info, access) {
                Ok(svc) => svc,
                Err(e) => return Err(roll_back(format!("creating the service: {e}"), None, fresh, had_old, &dest, &backup)),
            },
        };
        let result = if fresh { Ok(()) } else { svc.change_config(&info).map_err(|e| format!("updating the service: {e}")) }.and_then(|()| {
            let _ = svc.set_description(
                "Reads CPU and motherboard sensors for OpenDeviceControl through the PawnIO driver. Read-only; OpenDeviceControl gets the readings over a local pipe.",
            );
            start_and_verify(&svc)
        });
        match result {
            Ok(()) => {
                println!("Service {SERVICE_NAME} running");
                if had_old {
                    let _ = remove_dir(&backup);
                }
                remove_legacy(&mgr);
                Ok(())
            }
            Err(e) => Err(roll_back(e, Some(&svc), fresh, had_old, &dest, &backup)),
        }
    }

    /// After a failed update: put the previous version back and start it,
    /// or (on a first install) remove what was installed. Returns the error
    /// to report.
    fn roll_back(e: String, svc: Option<&windows_service::service::Service>, fresh: bool, had_old: bool, dest: &Path, backup: &Path) -> String {
        if let Some(svc) = svc {
            let _ = stop_and_wait(svc);
        }
        if fresh {
            // No service before this: don't leave a broken one behind.
            if let Some(svc) = svc {
                let _ = svc.delete();
            }
        }
        if !had_old {
            if fresh {
                let _ = remove_dir(dest);
            }
            return e;
        }
        match swap_back(dest, backup) {
            Ok(()) if fresh => e,
            Ok(()) => match svc.map(start_and_verify) {
                Some(Ok(())) | None => format!("{e}; the previous version was restored"),
                Some(Err(e2)) => format!("{e}; the previous version was restored but did not start either: {e2}"),
            },
            Err(e2) => format!("{e}; restoring the previous version failed too: {e2}"),
        }
    }

    /// Remove the service installed under the project's previous name.
    fn remove_legacy(mgr: &ServiceManager) {
        use opendevicecontrol_core::system::service::{LEGACY_INSTALL_DIR, LEGACY_SERVICE_NAME};
        if let Ok(svc) = mgr.open_service(LEGACY_SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE) {
            if let Err(e) = stop_and_wait(&svc) {
                println!("Couldn't stop the old service {LEGACY_SERVICE_NAME}: {e}");
            }
            match svc.delete() {
                Ok(()) => println!("Removed the old service {LEGACY_SERVICE_NAME}"),
                Err(e) => println!("Couldn't remove the old service {LEGACY_SERVICE_NAME}: {e}"),
            }
        }
        let old = program_files().join(LEGACY_INSTALL_DIR);
        let _ = remove_dir(&old);
        if let Some(parent) = old.parent() {
            let _ = std::fs::remove_dir(parent); // only if now empty
        }
    }

    fn uninstall() -> Result<(), String> {
        let mgr = manager(ServiceManagerAccess::CONNECT)?;
        match mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE) {
            Ok(svc) => {
                if let Err(e) = stop_and_wait(&svc) {
                    println!("{e}; removing it anyway (it goes once it stops)");
                }
                svc.delete().map_err(|e| format!("removing the service: {e}"))?;
                println!("Service {SERVICE_NAME} removed");
            }
            Err(e) if os_code(&e) == Some(1060) => println!("Service {SERVICE_NAME} was not installed"),
            Err(e) => return Err(format!("opening the service: {e}")),
        }
        let dest = install_dir();
        if dest.exists() {
            // The exe may stay locked for a moment after the service stops.
            if remove_dir(&dest).is_ok() {
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

#[cfg(all(test, windows))]
mod tests {
    use super::win::*;
    use std::path::{Path, PathBuf};

    /// A fresh scratch folder under the temp dir.
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("odc-sensors-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn pinned_pawnio_hash_matches_the_packaging_script() {
        let script = include_str!("../../../../packaging/windows/fetch-pawnio.ps1");
        assert!(script.contains(PAWNIO_SETUP_SHA256), "update PAWNIO_SETUP_SHA256 with fetch-pawnio.ps1");
    }

    #[test]
    fn sha256_known_answer() {
        assert_eq!(sha256_hex(b"abc").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn staging_copies_the_service_and_modules() {
        let d = scratch("stage");
        let src = d.join("app");
        write(&src.join("svc.exe"), "new service");
        write(&src.join("pawnio").join("A.bin"), "a");
        write(&src.join("pawnio").join("notes.txt"), "skip");
        let staging = d.join("Sensors.new");
        write(&staging.join("stale"), "left over");
        assert_eq!(stage(&src.join("svc.exe"), &src, &staging).unwrap(), 1);
        assert_eq!(read(&staging.join("opendevicecontrol-sensors.exe")), "new service");
        assert_eq!(read(&staging.join("pawnio").join("A.bin")), "a");
        assert!(!staging.join("stale").exists());
        assert!(!staging.join("pawnio").join("notes.txt").exists());
        // A missing service exe fails before anything is swapped.
        assert!(stage(&src.join("missing.exe"), &src, &staging).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn swap_keeps_the_old_version_and_can_go_back() {
        let d = scratch("swap");
        let (dest, staged, backup) = (d.join("Sensors"), d.join("Sensors.new"), d.join("Sensors.old"));
        write(&dest.join("v"), "old");
        write(&staged.join("v"), "new");
        write(&backup.join("v"), "older leftover");
        assert!(swap_in(&staged, &dest, &backup).unwrap());
        assert_eq!(read(&dest.join("v")), "new");
        assert_eq!(read(&backup.join("v")), "old");
        assert!(!staged.exists());
        swap_back(&dest, &backup).unwrap();
        assert_eq!(read(&dest.join("v")), "old");
        assert!(!backup.exists());

        // First install: nothing to keep.
        let fresh = d.join("Fresh");
        write(&staged.join("v"), "new");
        assert!(!swap_in(&staged, &fresh, &backup).unwrap());
        assert_eq!(read(&fresh.join("v")), "new");

        // Nothing staged: the installed version stays where it was.
        assert!(swap_in(&d.join("nothing"), &dest, &backup).is_err());
        assert_eq!(read(&dest.join("v")), "old");
        let _ = std::fs::remove_dir_all(&d);
    }
}
