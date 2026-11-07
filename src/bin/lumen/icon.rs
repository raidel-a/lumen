//! Draws the status item glyph. A template image, so macOS tints it for light and dark menus.

/// Backing scale of the bitmap (22 pt icon at 2x).
const SCALE: f32 = 2.0;
/// 22 pt square.
const SIZE: usize = 22 * 2;

/// A monitor: screen outline, neck and foot, drawn in points inside the 22 pt square.
pub fn monitor_icon() -> tray_icon::Icon {
    let mut rgba = vec![0u8; SIZE * SIZE * 4];

    // Point-space geometry. The screen sits high enough to leave room for the stand.
    let screen = Rect { x: 3.0, y: 4.0, w: 16.0, h: 11.5 };
    let radius = 2.0;
    let thickness = 1.25;
    let neck = Rect { x: 10.35, y: 15.5, w: 1.3, h: 2.3 };
    let foot = Rect { x: 7.5, y: 17.8, w: 7.0, h: 1.3 };

    // 3x3 coverage per pixel keeps the small glyph smooth without a real rasterizer.
    const SAMPLES: u32 = 3;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut covered = 0u32;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / SCALE;
                    let py = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / SCALE;
                    if screen_outline(px, py, screen, radius, thickness)
                        || solid(px, py, neck, 0.3)
                        || solid(px, py, foot, 0.6)
                    {
                        covered += 1;
                    }
                }
            }
            if covered > 0 {
                let index = (y * SIZE + x) * 4;
                rgba[index + 3] = (covered * 255 / (SAMPLES * SAMPLES)) as u8;
            }
        }
    }

    tray_icon::Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).expect("valid icon buffer")
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn right(&self) -> f32 {
        self.x + self.w
    }
    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// True inside a rounded-rect outline whose stroke is measured inward from `rect`.
fn screen_outline(x: f32, y: f32, rect: Rect, radius: f32, thickness: f32) -> bool {
    if !inside_rounded(x, y, rect, radius) {
        return false;
    }
    let inner = Rect {
        x: rect.x + thickness,
        y: rect.y + thickness,
        w: rect.w - thickness * 2.0,
        h: rect.h - thickness * 2.0,
    };
    !inside_rounded(x, y, inner, (radius - thickness).max(0.0))
}

/// True inside a solid rounded rect; `radius` of 0 is a plain rect.
fn solid(x: f32, y: f32, rect: Rect, radius: f32) -> bool {
    inside_rounded(x, y, rect, radius)
}

fn inside_rounded(x: f32, y: f32, rect: Rect, radius: f32) -> bool {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return false;
    }
    if x < rect.x || x > rect.right() || y < rect.y || y > rect.bottom() {
        return false;
    }
    let radius = radius.min(rect.w / 2.0).min(rect.h / 2.0);
    // A radius equal to half the side leaves no span to clamp into, and float error can make
    // the bounds cross; keep the centre of the flat run in that case.
    let cx = clamp_span(x, rect.x + radius, rect.right() - radius, rect.x + rect.w / 2.0);
    let cy = clamp_span(y, rect.y + radius, rect.bottom() - radius, rect.y + rect.h / 2.0);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

fn clamp_span(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if min >= max {
        fallback
    } else {
        value.clamp(min, max)
    }
}
