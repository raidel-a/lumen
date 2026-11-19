//! The on-screen brightness indicator.
//!
//! A single reusable borderless floating panel modelled on FineTune's volume HUD
//! (`FineTune/Views/HUD`). Two styles, selectable in Settings:
//!
//! - **Tahoe**: a 300x72 rounded pill in the top-right corner, with a "Brightness" title row and a
//!   row of `[sun glyph] [capsule progress bar] [percentage]`.
//! - **Classic**: a 200x200 pre-Tahoe panel centred near the bottom, with an 80 pt sun glyph above
//!   a row of sixteen square tiles that fill left to right. No percentage.
//!
//! Both use the system `.regularMaterial`-equivalent (`NSVisualEffectMaterial::HudWindow` here) with
//! a one-point `hudBorder` stroke (appearance-aware: white @ 0.08 dark, black @ 0.15 light), a
//! non-activating floating panel, no shadow, and a short fade. The panel is rebuilt when the style
//! changes because the two layouts differ in size and subviews.
//!
//! The pure helpers (`hud_spec`, `filled_tiles`) carry the value math and are unit tested; the
//! AppKit glue sits on top.

use crate::config::HudStyle;
use objc2::rc::Retained;
use objc2::{msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameDarkAqua, NSBackingStoreType, NSBox, NSBoxType, NSColor, NSFont,
    NSFontWeightMedium, NSFontWeightSemibold, NSFloatingWindowLevel, NSImage, NSImageScaling,
    NSImageSymbolConfiguration, NSImageView, NSScreen, NSTextAlignment, NSTextField,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSPoint, NSRect, NSSize, NSString, NSTimer};
use std::cell::RefCell;

// Shared timing, matching FineTune (fade-in 0.12s, fade-out 0.11s, hide after 1100 ms).
const FADE_IN: f64 = 0.12;
const FADE_OUT: f64 = 0.11;
const HOLD_SECONDS: f64 = 1.1;
const SCREEN_INSET: f64 = 8.0;

// Tahoe pill.
const TAHOE_WIDTH: f64 = 300.0;
const TAHOE_HEIGHT: f64 = 72.0;
const TAHOE_CORNER: f64 = 22.0;
const TAHOE_PAD_X: f64 = 14.0;
const TAHOE_PERCENT_WIDTH: f64 = 40.0;
const TAHOE_GLYPH_BOX: f64 = 18.0;
const TAHOE_BAR_HEIGHT: f64 = 4.0;

// Classic square.
const CLASSIC_SIZE: f64 = 200.0;
const CLASSIC_CORNER: f64 = 16.0;
const CLASSIC_BOTTOM_INSET: f64 = 140.0;
/// Glyph box side. Larger than FineTune's nominal 80 pt because the `sun.max.fill` symbol fills a
/// smaller fraction of its bounding box than the speaker glyph; a 94 pt box renders the sun at the
/// ~49 pt height the reference speaker occupies.
const CLASSIC_ICON: f64 = 94.0;
/// Point size for the Classic glyph, kept equal to the box so proportional scaling fills it.
const CLASSIC_ICON_POINT: f64 = 94.0;
const CLASSIC_TILES: usize = 16;
const CLASSIC_TILE: f64 = 7.5;
const CLASSIC_TILE_SPACING: f64 = 2.0;
/// Vertical positions of the glyph and tile row, in AppKit's bottom-left coordinate space.
///
/// FineTune's `ClassicStyleHUD` nests fixed-height sections (icon 100 pt, tiles 80 pt) whose
/// content overflows and re-centres in SwiftUI in a way that is not obvious from the source, so the
/// literal "10 pt margin + 56 pt spacer" arithmetic underestimates the spread. These two values are
/// measured from FineTune's rendered Classic HUD (a 200 pt panel; the reference capture put the
/// glyph top ~59 pt and the tile-row centre ~166 pt below the top edge) and reproduce it 1:1:
/// the glyph sits higher and the tiles sit lower, with a wider gap between them.
///
/// Glyph box bottom in AppKit's bottom-left space. The reference puts the glyph's optical centre
/// ~87 pt below the top edge, i.e. AppKit Y centre 113; with a 94 pt box that is a bottom of
/// 113 - 47 = 66.
const CLASSIC_GLYPH_Y: f64 = 66.0;
/// Tile bottom: row centre ~166 pt from the top edge -> 200 - 166 - 7.5/2 ≈ 27.
const CLASSIC_TILE_Y: f64 = 27.0;
/// FineTune's `hudTileActive` / `hudTileInactive` tokens: white at these alphas in the dark HUD.
const TILE_ACTIVE_ALPHA: f64 = 0.7;
const TILE_INACTIVE_ALPHA: f64 = 0.2;

/// What the HUD should draw for a given brightness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudSpec {
    /// 0.0..=1.0 fill fraction of the bar.
    pub fraction: f64,
    /// The whole-percent reading shown as text.
    pub percent: u16,
    /// False when the display cannot be controlled: dim the bar and mark the glyph.
    pub controllable: bool,
}

/// Maps a `(value, max)` pair and controllability to the bar fill, percentage and glyph state.
pub fn hud_spec(value: u16, max: u16, controllable: bool) -> HudSpec {
    let max = max.max(1);
    let value = value.min(max);
    let fraction = f64::from(value) / f64::from(max);
    HudSpec {
        fraction,
        percent: (fraction * 100.0).round() as u16,
        controllable,
    }
}

/// How many of the Classic HUD's tiles are filled for a fraction, rounded like FineTune.
pub fn filled_tiles(fraction: f64, tiles: usize) -> usize {
    let fraction = fraction.clamp(0.0, 1.0);
    ((fraction * tiles as f64).round() as usize).min(tiles)
}

thread_local! {
    static HUD: RefCell<Option<Hud>> = const { RefCell::new(None) };
}

/// The live panel and whichever style's widgets it currently holds.
struct Hud {
    style: HudStyle,
    window: Retained<NSWindow>,
    glyph: Retained<NSImageView>,
    dismiss: Option<Retained<NSTimer>>,
    /// Tahoe only.
    fill: Option<Retained<NSBox>>,
    label: Option<Retained<NSTextField>>,
    /// Classic only.
    tiles: Vec<Retained<NSBox>>,
}

fn mtm() -> MainThreadMarker {
    unsafe { MainThreadMarker::new_unchecked() }
}

/// An appearance-aware fill for the HUD's bar, tiles and glyph: white at `alpha` on a dark panel,
/// black at `alpha` on a light one. The HUD material (`HUDWindow`) tracks the system appearance,
/// so a fixed white fill is nearly invisible in light mode; this resolves per effective appearance
/// like `hud_border`, keeping the content legible in both.
fn hud_fill(alpha: f64) -> Retained<NSColor> {
    use block2::RcBlock;
    unsafe {
        let provider = RcBlock::new(
            move |appearance: std::ptr::NonNull<NSAppearance>| -> std::ptr::NonNull<NSColor> {
                let appearance = appearance.as_ref();
                let names = NSArray::arrayWithObject(NSAppearanceNameDarkAqua);
                let is_dark = appearance
                    .bestMatchFromAppearancesWithNames(&names)
                    .map(|name| &*name == NSAppearanceNameDarkAqua)
                    .unwrap_or(false);
                let white = if is_dark { 1.0 } else { 0.0 };
                let color = NSColor::colorWithWhite_alpha(white, alpha);
                std::ptr::NonNull::new(Retained::into_raw(color)).unwrap()
            },
        );
        NSColor::colorWithName_dynamicProvider(None, &provider)
    }
}

/// FineTune's `hudBorder` design token: an appearance-aware stroke that resolves to white @ 0.08
/// in dark mode and black @ 0.15 in light mode. Returned as a dynamic `NSColor` so the border
/// tracks the panel's effective appearance.
fn hud_border() -> Retained<NSColor> {
    use block2::RcBlock;
    unsafe {
        let provider = RcBlock::new(
            move |appearance: std::ptr::NonNull<NSAppearance>| -> std::ptr::NonNull<NSColor> {
                let appearance = appearance.as_ref();
                let names = NSArray::arrayWithObject(NSAppearanceNameDarkAqua);
                let is_dark = appearance
                    .bestMatchFromAppearancesWithNames(&names)
                    .map(|name| &*name == NSAppearanceNameDarkAqua)
                    .unwrap_or(false);
                let color = if is_dark {
                    NSColor::colorWithWhite_alpha(1.0, 0.08)
                } else {
                    NSColor::colorWithWhite_alpha(0.0, 0.15)
                };
                // The dynamic provider hands ownership of a +1 color to AppKit, which caches and
                // manages it, so transfer the retain rather than leaking a new color each call.
                std::ptr::NonNull::new(Retained::into_raw(color)).unwrap()
            },
        );
        NSColor::colorWithName_dynamicProvider(None, &provider)
    }
}

/// A borderless custom `NSBox` filled with a colour, used for the bar and the tiles.
fn solid_box(frame: NSRect, color: &NSColor, corner: f64) -> Retained<NSBox> {
    unsafe {
        let box_ = NSBox::initWithFrame(mtm().alloc(), frame);
        box_.setBoxType(NSBoxType::NSBoxCustom);
        box_.setBorderWidth(0.0);
        box_.setCornerRadius(corner);
        box_.setFillColor(color);
        box_.setContentViewMargins(NSSize::new(0.0, 0.0));
        box_.setTitlePosition(objc2_app_kit::NSTitlePosition::NSNoTitle);
        box_
    }
}

/// Shows the brightness HUD for a value on the display under the pointer and arms the fade-out.
pub fn show(value: u16, max: u16, controllable: bool) {
    let style = crate::with_model(|model| model.config.hud_style);
    let spec = hud_spec(value, max, controllable);
    HUD.with(|slot| {
        let mut slot = slot.borrow_mut();
        // Rebuild when absent or when the chosen style changed: the layouts differ in size.
        let rebuild = slot.as_ref().map(|hud| hud.style != style).unwrap_or(true);
        if rebuild {
            if let Some(old) = slot.as_ref() {
                old.window.close();
            }
            *slot = Some(build(style));
        }
        if let Some(hud) = slot.as_mut() {
            hud.present(spec);
        }
    });
}

/// Fades the HUD out. Called from the dismiss timer on the `Actions` class.
pub fn dismiss() {
    HUD.with(|slot| {
        if let Some(hud) = slot.borrow_mut().as_mut() {
            hud.fade_out();
        }
    });
}

impl Hud {
    fn present(&mut self, spec: HudSpec) {
        self.update_glyph(spec.controllable);
        match self.style {
            HudStyle::Tahoe => self.update_tahoe(spec),
            HudStyle::Classic => self.update_classic(spec),
        }
        unsafe {
            place(&self.window, self.style);
            // Only fade in from hidden. While the HUD is already visible (e.g. a held key fires
            // repeated shows), keep it opaque and just refresh content, or it flickers.
            if self.window.isVisible() && self.window.alphaValue() > 0.0 {
                self.window.orderFrontRegardless();
            } else {
                self.window.setAlphaValue(0.0);
                self.window.orderFrontRegardless();
                animate_alpha(&self.window, 1.0, FADE_IN);
            }
        }
        self.arm_dismiss();
    }

    fn update_glyph(&self, controllable: bool) {
        let symbol = if controllable {
            "sun.max.fill"
        } else {
            "sun.max.trianglebadge.exclamationmark"
        };
        unsafe {
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                None,
            ) {
                self.glyph.setImage(Some(&image));
            }
            // FineTune tints the glyph with `hudTileActive` (.primary @ 0.7), not full white.
            let tint = if controllable {
                hud_fill(TILE_ACTIVE_ALPHA)
            } else {
                NSColor::secondaryLabelColor()
            };
            self.glyph.setContentTintColor(Some(&tint));
        }
    }

    fn update_tahoe(&self, spec: HudSpec) {
        let Some(fill) = self.fill.as_ref() else {
            return;
        };
        unsafe {
            // The track spans the middle column; the fill is a child clamped to the fraction.
            let track_width = fill
                .superview()
                .map(|track| track.frame().size.width)
                .unwrap_or(0.0);
            let fill_width = (track_width * spec.fraction.clamp(0.0, 1.0)).round();
            fill.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(fill_width, TAHOE_BAR_HEIGHT),
            ));
            fill.setFillColor(&hud_fill(if spec.controllable { 1.0 } else { 0.35 }));
            fill.setHidden(fill_width <= 0.0);

            if let Some(label) = self.label.as_ref() {
                let reading = if spec.controllable {
                    format!("{}%", spec.percent)
                } else {
                    "—".to_string()
                };
                label.setStringValue(&NSString::from_str(&reading));
            }
        }
    }

    fn update_classic(&self, spec: HudSpec) {
        let filled = if spec.controllable {
            filled_tiles(spec.fraction, CLASSIC_TILES)
        } else {
            0
        };
        for (index, tile) in self.tiles.iter().enumerate() {
            let color = if index < filled {
                hud_fill(TILE_ACTIVE_ALPHA)
            } else {
                hud_fill(TILE_INACTIVE_ALPHA)
            };
            unsafe { tile.setFillColor(&color) };
        }
    }

    fn arm_dismiss(&mut self) {
        if let Some(timer) = self.dismiss.take() {
            unsafe { timer.invalidate() };
        }
        let Some(actions) = crate::ui::target() else {
            return;
        };
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                HOLD_SECONDS,
                &*actions,
                sel!(hudDismiss:),
                None,
                false,
            )
        };
        self.dismiss = Some(timer);
    }

    fn fade_out(&mut self) {
        if let Some(timer) = self.dismiss.take() {
            unsafe { timer.invalidate() };
        }
        unsafe { animate_alpha(&self.window, 0.0, FADE_OUT) };
    }
}

/// Animates a window's alpha through its animator proxy, falling back to a direct set.
unsafe fn animate_alpha(window: &NSWindow, target: f64, duration: f64) {
    let animator: *mut objc2::runtime::AnyObject = msg_send![window, animator];
    let context_class = objc2::runtime::AnyClass::get("NSAnimationContext");
    if let (Some(context_class), false) = (context_class, animator.is_null()) {
        let _: () = msg_send![context_class, beginGrouping];
        let current: *mut objc2::runtime::AnyObject = msg_send![context_class, currentContext];
        if !current.is_null() {
            let _: () = msg_send![current, setDuration: duration];
        }
        let _: () = msg_send![animator, setAlphaValue: target];
        let _: () = msg_send![context_class, endGrouping];
    } else {
        unsafe { window.setAlphaValue(target) };
    }
}

/// An opaque-black rounded-rect image used as an `NSVisualEffectView` mask. Only the alpha matters:
/// the material shows where the image is opaque. Cap insets keep the corners crisp if resized.
fn rounded_mask(size: NSSize, corner: f64) -> Retained<NSImage> {
    use block2::RcBlock;
    unsafe {
        let handler = RcBlock::new(move |rect: NSRect| -> objc2::runtime::Bool {
            let path = objc2_app_kit::NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                rect, corner, corner,
            );
            NSColor::blackColor().set();
            path.fill();
            objc2::runtime::Bool::YES
        });
        let image = NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler);
        image.setCapInsets(objc2_foundation::NSEdgeInsets {
            top: corner,
            left: corner,
            bottom: corner,
            right: corner,
        });
        image.setResizingMode(objc2_app_kit::NSImageResizingMode::Tile);
        image
    }
}

/// A borderless, non-activating floating panel with a frosted rounded content view and a
/// one-point separator border, matching FineTune's panel.
fn make_window(size: NSSize, corner: f64) -> (Retained<NSWindow>, Retained<NSVisualEffectView>) {
    unsafe {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm().alloc(),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setOpaque(false);
        window.setHasShadow(false);
        window.setLevel(NSFloatingWindowLevel);
        window.setIgnoresMouseEvents(true);
        window.setHidesOnDeactivate(false);
        window.setAlphaValue(0.0);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );

        let effect = NSVisualEffectView::initWithFrame(mtm().alloc(), frame);
        effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.setWantsLayer(true);
        // A rounded-rect mask image is how AppKit rounds a `BehindWindow` effect view: the layer's
        // `cornerRadius` alone does not clip the vibrancy, so the corners read as square.
        effect.setMaskImage(Some(&rounded_mask(size, corner)));
        window.setContentView(Some(&effect));

        // A one-point hudBorder stroke around the rounded rect, drawn as a borderless box since
        // `CALayer::setBorderColor:` is not in these bindings. FineTune uses the same hudBorder
        // token for both the Tahoe and Classic panels, so this is shared.
        let border = NSBox::initWithFrame(mtm().alloc(), frame);
        border.setBoxType(NSBoxType::NSBoxCustom);
        border.setFillColor(&NSColor::clearColor());
        border.setBorderColor(&hud_border());
        border.setBorderWidth(1.0);
        border.setCornerRadius(corner);
        border.setContentViewMargins(NSSize::new(0.0, 0.0));
        border.setTitlePosition(objc2_app_kit::NSTitlePosition::NSNoTitle);
        border.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::NSViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::NSViewHeightSizable,
        );
        effect.addSubview(&border);
        (window, effect)
    }
}

/// A sun glyph image view at a point size, centred in a box of the given size.
fn make_glyph(point_size: f64, box_frame: NSRect) -> Retained<NSImageView> {
    unsafe {
        let glyph = NSImageView::new(mtm());
        glyph.setFrame(box_frame);
        glyph.setImageScaling(NSImageScaling::NSImageScaleProportionallyUpOrDown);
        glyph.setContentTintColor(Some(&hud_fill(TILE_ACTIVE_ALPHA)));
        // Point size + weight only, no scale: FineTune uses `.font(.system(size:weight:))`, so a
        // `.large` symbol scale would oversize the glyph.
        let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(
            point_size,
            NSFontWeightMedium,
        );
        glyph.setSymbolConfiguration(Some(&config));
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("sun.max.fill"),
            None,
        ) {
            glyph.setImage(Some(&image));
        }
        glyph
    }
}

/// Builds the panel for a style once. Rebuilt by `show` when the style changes.
fn build(style: HudStyle) -> Hud {
    match style {
        HudStyle::Tahoe => build_tahoe(),
        HudStyle::Classic => build_classic(),
    }
}

fn build_tahoe() -> Hud {
    unsafe {
        let size = NSSize::new(TAHOE_WIDTH, TAHOE_HEIGHT);
        let (window, effect) = make_window(size, TAHOE_CORNER);

        // Title row near the top.
        let title = NSTextField::labelWithString(&NSString::from_str("Brightness"), mtm());
        title.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, NSFontWeightSemibold)));
        title.setTextColor(Some(&NSColor::labelColor()));
        title.setAlignment(NSTextAlignment::Left);
        title.setDrawsBackground(false);
        title.setBezeled(false);
        title.setFrame(NSRect::new(
            NSPoint::new(TAHOE_PAD_X, TAHOE_HEIGHT - 14.0 - 18.0),
            NSSize::new(TAHOE_WIDTH - TAHOE_PAD_X * 2.0, 18.0),
        ));
        effect.addSubview(&title);

        // Bottom row: [glyph] [bar] [percentage], vertically centred near y = 20.
        let row_y = 18.0;
        let glyph = make_glyph(
            13.0,
            NSRect::new(
                NSPoint::new(TAHOE_PAD_X, row_y),
                NSSize::new(TAHOE_GLYPH_BOX, TAHOE_GLYPH_BOX),
            ),
        );
        effect.addSubview(&glyph);

        // Percentage, right-aligned.
        let label = NSTextField::labelWithString(&NSString::from_str("0%"), mtm());
        label.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            11.0,
            NSFontWeightSemibold,
        )));
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        label.setAlignment(NSTextAlignment::Right);
        label.setDrawsBackground(false);
        label.setBezeled(false);
        label.setFrame(NSRect::new(
            NSPoint::new(
                TAHOE_WIDTH - TAHOE_PAD_X - TAHOE_PERCENT_WIDTH,
                row_y + (TAHOE_GLYPH_BOX - 14.0) / 2.0,
            ),
            NSSize::new(TAHOE_PERCENT_WIDTH, 14.0),
        ));
        effect.addSubview(&label);

        // Capsule progress bar between the glyph and the percentage.
        let bar_x = TAHOE_PAD_X + TAHOE_GLYPH_BOX + 8.0;
        let bar_right = TAHOE_WIDTH - TAHOE_PAD_X - TAHOE_PERCENT_WIDTH - 8.0;
        let track_width = (bar_right - bar_x).max(0.0);
        let bar_y = row_y + (TAHOE_GLYPH_BOX - TAHOE_BAR_HEIGHT) / 2.0;
        let track = solid_box(
            NSRect::new(
                NSPoint::new(bar_x, bar_y),
                NSSize::new(track_width, TAHOE_BAR_HEIGHT),
            ),
            &hud_fill(0.25),
            TAHOE_BAR_HEIGHT / 2.0,
        );
        let fill = solid_box(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, TAHOE_BAR_HEIGHT)),
            &hud_fill(1.0),
            TAHOE_BAR_HEIGHT / 2.0,
        );
        track.addSubview(&fill);
        effect.addSubview(&track);

        Hud {
            style: HudStyle::Tahoe,
            window,
            glyph,
            dismiss: None,
            fill: Some(fill),
            label: Some(label),
            tiles: Vec::new(),
        }
    }
}

fn build_classic() -> Hud {
    unsafe {
        let size = NSSize::new(CLASSIC_SIZE, CLASSIC_SIZE);
        let (window, effect) = make_window(size, CLASSIC_CORNER);

        // Large sun glyph in the upper portion, positioned to match FineTune's rendered Classic
        // HUD (glyph sits high). CLASSIC_GLYPH_Y is the box bottom in AppKit's bottom-left space.
        let glyph = make_glyph(
            CLASSIC_ICON_POINT,
            NSRect::new(
                NSPoint::new((CLASSIC_SIZE - CLASSIC_ICON) / 2.0, CLASSIC_GLYPH_Y),
                NSSize::new(CLASSIC_ICON, CLASSIC_ICON),
            ),
        );
        effect.addSubview(&glyph);

        // Sixteen square tiles, centred horizontally, sitting low near the bottom to match
        // FineTune's rendered Classic HUD. CLASSIC_TILE_Y is the tile bottom in AppKit space.
        let row_width =
            CLASSIC_TILES as f64 * CLASSIC_TILE + (CLASSIC_TILES as f64 - 1.0) * CLASSIC_TILE_SPACING;
        let start_x = (CLASSIC_SIZE - row_width) / 2.0;
        let tile_y = CLASSIC_TILE_Y;
        let mut tiles = Vec::with_capacity(CLASSIC_TILES);
        for index in 0..CLASSIC_TILES {
            let x = start_x + index as f64 * (CLASSIC_TILE + CLASSIC_TILE_SPACING);
            let tile = solid_box(
                NSRect::new(NSPoint::new(x, tile_y), NSSize::new(CLASSIC_TILE, CLASSIC_TILE)),
                &hud_fill(TILE_INACTIVE_ALPHA),
                1.0,
            );
            effect.addSubview(&tile);
            tiles.push(tile);
        }

        Hud {
            style: HudStyle::Classic,
            window,
            glyph,
            dismiss: None,
            fill: None,
            label: None,
            tiles,
        }
    }
}

/// Positions the panel on the screen under the pointer: Tahoe top-right, Classic bottom-centre,
/// each inset like FineTune.
fn place(window: &NSWindow, style: HudStyle) {
    unsafe {
        let mouse = objc2_app_kit::NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm());
        let mut visible = screens
            .get_retained(0)
            .map(|screen| screen.visibleFrame())
            .unwrap_or(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0)));
        for index in 0..screens.len() {
            let Some(screen) = screens.get_retained(index) else {
                continue;
            };
            let frame = screen.frame();
            if mouse.x >= frame.origin.x
                && mouse.x <= frame.origin.x + frame.size.width
                && mouse.y >= frame.origin.y
                && mouse.y <= frame.origin.y + frame.size.height
            {
                visible = screen.visibleFrame();
                break;
            }
        }
        let size = window.frame().size;
        let (x, y) = match style {
            HudStyle::Tahoe => (
                visible.origin.x + visible.size.width - size.width - SCREEN_INSET,
                visible.origin.y + visible.size.height - size.height - SCREEN_INSET,
            ),
            HudStyle::Classic => (
                visible.origin.x + (visible.size.width - size.width) / 2.0,
                visible.origin.y + CLASSIC_BOTTOM_INSET,
            ),
        };
        window.setFrameOrigin(NSPoint::new(x.round(), y.round()));
    }
}

#[cfg(test)]
mod tests {
    use super::{filled_tiles, hud_spec, CLASSIC_GLYPH_Y, CLASSIC_TILE_Y};

    #[test]
    fn classic_geometry_matches_finetune_render() {
        // Measured from FineTune's rendered Classic HUD (200 pt panel): glyph optical centre
        // ~87 pt below the top edge (AppKit centre 113; 94 pt box -> bottom 66), tiles low with a
        // bottom of 27. The glyph must sit clearly above the tiles.
        assert!((CLASSIC_GLYPH_Y - 66.0).abs() < 1e-9);
        assert!((CLASSIC_TILE_Y - 27.0).abs() < 1e-9);
        assert!(CLASSIC_GLYPH_Y > CLASSIC_TILE_Y);
    }

    #[test]
    fn maps_value_to_fraction_and_percent() {
        let spec = hud_spec(50, 100, true);
        assert!((spec.fraction - 0.5).abs() < 1e-9);
        assert_eq!(spec.percent, 50);
        assert!(spec.controllable);
    }

    #[test]
    fn full_and_empty() {
        assert_eq!(hud_spec(100, 100, true).percent, 100);
        assert!((hud_spec(100, 100, true).fraction - 1.0).abs() < 1e-9);
        assert_eq!(hud_spec(0, 100, true).percent, 0);
        assert_eq!(hud_spec(0, 100, true).fraction, 0.0);
    }

    #[test]
    fn handles_ddc_ranges_other_than_100() {
        // A monitor reporting value/max of 50/80 is 62.5% -> 63.
        let spec = hud_spec(50, 80, true);
        assert_eq!(spec.percent, 63);
    }

    #[test]
    fn clamps_value_above_max_and_guards_zero_max() {
        assert_eq!(hud_spec(200, 100, true).percent, 100);
        assert_eq!(hud_spec(10, 0, true).percent, 100);
    }

    #[test]
    fn not_available_state_is_carried_through() {
        let spec = hud_spec(40, 100, false);
        assert!(!spec.controllable);
        assert_eq!(spec.percent, 40);
    }

    #[test]
    fn classic_tiles_round_to_the_nearest() {
        assert_eq!(filled_tiles(0.0, 16), 0);
        assert_eq!(filled_tiles(1.0, 16), 16);
        assert_eq!(filled_tiles(0.5, 16), 8);
        // 0.72 * 16 = 11.52 -> 12
        assert_eq!(filled_tiles(0.72, 16), 12);
        // Clamps out-of-range fractions.
        assert_eq!(filled_tiles(1.5, 16), 16);
        assert_eq!(filled_tiles(-0.2, 16), 0);
    }
}
