//! `cargo run -p opendevicecontrol-core --example probe` — read every supported device once.

fn main() {
    let devices = opendevicecontrol_core::discover(&[]).expect("hid init");
    if devices.is_empty() {
        println!("no supported Corsair devices found");
    }
    for (_, mut d) in devices {
        println!("{:#?}", d.info());
        for l in d.limits() {
            println!("  limit  {:<32} {:>8.2} {}", l.name, l.value, l.kind.unit());
        }
        for round in 0..3 {
            let t = std::time::Instant::now();
            match d.poll() {
                Ok(s) => {
                    println!("-- poll {round} ({} ms)", t.elapsed().as_millis());
                    for x in &s.sensors {
                        println!("  {:<16} {:<20} {:>10.3} {}{}", x.id, x.name, x.value, x.kind.unit(), if x.estimated { " (est.)" } else { "" });
                    }
                    for c in &s.controls {
                        println!("  control {} = {:?}", c.id, c.value);
                    }
                }
                Err(e) => println!("poll error: {e}"),
            }
        }
        for z in d.light_zones() {
            println!("  light zone {} ({}): {} LED(s)", z.id, z.name, z.leds);
        }
        for k in d.input_keys() {
            println!("  input key {} ({})", k.id, k.name);
        }
        d.release();
    }
}
