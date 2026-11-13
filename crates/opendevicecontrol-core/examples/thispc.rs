//! `cargo run -p opendevicecontrol-core --example thispc` — print This PC's sensor
//! tree (rates need a time window between polls, so it polls twice).

use opendevicecontrol_core::{Device, Hardware};

fn print(hw: &[Hardware], sensors: &[opendevicecontrol_core::Sensor], parent: Option<&str>, depth: usize) {
    for h in hw.iter().filter(|h| h.parent.as_deref() == parent) {
        println!("{:indent$}[{:?}] {}  ({})", "", h.kind, h.name, h.id, indent = depth * 2);
        for s in sensors.iter().filter(|s| s.hardware.as_deref() == Some(&h.id)) {
            println!(
                "{:indent$}{:<34} {:>10.2} {:<4} {:<40} {}{}",
                "",
                s.name,
                s.value,
                s.kind.unit(),
                s.id,
                if s.extra { "" } else { "HA " },
                s.source.as_deref().unwrap_or(""),
                indent = depth * 2 + 4
            );
        }
        print(hw, sensors, Some(&h.id), depth + 1);
    }
}

fn main() {
    // `--no-lhm`: as if LibreHardwareMonitor weren't running (its web
    // server is looked for on a port nothing listens on).
    if std::env::args().any(|a| a == "--no-lhm") {
        opendevicecontrol_core::system::configure_sensors(false, 1);
    }
    let mut pc = opendevicecontrol_core::system::ThisPc::open();
    let _ = pc.poll();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let snap = pc.poll().expect("poll");
    print(&pc.hardware(), &snap.sensors, None, 0);
    println!("{} sensors, {} sent to Home Assistant by default", snap.sensors.len(), snap.sensors.iter().filter(|s| !s.extra).count());
}
