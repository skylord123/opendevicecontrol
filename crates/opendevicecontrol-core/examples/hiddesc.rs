//! `cargo run -p opendevicecontrol-core --example hiddesc <vid> [pid]` — print the HID
//! report descriptor of every interface of a device (hex), for working out
//! report ids and sizes when adding a new driver.

fn main() {
    let mut args = std::env::args().skip(1);
    let hex = |s: String| u16::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex id");
    let vid = hex(args.next().expect("usage: hiddesc <vid> [pid]"));
    let pid = args.next().map(hex);
    let api = hidapi::HidApi::new().expect("hid init");
    for d in api
        .device_list()
        .filter(|d| d.vendor_id() == vid && pid.is_none_or(|p| d.product_id() == p))
    {
        println!(
            "{:04x}:{:04x} if={} usage_page={:#06x} usage={:#06x}",
            d.vendor_id(),
            d.product_id(),
            d.interface_number(),
            d.usage_page(),
            d.usage()
        );
        match d.open_device(&api) {
            Ok(dev) => {
                let mut buf = [0u8; 4096];
                match dev.get_report_descriptor(&mut buf) {
                    Ok(n) => {
                        for chunk in buf[..n].chunks(16) {
                            let line: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                            println!("    {}", line.join(" "));
                        }
                    }
                    Err(e) => println!("    descriptor error: {e}"),
                }
            }
            Err(e) => println!("    open error: {e}"),
        }
    }
}
