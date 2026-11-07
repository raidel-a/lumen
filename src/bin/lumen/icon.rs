//! Draws the status item glyph. A template image, so macOS tints it for light and dark menus.

/// A simple monitor glyph drawn at 2x (44×44 px, shown at 22 pt).
pub fn monitor_icon() -> tray_icon::Icon {
    const SIZE: usize = 44;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];

    // Rounded 32×22 pt screen plus a stand, in device pixels.
    let screen = (left() * 2.0, 9.0 * 2.0, 32.0 * 2.0, 22.0 * 2.0);
    let thickness = 2.0 * 2.0;
    let radius = 4.0 * 2.0;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let on_screen_edge = rounded_ring(fx, fy, screen, radius, thickness);
            let on_stand = (fx - 22.0 * 2.0).abs() <= thickness / 2.0
                && fy >= (9.0 + 22.0) * 2.0
                && fy <= (9.0 + 26.0) * 2.0;
            let on_base = fy >= (9.0 + 26.0) * 2.0
                && fy <= (9.0 + 28.0) * 2.0
                && (fx - 22.0 * 2.0).abs() <= 7.0 * 2.0;
            if on_screen_edge || on_stand || on_base {
                let index = (y * SIZE + x) * 4;
                rgba[index] = 0;
                rgba[index + 1] = 0;
                rgba[index + 2] = 0;
                rgba[index + 3] = 255;
            }
        }
    }

    tray_icon::Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).expect("valid icon buffer")
}

fn left() -> f32 {
    6.0
}

/// True inside a rounded-rect outline of `thickness`, measured inward from `rect`.
fn rounded_ring(x: f32, y: f32, rect: (f32, f32, f32, f32), radius: f32, thickness: f32) -> bool {
    let (left, top, width, height) = rect;
    let right = left + width;
    let bottom = top + height;
    if x < left - 0.5 || x > right + 0.5 || y < top - 0.5 || y > bottom + 0.5 {
        return false;
    }

    let inner_left = left + thickness;
    let inner_top = top + thickness;
    let inner_right = right - thickness;
    let inner_bottom = bottom - thickness;
    let inner_radius = (radius - thickness).max(0.0);

    inside_rounded(x, y, left, top, right, bottom, radius)
        && !inside_rounded(
            x,
            y,
            inner_left,
            inner_top,
            inner_right,
            inner_bottom,
            inner_radius,
        )
}

fn inside_rounded(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }
    let cx = x.clamp(left + radius, right - radius);
    let cy = y.clamp(top + radius, bottom - radius);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}
