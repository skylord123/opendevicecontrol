//! Detect other software that talks to the same Corsair devices.
//! Concurrent polling corrupts request/reply pairing on the USB HID channel.

use sysinfo::{ProcessesToUpdate, System};

const KNOWN: &[(&str, &str)] = &[
    ("icue.exe", "Corsair iCUE"),
    ("corsair.service.exe", "Corsair Service (iCUE)"),
    ("corsairlink4.service.exe", "Corsair LINK 4 service"),
    ("corsairlink4.exe", "Corsair LINK 4"),
    ("openlinkhub", "OpenLinkHub"),
    ("fancontrol.exe", "FanControl (if its Corsair plugin is enabled)"),
    ("signalrgb.exe", "SignalRGB"),
];

pub fn detect(sys: &mut System) -> Vec<String> {
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut out: Vec<String> = Vec::new();
    for p in sys.processes().values() {
        let name = p.name().to_string_lossy().to_lowercase();
        if let Some((_, label)) = KNOWN.iter().find(|(n, _)| *n == name) {
            if !out.iter().any(|o| o == label) {
                out.push(label.to_string());
            }
        }
    }
    out.sort();
    out
}
