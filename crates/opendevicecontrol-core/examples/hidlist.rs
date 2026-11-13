//! `cargo run -p opendevicecontrol-core --example hidlist [vid]` — list every HID
//! interface of a vendor (Corsair by default; pass a hex vendor id such as
//! `3554` for others). Useful when adding support for a new device.

fn main() {
    let vid = std::env::args()
        .nth(1)
        .map(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex vendor id"))
        .unwrap_or(opendevicecontrol_core::CORSAIR_VID);
    let api = hidapi::HidApi::new().expect("hid init");
    for d in api.device_list().filter(|d| d.vendor_id() == vid) {
        println!(
            "{:04x}:{:04x} if={:<2} usage_page={:#06x} usage={:#06x} product={:?} manufacturer={:?} serial={:?} release={:#06x}\n    path={}",
            d.vendor_id(),
            d.product_id(),
            d.interface_number(),
            d.usage_page(),
            d.usage(),
            d.product_string().unwrap_or(""),
            d.manufacturer_string().unwrap_or(""),
            d.serial_number().unwrap_or(""),
            d.release_number(),
            d.path().to_string_lossy()
        );
    }
}
