//! The status item glyph.
//!
//! Prefers an SF Symbol (`display.and.screwdriver`) rasterised into a template image so macOS
//! tints it for light and dark menu bars. Falls back to a drawn monitor if the symbol is
//! unavailable.

use objc2::ClassType;
use objc2_app_kit::{
    NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace, NSGraphicsContext, NSImage,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// Symbol drawn at 2x for a 22 pt menu bar slot.
const PIXELS: isize = 44;
/// Glyph extent in pixels, leaving a hair of padding around the symbol.
const GLYPH: f64 = 40.0;

/// The icon to hand to the status item.
pub fn menu_bar_icon() -> tray_icon::Icon {
    symbol_icon("display.and.screwdriver").unwrap_or_else(drawn_monitor)
}

/// Rasterises an SF Symbol into straight RGBA.
fn symbol_icon(name: &str) -> Option<tray_icon::Icon> {
    unsafe {
        let symbol_name = NSString::from_str(name);
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&symbol_name, None)?;
        image.setTemplate(true);

        let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            PIXELS,
            PIXELS,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            PIXELS * 4,
            32,
        )?;

        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        let previous = NSGraphicsContext::currentContext();
        NSGraphicsContext::setCurrentContext(Some(&context));

        let natural = image.size();
        let extent = natural.width.max(natural.height).max(1.0);
        let scale = GLYPH / extent;
        let width = natural.width * scale;
        let height = natural.height * scale;
        let rect = NSRect::new(
            NSPoint::new((PIXELS as f64 - width) / 2.0, (PIXELS as f64 - height) / 2.0),
            NSSize::new(width, height),
        );
        image.drawInRect_fromRect_operation_fraction(
            rect,
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.0,
        );

        NSGraphicsContext::setCurrentContext(previous.as_deref());

        let pixels = rep.bitmapData();
        if pixels.is_null() {
            return None;
        }
        let mut rgba = std::slice::from_raw_parts(pixels, (PIXELS * PIXELS * 4) as usize).to_vec();

        // Template images are masked by alpha, but macOS also composites the colour. Draw the
        // glyph white so the tint is never muddied by premultiplied black.
        for pixel in rgba.chunks_exact_mut(4) {
            if pixel[3] > 0 {
                pixel[0] = 255;
                pixel[1] = 255;
                pixel[2] = 255;
            }
        }

        tray_icon::Icon::from_rgba(rgba, PIXELS as u32, PIXELS as u32).ok()
    }
}

/// Fallback monitor glyph, drawn in points inside a 22 pt square at 2x.
fn drawn_monitor() -> tray_icon::Icon {
    const SCALE: f32 = 2.0;
    const SIZE: usize = 22 * 2;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];

    let screen = Rect { x: 3.0, y: 4.0, w: 16.0, h: 11.5 };
    let radius = 2.0;
    let thickness = 1.25;
    let neck = Rect { x: 10.35, y: 15.5, w: 1.3, h: 2.3 };
    let foot = Rect { x: 7.5, y: 17.8, w: 7.0, h: 1.3 };

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
