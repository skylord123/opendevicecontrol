// Embed the application icon and version info into the Windows executable.
// The icon is rendered from the same code the app uses at runtime.

#[path = "src/icon.rs"]
#[allow(dead_code)]
mod icon;

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ico = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("opendevicecontrol.ico");
    std::fs::write(&ico, icon::ico()).unwrap();
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().unwrap());
    res.set("ProductName", "opendevicecontrol");
    res.set("FileDescription", "opendevicecontrol - Corsair device monitor and Home Assistant bridge");
    res.set("LegalCopyright", "GPL-3.0-or-later");
    if let Err(e) = res.compile() {
        // Missing resource compiler must not break the build.
        println!("cargo:warning=could not embed Windows icon: {e}");
    }
}
