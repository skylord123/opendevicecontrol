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
                     control) and wait for it; exit code 1 if none was running
    --export-icon P [SIZE]
                     Write the application icon to P (PNG of SIZE px, default 256,
                     or a multi-size .ico if P ends in .ico) and exit
    -v, --verbose    Debug logging
    -h, --help       Show this help
    -V, --version    Show version
";

fn init_logging(verbose: bool) {
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
    if std::fs::create_dir_all(&dir).is_ok() {
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
        let size: u32 = args.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(256);
        let data = if path.to_lowercase().ends_with(".ico") { icon::ico() } else { icon::png(size) };
        if let Err(e) = std::fs::write(path, data) {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
        return;
    }
    if has(&["--quit"]) {
        // Ask a running instance to shut down cleanly (used by installers).
        let found = ipc::request_quit(std::time::Duration::from_secs(10));
        std::process::exit(if found { 0 } else { 1 });
    }
    init_logging(has(&["-v", "--verbose"]));
    if has(&["--probe"]) {
        return probe();
    }
    let headless = has(&["--headless"]);

    // Two instances would fight over the same USB devices.
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    let instance = single_instance::SingleInstance::new(&format!("opendevicecontrol-{}", config::sanitize_id(&user)));
    match &instance {
        Ok(i) if !i.is_single() => {
            if !headless && ipc::notify_existing() {
                log::info!("OpenDeviceControl is already running; showed its window");
                return;
            }
            log::error!("OpenDeviceControl is already running");
            eprintln!("OpenDeviceControl is already running (check the system tray).");
            std::process::exit(1);
        }
        Err(e) => log::warn!("single-instance check failed: {e}"),
        _ => {}
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
            h.shutdown();
            std::process::exit(0);
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
        h.shutdown();
        return;
    }

    // Exit cleanly on SIGTERM / console close too.
    {
        let h = handle.clone();
        let _ = ctrlc::set_handler(move || {
            h.shutdown();
            std::process::exit(0);
        });
    }

    let start_page = args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)).cloned();
    if let Err(e) = gui::run(handle.clone(), start_hidden, start_page) {
        log::error!("GUI failed: {e}. Falling back to headless mode.");
        // Without a display (e.g. a Linux server) keep serving MQTT.
        loop {
            std::thread::park();
        }
    }
    handle.shutdown();
    drop(instance);
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
            }
        }
        Err(e) => println!("RAM: {e}"),
    }
}
