//! Screenshots of the real UI without a window or GPU: egui's meshes are
//! drawn by a tiny software rasterizer into PNG files. For checking the
//! layout on any machine (CI, a locked screen, Linux in docker):
//!
//! `cargo test -p opendevicecontrol render_screenshots -- --ignored --nocapture`
//!
//! It starts the device service like the app does (devices are opened and
//! returned to hardware control at the end) with a throw-away config.

use super::*;
use eframe::egui::epaint::{ClippedPrimitive, ImageData, Primitive, TextureId};
use std::collections::HashMap;

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

struct Texture {
    size: [usize; 2],
    px: Vec<Color32>,
}

fn apply(textures: &mut HashMap<TextureId, Texture>, delta: &egui::TexturesDelta) {
    for (id, deltas) in &delta.set {
        for d in deltas {
            let ImageData::Color(img) = &d.image;
            match d.pos {
                None => {
                    textures.insert(*id, Texture { size: img.size, px: img.pixels.clone() });
                }
                Some([x0, y0]) => {
                    let Some(t) = textures.get_mut(id) else { continue };
                    for y in 0..img.size[1] {
                        for x in 0..img.size[0] {
                            t.px[(y0 + y) * t.size[0] + x0 + x] = img.pixels[y * img.size[0] + x];
                        }
                    }
                }
            }
        }
    }
}

fn sample(t: &Texture, u: f32, v: f32) -> [f32; 4] {
    let x = ((u * t.size[0] as f32 - 0.5).round().max(0.0) as usize).min(t.size[0] - 1);
    let y = ((v * t.size[1] as f32 - 0.5).round().max(0.0) as usize).min(t.size[1] - 1);
    let c = t.px[y * t.size[0] + x];
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
}

impl Canvas {
    fn new(w: usize, h: usize, bg: Color32) -> Self {
        let c = [bg.r() as f32 / 255.0, bg.g() as f32 / 255.0, bg.b() as f32 / 255.0, 1.0];
        Canvas { w, h, px: vec![c; w * h] }
    }

    fn draw(&mut self, prims: &[ClippedPrimitive], textures: &HashMap<TextureId, Texture>, ppp: f32) {
        for p in prims {
            let Primitive::Mesh(mesh) = &p.primitive else { continue };
            let Some(tex) = textures.get(&mesh.texture_id) else { continue };
            let clip = p.clip_rect;
            let (cx0, cy0) = ((clip.min.x * ppp).max(0.0) as usize, (clip.min.y * ppp).max(0.0) as usize);
            let (cx1, cy1) = (((clip.max.x * ppp).ceil() as usize).min(self.w), ((clip.max.y * ppp).ceil() as usize).min(self.h));
            for tri in mesh.indices.as_chunks::<3>().0 {
                let v = [&mesh.vertices[tri[0] as usize], &mesh.vertices[tri[1] as usize], &mesh.vertices[tri[2] as usize]];
                let p: Vec<(f32, f32)> = v.iter().map(|v| (v.pos.x * ppp, v.pos.y * ppp)).collect();
                let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
                if area.abs() < 1e-6 {
                    continue;
                }
                let minx = (p.iter().map(|q| q.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(cx0);
                let maxx = (p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil() as usize).min(cx1);
                let miny = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(cy0);
                let maxy = (p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as usize).min(cy1);
                for y in miny..maxy {
                    for x in minx..maxx {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
                        let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                        let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                        let t = sample(tex, u, vv);
                        let mut src = [0f32; 4];
                        for (k, s) in src.iter_mut().enumerate() {
                            let c = |i: usize| v[i].color.to_array()[k] as f32 / 255.0;
                            *s = (w0 * c(0) + w1 * c(1) + w2 * c(2)) * t[k];
                        }
                        let d = &mut self.px[y * self.w + x];
                        for k in 0..3 {
                            d[k] = src[k] + d[k] * (1.0 - src[3]);
                        }
                    }
                }
            }
        }
    }

    fn save(&self, path: &std::path::Path) {
        let f = std::fs::File::create(path).expect("create png");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().expect("png header");
        let data: Vec<u8> = self.px.iter().flat_map(|c| [c[0], c[1], c[2]].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
        wr.write_image_data(&data).expect("png data");
    }
}

struct Shooter {
    ctx: egui::Context,
    app: FerroApp,
    frame: eframe::Frame,
    textures: HashMap<TextureId, Texture>,
    size: egui::Vec2,
    ppp: f32,
    time: f64,
}

impl Shooter {
    fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        self.time += 0.1;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            max_texture_side: Some(8192),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.ctx.set_pixels_per_point(self.ppp);
        let app = &mut self.app;
        let frame = &mut self.frame;
        let mut out = self.ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, frame));
        apply(&mut self.textures, &out.textures_delta);
        out.textures_delta.clear();
        out
    }

    fn shot(&mut self, name: &str, dir: &std::path::Path) {
        for _ in 0..3 {
            self.frame(Vec::new());
        }
        let out = self.frame(Vec::new());
        let prims = self.ctx.tessellate(out.shapes, self.ppp);
        let bg = self.ctx.global_style().visuals.panel_fill;
        let mut c = Canvas::new((self.size.x * self.ppp) as usize, (self.size.y * self.ppp) as usize, bg);
        c.draw(&prims, &self.textures, self.ppp);
        let path = dir.join(format!("{name}.png"));
        c.save(&path);
        println!("wrote {}", path.display());
    }
}

#[test]
#[ignore = "opens devices; run by hand to look at the UI"]
fn render_screenshots() {
    let dir = std::env::temp_dir().join("opendevicecontrol-shots");
    let _ = std::fs::create_dir_all(dir.join("cfg"));
    #[allow(unused_unsafe)]
    // SAFETY: set before any other thread of this test reads them.
    unsafe {
        std::env::set_var("OPENDEVICECONTROL_CONFIG_DIR", dir.join("cfg"));
        std::env::set_var("OPENDEVICECONTROL_DATA_DIR", dir.join("cfg"));
    }
    let cfg = std::sync::Arc::new(parking_lot::RwLock::new(Config::default()));
    let h = crate::service::start(cfg);
    // Discovery, then a few polls for min/max and trends.
    std::thread::sleep(std::time::Duration::from_secs(9));
    let ctx = egui::Context::default();
    style(&ctx);
    let mut s = Shooter {
        app: FerroApp::new(h.clone()),
        ctx,
        frame: eframe::Frame::_new_kittest(),
        textures: HashMap::new(),
        size: egui::vec2(1180.0, 900.0),
        ppp: 1.0,
        time: 0.0,
    };
    s.app.page = Some(Page::Sensors);
    s.shot("1-sensors", &dir);
    s.app.page = Some(Page::Device(opendevicecontrol_core::system::ID.into()));
    s.shot("2-this-pc", &dir);
    s.app.page = Some(Page::Sensors);
    s.shot("3-graph", &dir);
    s.app.picker_open = true;
    s.shot("4-ha-picker", &dir);
    s.app.picker_open = false;
    // Overview: everything collapsed except devices and their hardware.
    s.app.tree.set_all(false);
    s.shot("5-overview", &dir);
    s.app.tree.set_all(true);
    s.app.tree.filter = "temp".into();
    s.shot("6-filter-temp", &dir);
    s.app.tree.filter.clear();
    // Ctrl+click two sensors (filtered to the CPU): both on the graph.
    s.app.tree.filter = "cpu".into();
    s.app.sensors_chart.clear();
    s.shot("7-filter-cpu", &dir);
    s.app.sensors_chart = vec![
        (opendevicecontrol_core::system::ID.into(), "cpu_temp".into()),
        (opendevicecontrol_core::system::ID.into(), "cpu_load".into()),
    ];
    s.shot("8-two-series", &dir);
    s.app.tree.filter.clear();
    s.app.page = Some(Page::Settings);
    s.shot("9-settings", &dir);
    s.app.page = Some(Page::Sensors);
    s.size = egui::vec2(700.0, 800.0);
    s.shot("10-narrow", &dir);
    // Every other device's page, tall enough to reach its sensor tree.
    s.size = egui::vec2(1180.0, 2400.0);
    let ids: Vec<String> = h.shared.read().devices.keys().filter(|k| *k != opendevicecontrol_core::system::ID).cloned().collect();
    for id in ids {
        s.app.page = Some(Page::Device(id.clone()));
        s.shot(&format!("11-device-{id}"), &dir);
    }
    let shared = h.shared.read().clone();
    let devices = crate::gui_sensors::device_order(shared.devices.values());
    std::fs::write(dir.join("report.txt"), crate::gui_sensors::report(&devices)).unwrap();
    h.shutdown();
}
