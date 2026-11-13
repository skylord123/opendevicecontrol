// No console window for the GUI build on Windows. The `console` feature
// builds opendevicecontrol-cli.exe instead, for command-line use (--probe, --headless).
#![cfg_attr(all(windows, not(debug_assertions), not(feature = "console")), windows_subsystem = "windows")]

mod audio;
mod autostart;
mod config;
mod conflicts;
mod gui;
mod gui_keymap;
mod gui_peripheral;
mod gui_sensors;
mod hass;
mod icon;
mod icons;
mod inject;
mod ipc;
mod lighting;
mod service;
mod tray;
mod sensor_service;

use parking_lot::RwLock;
use std::sync::Arc;

const HELP: &str = "\
opendevicecontrol — open-source Corsair device monitor/controller with Home Assistant integration

USAGE: opendevicecontrol [OPTIONS]

OPTIONS:
    --minimized      Start hidden in the system tray
    --headless       No window or tray; run the device service and MQTT bridge only
                     (for systemd, servers, or debugging). Stop with Ctrl+C / SIGTERM.
    --page ID        Open on a device page (device id, e.g. k55) or the settings page
    --probe          Print one reading of every device and exit
    --quit           Ask a running opendevicecontrol to exit cleanly (restores hardware fan
                     control) and wait for it. Exit code 0 when it stopped and handed
                     its devices back, 1 if none was running, 2 if it did not stop in
                     time, 3 if it stopped but could not hand a device back
    --export-icon P [SIZE]
                     Write the application icon to P (PNG of SIZE px, 16 to 1024,
                     default 256, or a multi-size .ico if P ends in .ico) and exit
    -v, --verbose    Debug logging
    -h, --help       Show this help
    -V, --version    Show version
";

/// `file`: also write the log file. Only the instance that owns the devices
/// does, so a second launch does not rotate away the running one's log.
fn init_logging(verbose: bool, file: bool) {
    use simplelog::*;
    let level = if verbose { LevelFilter::Debug } else { LevelFilter::Info };
    let cfg = ConfigBuilder::new()
        .add_filter_allow_str("opendevicecontrol")
        .set_time_format_rfc3339()
        .set_time_offset_to_local()
        .unwrap_or_else(|b| b)
        .build();
    let mut loggers: Vec<Box<dyn SharedLogger>> =
        vec![TermLogger::new(level, cfg.clone(), TerminalMode::Stderr, ColorChoice::Auto)];
    let dir = config::data_dir();
    if file && std::fs::create_dir_all(&dir).is_ok() {
        let path = dir.join("opendevicecontrol.log");
        // Keep one previous log.
        let _ = std::fs::rename(&path, dir.join("opendevicecontrol.old.log"));
        if let Ok(f) = std::fs::File::create(&path) {
            loggers.push(WriteLogger::new(level, cfg, f));
        }
    }
    let _ = CombinedLogger::init(loggers);
}

fn probe() {
    probe_ram();
    probe_temperatures();
    match opendevicecontrol_core::discover(&[]) {
        Ok(devs) if devs.is_empty() => println!("No supported Corsair devices found."),
        Ok(devs) => {
            for (_, mut d) in devs {
                let i = d.info().clone();
                println!("{} {} ({:04x}:{:04x}) id={}", i.manufacturer, i.model, i.usb_vid, i.usb_pid, i.id);
                match d.poll() {
                    Ok(s) => {
                        for x in s.sensors {
                            println!("  {:<22} {:>10.3} {}{}", x.name, x.value, x.kind.unit(), if x.estimated { "  (estimated)" } else { "" });
                        }
                        for c in s.controls {
                            println!("  {:<22} {:?}", c.name, c.value);
                        }
                    }
                    Err(e) => println!("  error: {e}"),
                }
                if let Err(e) = d.release() {
                    println!("  release error: {e}");
                }
            }
        }
        Err(e) => {
            eprintln!("HID error: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |names: &[&str]| args.iter().any(|a| names.contains(&a.as_str()));
    if has(&["-h", "--help"]) {
        print!("{HELP}");
        return;
    }
    if has(&["-V", "--version"]) {
        println!("opendevicecontrol {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--export-icon") {
        // Used by packaging to produce desktop icons (PNG) or the .ico.
        let path = args.get(i + 1).map(String::as_str).unwrap_or("opendevicecontrol.png");
        // An option such as -v after the path is not a size.
        let size_arg = args.get(i + 2).map(String::as_str).filter(|s| !s.starts_with('-') || s.parse::<i64>().is_ok());
        let size = match icon::parse_export_size(size_arg) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        };
        let data = if path.to_lowercase().ends_with(".ico") {
            Ok(icon::ico())
        } else {
            icon::png(size).map_err(|e| std::io::Error::other(e.to_string()))
        };
        if let Err(e) = data.and_then(|d| std::fs::write(path, d)) {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
        return;
    }
    if has(&["--quit"]) {
        // Ask a running instance to shut down cleanly (used by installers).
        let outcome = ipc::request_quit(std::time::Duration::from_secs(10));
        match &outcome {
            ipc::QuitOutcome::Stopped => {}
            ipc::QuitOutcome::NotRunning => eprintln!("OpenDeviceControl is not running."),
            ipc::QuitOutcome::TimedOut => eprintln!("OpenDeviceControl did not exit in time."),
            ipc::QuitOutcome::Failed(e) => eprintln!("OpenDeviceControl exited, but not cleanly: {e}"),
        }
        std::process::exit(outcome.exit_code());
    }
    let headless = has(&["--headless"]);

    // Two instances would fight over the same USB devices. Checked before
    // anything touches the hardware or the log file, including --probe.
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    let instance = single_instance::SingleInstance::new(&format!("opendevicecontrol-{}", config::sanitize_id(&user)));
    let other_running = matches!(&instance, Ok(i) if !i.is_single());
    init_logging(has(&["-v", "--verbose"]), !other_running);
    if other_running {
        if has(&["--probe"]) {
            eprintln!("OpenDeviceControl is running and owns the devices; quit it first (opendevicecontrol --quit) to probe them.");
            std::process::exit(1);
        }
        if !headless && ipc::notify_existing() {
            log::info!("OpenDeviceControl is already running; showed its window");
            return;
        }
        log::error!("OpenDeviceControl is already running");
        eprintln!("OpenDeviceControl is already running (check the system tray).");
        std::process::exit(1);
    }
    if let Err(e) = &instance {
        log::warn!("single-instance check failed: {e}");
    }
    if has(&["--probe"]) {
        // Holds the instance lock while the devices are open.
        probe();
        drop(instance);
        return;
    }

    // Settings, energy counters and the login entry from the previous name.
    let migrated = config::migrate_legacy();
    config::merge_retired_backups();
    let mut cfg = config::Config::load();
    if autostart::migrate_legacy() {
        cfg.general.autostart = true;
    }
    if migrated || !config::Config::path().exists() {
        let _ = cfg.save();
    }
    log::info!("OpenDeviceControl {} starting (config: {})", env!("CARGO_PKG_VERSION"), config::Config::path().display());
    if cfg.general.autostart {
        autostart::sync(true);
    }
    let start_hidden = has(&["--minimized"]) || cfg.general.start_minimized;
    let handle = service::start(Arc::new(RwLock::new(cfg)));
    hass::start(handle.clone());
    {
        let h = handle.clone();
        ipc::listen(Arc::new(move || {
            log::info!("quit requested by another process");
            // Waits for (and reports) the shutdown even if it already began.
            std::process::exit(if h.shutdown().is_ok() { 0 } else { 1 });
        }));
    }

    if headless {
        let h = handle.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        ctrlc::set_handler(move || {
            let _ = tx.try_send(());
        })
        .expect("signal handler");
        let _ = rx.recv();
        let r = h.shutdown();
        drop(instance);
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }

    // Exit cleanly on SIGTERM / console close too.
    {
        let h = handle.clone();
        if let Err(e) = ctrlc::set_handler(move || {
            std::process::exit(if h.shutdown().is_ok() { 0 } else { 1 });
        }) {
            log::warn!("signal handler: {e}");
        }
    }

    let start_page = args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)).cloned();
    if let Err(e) = gui::run(handle.clone(), start_hidden, start_page) {
        log::error!("GUI failed: {e}. Falling back to headless mode.");
        // Without a display (e.g. a Linux server) keep serving MQTT.
        loop {
            std::thread::park();
        }
    }
    let r = handle.shutdown();
    drop(instance);
    if r.is_err() {
        std::process::exit(1);
    }
}

/// This PC's temperatures and, on Windows, where they come from.
fn probe_temperatures() {
    config::Config::load().sensors.apply();
    for s in opendevicecontrol_core::system::sensor_sources() {
        println!("Sensor source: {s}");
    }
    let components = sysinfo::Components::new_with_refreshed_list();
    let temps = opendevicecontrol_core::system::host_temperatures(&components);
    if temps.is_empty() {
        println!("This PC: no temperatures available");
    }
    for t in temps {
        println!("This PC {:<34} {:>6.1} °C  ({})", t.name, t.value, t.id);
    }
}

/// RAM over SMBus, only when enabled in the config (see docs/ram.md).
fn probe_ram() {
    if !config::Config::load().ram.enabled {
        return;
    }
    match opendevicecontrol_core::devices::ram::discover(&[]) {
        Ok(scan) => {
            println!("SMBus: {}", if scan.buses.is_empty() { "-".to_string() } else { scan.buses.join(", ") });
            if scan.devices.is_empty() {
                println!("  no Corsair RGB memory found");
            }
            for (_, mut d) in scan.devices {
                let i = d.info().clone();
                println!("{} {} id={} firmware={}", i.manufacturer, i.model, i.id, i.firmware.as_deref().unwrap_or("?"));
                for z in d.light_zones() {
                    println!("  {:<22} {} LEDs", z.name, z.leds);
                }
                match d.poll() {
                    Ok(s) => s.sensors.iter().for_each(|x| println!("  {:<22} {:>10.1} {}", x.name, x.value, x.kind.unit())),
                    Err(e) => println!("  error: {e}"),
                }
                if let Err(e) = d.release() {
                    println!("  release error: {e}");
                }
            }
        }
        Err(e) => println!("RAM: {e}"),
    }
}
