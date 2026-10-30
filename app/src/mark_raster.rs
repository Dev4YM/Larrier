// The mark's geometry, drawn pixel by pixel. Shared by the app (window and
// notification icons) and the build script (the .exe's icon), so it has no
// dependencies: it is `include!`d in both.

type Rgb = [f32; 3];

fn hex(c: u32) -> Rgb {
    [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Whether (x, y) lies inside the 64 × 64 tile with corner radius 15, and
/// how far it is from the tile's edge.
fn tile(x: f32, y: f32) -> Option<f32> {
    const R: f32 = 15.0;
    let cx = x.clamp(R, 64.0 - R);
    let cy = y.clamp(R, 64.0 - R);
    let edge = if x != cx || y != cy { R - ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() } else { x.min(y).min(64.0 - x).min(64.0 - y) };
    (edge >= 0.0).then_some(edge)
}

/// Even-odd point-in-polygon test.
fn in_polygon(x: f32, y: f32, pts: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let ((xi, yi), (xj, yj)) = (pts[i], pts[j]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Whether (x, y) is cut off by a rounded corner: beyond the corner's
/// circle (centre `c`, radius 3) on the side given by `sx`, `sy`.
fn rounded_off(x: f32, y: f32, c: (f32, f32), sx: f32, sy: f32) -> bool {
    (x - c.0) * sx > 0.0 && (y - c.1) * sy > 0.0 && (x - c.0).powi(2) + (y - c.1).powi(2) > 9.0
}

/// The two pieces of the stencil L (see the SVG paths).
fn letter(x: f32, y: f32) -> bool {
    let stem = in_polygon(x, y, &[(18.0, 13.0), (32.0, 13.0), (32.0, 35.2), (22.6, 44.6), (18.0, 40.0)])
        && !rounded_off(x, y, (21.0, 16.0), -1.0, -1.0)
        && !rounded_off(x, y, (29.0, 16.0), 1.0, -1.0);
    let foot = in_polygon(x, y, &[(26.8, 48.8), (36.2, 39.4), (50.0, 39.4), (50.0, 51.0), (25.0, 51.0)])
        && !rounded_off(x, y, (47.0, 42.4), 1.0, -1.0)
        && !rounded_off(x, y, (47.0, 48.0), 1.0, 1.0);
    stem || foot
}

/// The colour of one point of the ink mark, or None outside the tile.
fn sample(x: f32, y: f32) -> Option<Rgb> {
    let edge = tile(x, y)?;
    if letter(x, y) {
        return Some(hex(0xF4F3EF));
    }
    let mut c = mix(hex(0x232327), hex(0x0D0D0F), (y / 64.0).clamp(0.0, 1.0));
    if edge < 1.0 {
        c = mix(c, [1.0; 3], 0.09);
    }
    Some(c)
}

/// The ink mark as straight-alpha RGBA pixels, `n` × `n`, antialiased with
/// 4 × 4 samples per pixel.
pub fn mark_rgba(n: u32) -> Vec<u8> {
    const S: u32 = 4;
    let k = 64.0 / n as f32;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for py in 0..n {
        for pxi in 0..n {
            let mut acc = [0f32; 3];
            let mut cover = 0f32;
            for sy in 0..S {
                for sx in 0..S {
                    let x = (pxi as f32 + (sx as f32 + 0.5) / S as f32) * k;
                    let y = (py as f32 + (sy as f32 + 0.5) / S as f32) * k;
                    if let Some(c) = sample(x, y) {
                        acc[0] += c[0];
                        acc[1] += c[1];
                        acc[2] += c[2];
                        cover += 1.0;
                    }
                }
            }
            if cover == 0.0 {
                continue;
            }
            let i = ((py * n + pxi) * 4) as usize;
            for ch in 0..3 {
                px[i + ch] = (acc[ch] / cover * 255.0).round() as u8;
            }
            px[i + 3] = (cover / (S * S) as f32 * 255.0).round() as u8;
        }
    }
    px
}
