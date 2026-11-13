//! Procedurally drawn application icon (no binary assets needed):
//! a dark disc with a lightning bolt and a coloured ring.

/// Sizes `--export-icon` accepts, in pixels.
pub const EXPORT_SIZES: std::ops::RangeInclusive<u32> = 16..=1024;

/// Parse the optional SIZE argument of `--export-icon` (default 256).
pub fn parse_export_size(arg: Option<&str>) -> Result<u32, String> {
    let Some(arg) = arg else { return Ok(256) };
    let range = format!("{} to {}", EXPORT_SIZES.start(), EXPORT_SIZES.end());
    match arg.parse::<u32>() {
        Ok(n) if EXPORT_SIZES.contains(&n) => Ok(n),
        _ => Err(format!("invalid icon size '{arg}': expected a number of pixels from {range}")),
    }
}

pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    // Bolt polygon in unit coordinates.
    let bolt: [(f32, f32); 6] = [(0.58, 0.12), (0.28, 0.55), (0.48, 0.55), (0.40, 0.88), (0.72, 0.42), (0.52, 0.42)];
    let bolt: Vec<(f32, f32)> = bolt.iter().map(|(x, y)| (x * s, y * s)).collect();
    let c = s / 2.0;
    let r = s / 2.0 - 0.5;
    // Supersample 4x4 for anti-aliasing.
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0f32; 4];
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                    let d = ((px - c).powi(2) + (py - c).powi(2)).sqrt();
                    let col: Option<[f32; 3]> = if d > r {
                        None
                    } else if inside(&bolt, px, py) {
                        Some([1.0, 0.82, 0.18])
                    } else if d > r - s * 0.09 {
                        Some([0.13, 0.75, 0.70])
                    } else {
                        Some([0.10, 0.11, 0.13])
                    };
                    if let Some(col) = col {
                        acc[0] += col[0];
                        acc[1] += col[1];
                        acc[2] += col[2];
                        acc[3] += 1.0;
                    }
                }
            }
            let i = ((y * size + x) * 4) as usize;
            if acc[3] > 0.0 {
                out[i] = (acc[0] / acc[3] * 255.0) as u8;
                out[i + 1] = (acc[1] / acc[3] * 255.0) as u8;
                out[i + 2] = (acc[2] / acc[3] * 255.0) as u8;
                out[i + 3] = (acc[3] / 16.0 * 255.0) as u8;
            }
        }
    }
    out
}

/// The icon as a PNG file. `size` must be in [`EXPORT_SIZES`].
pub fn png(size: u32) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header()?;
        w.write_image_data(&rgba(size))?;
    }
    Ok(out)
}

/// The icon as a Windows .ico file (PNG-compressed entries, 16–256 px).
pub fn ico() -> Vec<u8> {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    // Fixed, valid sizes: encoding into memory cannot fail.
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| png(s).expect("icon png")).collect();
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in &images {
        out.extend_from_slice(img);
    }
    out
}

fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_size_is_validated() {
        assert_eq!(parse_export_size(None), Ok(256));
        assert_eq!(parse_export_size(Some("16")), Ok(16));
        assert_eq!(parse_export_size(Some("1024")), Ok(1024));
        for bad in ["0", "15", "1025", "4294967295", "-1", "abc", ""] {
            assert!(parse_export_size(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn png_at_the_smallest_size() {
        let data = png(*EXPORT_SIZES.start()).unwrap();
        assert_eq!(&data[1..4], b"PNG");
    }
}
