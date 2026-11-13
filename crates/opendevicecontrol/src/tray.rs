//! System tray icon. Runs on its own thread with its own platform event loop
//! (Win32 message pump on Windows, GTK main loop on Linux), so it keeps
//! working while the main window is hidden.

use crate::service::Handle;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub type ShowFn = Arc<dyn Fn() + Send + Sync>;

struct Tray {
    tray: TrayIcon,
    status: MenuItem,
    show_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
    last: Instant,
}

fn build() -> Result<Tray, Box<dyn std::error::Error>> {
    let menu = Menu::new();
    let status = MenuItem::new("OpenDeviceControl — starting…", false, None);
    let show = MenuItem::new("Show OpenDeviceControl", true, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&show)?;
    menu.append(&quit)?;
    let icon = tray_icon::Icon::from_rgba(crate::icon::rgba(32), 32, 32)?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("OpenDeviceControl")
        .with_icon(icon)
        .with_menu_on_left_click(false)
        .build()?;
    Ok(Tray { tray, status, show_id: show.id().clone(), quit_id: quit.id().clone(), last: Instant::now() - Duration::from_secs(60) })
}

fn summary(h: &Handle) -> String {
    let s = h.shared.read();
    let parts: Vec<String> = s
        .devices
        .values()
        .filter(|d| d.online)
        .map(|d| {
            let pin = d.value("input_power").unwrap_or(0.0);
            let t = d.value("temp_vrm").unwrap_or(0.0);
            format!("{}: {:.0} W · {:.0} °C", d.info.model, pin, t)
        })
        .collect();
    if parts.is_empty() { "No devices".into() } else { parts.join("\n") }
}

/// Process pending tray events. Returns false when the app should exit.
fn tick(t: &mut Tray, h: &Handle, show: &ShowFn) {
    while let Ok(ev) = MenuEvent::receiver().try_recv() {
        if ev.id == t.show_id {
            show();
        } else if ev.id == t.quit_id {
            h.shutdown();
            std::process::exit(0);
        }
    }
    while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
        match ev {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
            | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => show(),
            _ => {}
        }
    }
    if t.last.elapsed() > Duration::from_secs(2) {
        t.last = Instant::now();
        let s = summary(h);
        t.status.set_text(s.lines().next().unwrap_or("OpenDeviceControl"));
        let _ = t.tray.set_tooltip(Some(format!("OpenDeviceControl\n{s}")));
    }
}

#[cfg(windows)]
pub fn spawn(h: Handle, show: ShowFn) {
    std::thread::Builder::new()
        .name("opendevicecontrol-tray".into())
        .spawn(move || {
            use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
            let mut t = match build() {
                Ok(t) => t,
                Err(e) => return log::error!("tray icon unavailable: {e}"),
            };
            loop {
                unsafe {
                    let mut msg: MSG = std::mem::zeroed();
                    while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                tick(&mut t, &h, &show);
                std::thread::sleep(Duration::from_millis(30));
            }
        })
        .expect("spawn tray");
}

#[cfg(target_os = "linux")]
pub fn spawn(h: Handle, show: ShowFn) {
    std::thread::Builder::new()
        .name("opendevicecontrol-tray".into())
        .spawn(move || {
            if let Err(e) = gtk::init() {
                return log::error!("tray icon unavailable (GTK init failed): {e}");
            }
            let t = match build() {
                Ok(t) => t,
                Err(e) => return log::error!("tray icon unavailable: {e}"),
            };
            let t = std::cell::RefCell::new(t);
            gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
                tick(&mut t.borrow_mut(), &h, &show);
                gtk::glib::ControlFlow::Continue
            });
            gtk::main();
        })
        .expect("spawn tray");
}
