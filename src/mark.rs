//! The application's mark: Azur's `audio-mixer` icon — three faders, knobs at three
//! settings, on a plate in the dark theme's own `bg.layer`.
//!
//! **The art is not this program's.** `azur-egui-theme/app-icons/` ships a drawn icon
//! per application, and one of the four is an audio mixer. What is here is that
//! icon's geometry — `app-icons/marks.rs`, `AUDIO_MIXER` — and the rules its
//! generator rasterises with — `examples/app_icons.rs` — transcribed so the two
//! surfaces this program has to fill at *run time* can be drawn without carrying a
//! PNG decoder. `tests::the_art_is_the_design_systems` compares every size against the
//! shipped PNGs, so "transcribed" is a claim with a check behind it.
//!
//! | surface | how it gets here |
//! | --- | --- |
//! | Explorer, shortcuts, a pinned taskbar button | `build.rs` copies the design system's `app.ico` into `assets/`, and the linker embeds it |
//! | the notification area, and the window icon | [`rgba`], at whatever size the display scale asks for |
//! | the panel's own header, 16px | [`paint`] — the same primitives, through Azur's `Pen` |
//!
//! # Why the `.ico` is copied and the pixels are computed
//!
//! Two media, two answers. The `.ico` is a file the design system already ships, so
//! it is taken as it is: `build.rs` copies it and the linker embeds it, and the
//! executable ends up carrying the design system's bytes rather than a re-encoding of
//! them. It is also PNG-compressed, which is 15KB of PE resources where writing the
//! same nine images as uncompressed BMP entries was 360KB.
//!
//! The other two surfaces cannot be a file. The notification area wants an `HICON`
//! at `SM_CXSMICON` — 16, 20, 24 or 32px, depending on the display scale, decided
//! while the program is running — and the window icon wants straight RGBA. Both are
//! [`rgba`], because the alternative is a PNG decoder in the dependency graph of a
//! binary built with `opt-level = "s"` for the sake of two bitmaps.
//!
//! # Colour
//!
//! The design system's **dark chrome** pair, and both halves are roles rather than
//! shades: [`PLATE`] is `GRAY_2`, the dark theme's `bg.layer`, and [`INK`] is
//! `AZURE_70`, its `accent.mark` — 6.6:1 against the plate. The icon is in the same
//! paint as the panel it opens. They are literals here and checked against
//! `tokens::palette` *and* against `Theme::dark()` by a test in [`crate::icons`], so
//! neither can quietly become a shade that is nearly right.
//!
//! What that pair costs is worth knowing, and the design system says it out loud:
//! `GRAY_2` on Windows 11's dark taskbar is **1.1:1**. On a light taskbar this is a
//! deliberate dark tile at 16.6:1; on a dark one the plate disappears and the icon
//! reads as a blue mark floating on the chrome — which is how a good many Windows 11
//! icons look, and here it is a choice rather than an accident. `app-icons/README.md`
//! names the two ways out (lift the plate to `GRAY_4`, or give it a one-pixel rim)
//! and points at the one constant that does it. That constant is in the design
//! system, which is where the decision belongs: changing it here would fork the art
//! from the four icons it is a set with.

use azur_egui_theme::icons::Pen;
use egui::{Color32, Painter, Rect};

/// `palette::GRAY_2` — `#14171A`, the dark theme's `bg.layer`. The plate.
pub const PLATE: [u8; 3] = [0x14, 0x17, 0x1a];
/// `palette::AZURE_70` — `#3AA0FF`, the dark theme's `accent.mark`. The faders.
pub const INK: [u8; 3] = [0x3a, 0xa0, 0xff];

/// The mark's stroke weight in grid units, for the **raster** form only.
///
/// `AUDIO_MIXER.stroke`. A glyph on a plate carries more weight than one in a menu
/// row, so [`paint`] ignores this and takes `Pen`'s weight instead — which is what
/// keeps the header's copy level with the chevrons beside it.
const STROKE: f32 = 1.3;

/// The plate's corner radius as a fraction of the side: 25%, the brief's §14.
const CORNER: f32 = 0.25;

/// Subsamples per pixel per axis.
const SAMPLES: u32 = 4;

/// One drawing instruction on Azur's 16-unit icon grid.
///
/// The design system's `Prim` has two more variants — an open and a closed stroked
/// polyline — for the marks that need them. This mark is three lines and three
/// rounded rectangles, and carrying two variants it never uses would be two more
/// things to keep in step with upstream for nothing.
#[derive(Clone, Copy)]
pub enum Prim {
    /// A stroked straight line: `x1, y1, x2, y2`.
    Line(f32, f32, f32, f32),
    /// A rounded rectangle: `x, y, width, height, corner radius`.
    ///
    /// Filled, because an outline is self-defeating at this width: a 1.9-unit knob
    /// stroked at 1.3 units is a ring with a hole the size of the stroke. The brief's
    /// §4 is outline-first and this is the exception it allows for shapes too narrow
    /// to hold one.
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
    },
}

/// The mark, verbatim from the design system's `AUDIO_MIXER`.
///
/// Three full-height tracks with a knob on each, all three at different settings —
/// the knobs are what make this a mixer rather than a bar chart, which is the other
/// thing three vertical shapes read as at 16px. Knob widths just meet, and the
/// heights are staggered so neighbours never collide.
pub const PRIMS: &[Prim] = &[
    Prim::Line(4.4, 3.0, 4.4, 13.0),
    Prim::Line(8.0, 3.0, 8.0, 13.0),
    Prim::Line(11.6, 3.0, 11.6, 13.0),
    Prim::Rect {
        x: 2.6,
        y: 8.7,
        w: 3.6,
        h: 1.9,
        r: 0.95,
    },
    Prim::Rect {
        x: 6.2,
        y: 4.9,
        w: 3.6,
        h: 1.9,
        r: 0.95,
    },
    Prim::Rect {
        x: 9.8,
        y: 7.1,
        w: 3.6,
        h: 1.9,
        r: 0.95,
    },
];

/// The mark painted into `rect` in one colour — the mark's third form, and an
/// ordinary [`Icon`](azur_egui_theme::icons::Icon).
///
/// No plate: the header is already a surface, which is the brief's §17. Weight comes
/// from the box rather than from [`STROKE`], so this sits at the same weight as the
/// close and pin glyphs it shares a row with.
pub fn paint(painter: &Painter, rect: Rect, color: Color32) {
    let pen = Pen::new(painter, rect, color);
    for prim in PRIMS {
        match *prim {
            Prim::Line(x1, y1, x2, y2) => pen.line(x1, y1, x2, y2),
            Prim::Rect { x, y, w, h, r } => pen.rect_filled(x, y, w, h, r),
        }
    }
}

/// The mark at `size`, as straight (non-premultiplied) RGBA rows, top row first.
///
/// The plate fills the square and the only alpha in the image is the antialiasing of
/// its four corners — a semi-transparent icon body picks up whatever wallpaper is
/// behind it.
pub fn rgba(size: u32) -> Vec<u8> {
    let scale = size as f32 / 16.0;
    let (stroke, snap) = weight(size);
    let radius = size as f32 * CORNER;
    let half = size as f32 * 0.5;

    let step = 1.0 / SAMPLES as f32;
    let total = (SAMPLES * SAMPLES) as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];

    for py in 0..size {
        for px in 0..size {
            let (mut plate_hits, mut mark_hits) = (0.0f32, 0.0f32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = px as f32 + (sx as f32 + 0.5) * step;
                    let y = py as f32 + (sy as f32 + 0.5) * step;
                    if round_box(x, y, half, half, half, half, radius) <= 0.0 {
                        plate_hits += 1.0;
                    }
                    if inside_mark(x, y, scale, stroke, snap) {
                        mark_hits += 1.0;
                    }
                }
            }
            let plate = plate_hits / total;
            // Clipped to the plate, so the mark cannot spill out past a rounded
            // corner into a pixel the plate does not own.
            let ink = (mark_hits / total).min(plate);
            let rgb = mix(PLATE, INK, ink);

            let i = ((py * size + px) * 4) as usize;
            out[i..i + 3].copy_from_slice(&rgb);
            out[i + 3] = (plate * 255.0).round() as u8;
        }
    }
    out
}

/// The stroke weight this size is drawn at, and whether its edges are snapped.
///
/// The small sizes are pixel work rather than a downscale, and this is the whole of
/// that rule. A 16px mark stroked at 1.3 units would be 1.3px and read as grey, so
/// under 32px the weight is floored at 2px instead; and whole-pixel edges are only
/// worth having if the weight is a whole number too, so up to 48px it is rounded.
///
/// 16→2, 20→2, 24→2, 32→3, 40→3, 48→4, then 5.2, 10.4 and 20.8 unsnapped.
fn weight(size: u32) -> (f32, bool) {
    let stroke = (STROKE * size as f32 / 16.0).max(if size <= 32 { 2.0 } else { 0.0 });
    if size <= 48 {
        (stroke.round().max(1.0), true)
    } else {
        (stroke, false)
    }
}

/// Is this device-space point inside the mark's ink?
///
/// `snap` puts the edges of anything axis-aligned on whole pixels, which is the
/// difference between a crisp 16px icon and a grey one: a 2px track centred on x=4.4
/// covers two pixels at 65% and 35% and reads as a smudge.
fn inside_mark(x: f32, y: f32, scale: f32, stroke: f32, snap: bool) -> bool {
    let half = stroke * 0.5;
    // Grid units -> device pixels.
    let g = |u: f32| u * scale;
    // A stroke of width `w` centred on `c` covers `c ± w/2`; putting the near edge on
    // an integer is what makes it land on whole pixels.
    let center = |v: f32| if snap { (v - half).round() + half } else { v };
    let edge = |v: f32| if snap { v.round() } else { v };

    PRIMS.iter().any(|prim| match *prim {
        Prim::Line(x1, y1, x2, y2) => {
            let (mut ax, mut ay, mut bx, mut by) = (g(x1), g(y1), g(x2), g(y2));
            if (ax - bx).abs() < 0.01 {
                ax = center(ax);
                bx = ax;
            }
            if (ay - by).abs() < 0.01 {
                ay = center(ay);
                by = ay;
            }
            segment(x, y, ax, ay, bx, by) <= half
        }
        Prim::Rect {
            x: rx,
            y: ry,
            w,
            h,
            r,
        } => {
            let (x0, y0) = (edge(g(rx)), edge(g(ry)));
            let (x1, y1) = (edge(g(rx + w)), edge(g(ry + h)));
            round_box(
                x,
                y,
                (x0 + x1) * 0.5,
                (y0 + y1) * 0.5,
                (x1 - x0) * 0.5,
                (y1 - y0) * 0.5,
                g(r),
            ) <= 0.0
        }
    })
}

/// Distance from `p` to the segment `a`–`b`.
///
/// Distance to a segment is what rounds the ends of a track the way Azur's
/// `stroke-linecap="round"` does — `egui::Stroke` has no cap setting, so [`paint`]'s
/// copy has butt ends. At 16px that is sub-pixel; it is the gap the brief's §10
/// already records between the painted and the drawn glyph.
fn segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (pax, pay) = (px - ax, py - ay);
    let (bax, bay) = (bx - ax, by - ay);
    let len2 = bax * bax + bay * bay;
    let h = if len2 > 0.0 {
        ((pax * bax + pay * bay) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (pax - bax * h, pay - bay * h);
    (dx * dx + dy * dy).sqrt()
}

/// Signed distance to a rounded box: negative inside, zero on the edge.
fn round_box(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let r = r.min(hw).min(hh);
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

/// Mix two sRGB colours through linear light, so an antialiased edge does not come
/// out darker than the two sides it sits between.
fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let to_linear = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let to_srgb = |l: f32| {
        let s = if l <= 0.0031308 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        };
        (s.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    let mut out = [0u8; 3];
    for i in 0..3 {
        out[i] = to_srgb(to_linear(a[i]) + (to_linear(b[i]) - to_linear(a[i])) * t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the design system keeps the icon this module transcribes.
    ///
    /// A sibling of this crate, because `azur-egui-theme` is a path dependency: if the
    /// directory is not there then `cargo build` has already failed for a louder
    /// reason than this test.
    const REFERENCE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../azur-egui-theme/app-icons/audio-mixer"
    );

    /// Every size Windows asks for, and the ones the design system ships: 16/32/48/256
    /// for its own views, 20/24/40/64/128 for the display-scaling steps between.
    const SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

    /// The shipped PNG at `size`, decoded to straight RGBA.
    fn reference(size: u32) -> Vec<u8> {
        let path = format!("{REFERENCE}/app-{size}.png");
        let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("a PNG header");
        let mut buffer = vec![0u8; reader.output_buffer_size().expect("a bounded image")];
        let info = reader.next_frame(&mut buffer).expect("PNG pixels");
        assert_eq!(
            (info.width, info.height),
            (size, size),
            "{path}: not {size}x{size}"
        );
        assert_eq!(info.color_type, png::ColorType::Rgba, "{path}: not RGBA");
        assert_eq!(info.bit_depth, png::BitDepth::Eight, "{path}: not 8bpc");
        buffer.truncate(info.buffer_size());
        buffer
    }

    /// **The point of this module.** What [`rgba`] draws is the design system's icon,
    /// pixel for pixel, at every size it ships.
    ///
    /// This is what makes transcribing the geometry and the rasteriser legitimate
    /// rather than a second drawing that currently looks similar. If the design system
    /// redraws its audio mixer — or changes the plate, the corner, the stroke floor or
    /// the snapping rule — this fails with the size and the pixel, and the answer is
    /// to bring the transcription back into line, never to adjust the expectation.
    #[test]
    fn the_art_is_the_design_systems() {
        for size in SIZES {
            let theirs = reference(size);
            let ours = rgba(size);
            assert_eq!(ours.len(), theirs.len(), "{size}px: wrong length");
            if let Some((i, _)) = ours
                .chunks_exact(4)
                .zip(theirs.chunks_exact(4))
                .enumerate()
                .find(|(_, (ours, theirs))| ours != theirs)
            {
                let (x, y) = (i as u32 % size, i as u32 / size);
                let at = |pixels: &[u8]| {
                    let p = &pixels[i * 4..i * 4 + 4];
                    format!("{},{},{},{}", p[0], p[1], p[2], p[3])
                };
                panic!(
                    "{size}px: pixel {x},{y} is {} and the design system's is {}",
                    at(&ours),
                    at(&theirs)
                );
            }
        }
    }

    /// Dump every size for a look. Ignored by default: it writes files, and its real
    /// assertion is a pair of human eyes on three backgrounds — a light taskbar, a
    /// dark one and a photograph — which is what the brief's checklist asks for and
    /// what no `assert` can stand in for.
    #[test]
    #[ignore = "writes raw bitmaps for visual inspection"]
    fn dump() {
        let dir = std::env::var("MARK_DUMP").expect("MARK_DUMP");
        for size in SIZES {
            std::fs::write(format!("{dir}/mark-{size}.rgba"), rgba(size)).expect("write");
        }
    }

    fn alpha_at(pixels: &[u8], size: u32, x: u32, y: u32) -> u8 {
        pixels[((y * size + x) * 4 + 3) as usize]
    }

    /// The plate fills the square, and its corners are cut.
    #[test]
    fn the_plate_fills_the_square_with_rounded_corners() {
        for size in SIZES {
            let pixels = rgba(size);
            let middle = size / 2;
            assert_eq!(
                alpha_at(&pixels, size, middle, middle),
                255,
                "{size}px: the middle of the plate is not opaque"
            );
            // Right up to the edge along both midlines. There is no margin: the plate
            // is the square, and the mark's own margin is the 2–14 it is drawn in.
            assert_eq!(
                alpha_at(&pixels, size, 0, middle),
                255,
                "{size}px: the plate does not reach the left edge"
            );
            assert_eq!(
                alpha_at(&pixels, size, middle, 0),
                255,
                "{size}px: the plate does not reach the top edge"
            );
            assert_eq!(
                alpha_at(&pixels, size, 0, 0),
                0,
                "{size}px: the corner is not rounded"
            );
        }
    }

    /// Inside the plate there is no partial alpha: the brief's "full opacity inside
    /// the plate", which is what stops a wallpaper showing through the mark.
    #[test]
    fn nothing_inside_the_plate_is_translucent() {
        for size in SIZES {
            let pixels = rgba(size);
            for (i, pixel) in pixels.chunks_exact(4).enumerate() {
                let alpha = pixel[3];
                assert!(
                    alpha == 0 || alpha == 255 || is_on_a_corner(size, i as u32),
                    "{size}px: pixel {i} has alpha {alpha} away from a corner"
                );
            }
        }
    }

    /// Is this pixel in one of the four corner squares, where the radius is
    /// antialiased and partial alpha is the point?
    fn is_on_a_corner(size: u32, index: u32) -> bool {
        let radius = size as f32 * CORNER + 1.0;
        let (x, y) = ((index % size) as f32, (index / size) as f32);
        let near = |v: f32| v <= radius || v >= size as f32 - radius;
        near(x) && near(y)
    }

    /// Which of the two colours a pixel is nearer to.
    ///
    /// By distance to [`INK`] and [`PLATE`] rather than by a threshold on one channel:
    /// these tests ask whether the mark is *drawn* somewhere, and the answer must not
    /// depend on which pair of palette colours it is drawn in.
    fn is_ink(pixels: &[u8], size: u32, x: u32, y: u32) -> bool {
        let i = ((y * size + x) * 4) as usize;
        let distance = |to: [u8; 3]| {
            (0..3)
                .map(|c| (pixels[i + c] as i32 - to[c] as i32).pow(2))
                .sum::<i32>()
        };
        distance(INK) < distance(PLATE)
    }

    /// Grid units to the pixel that contains them.
    fn px(size: u32, unit: f32) -> u32 {
        ((unit * size as f32 / 16.0) as u32).min(size - 1)
    }

    /// Three separate tracks, at every size — including 16px, where a unit is a pixel
    /// and a gap that rounds away fuses two faders into one wide bar.
    #[test]
    fn the_three_faders_stay_apart() {
        for size in SIZES {
            let pixels = rgba(size);
            // Grid y=12 crosses all three tracks and none of the three knobs.
            let y = px(size, 12.0);
            for track in [4.4, 8.0, 11.6] {
                assert!(
                    is_ink(&pixels, size, px(size, track), y),
                    "{size}px: no track at grid x={track}"
                );
            }
            for gap in [6.2, 9.8] {
                assert!(
                    !is_ink(&pixels, size, px(size, gap), y),
                    "{size}px: the gap at grid x={gap} has closed up"
                );
            }
            // And nothing above the tracks: the mark keeps its margin inside the plate.
            assert!(
                !is_ink(&pixels, size, px(size, 8.0), px(size, 1.5)),
                "{size}px: ink where the plate should be plain"
            );
        }
    }

    /// A knob on each fader, and the three at three different settings — which is what
    /// makes this a mixer rather than the bar chart three vertical shapes otherwise
    /// read as.
    #[test]
    fn each_fader_has_a_knob_at_its_own_height() {
        for size in SIZES {
            let pixels = rgba(size);
            // The widest row in a fader's own column band is its knob: a knob is 3.6
            // units across where a track is 1.3.
            let knob_row = |left: f32, right: f32| {
                let (left, right) = (px(size, left), px(size, right));
                (0..size)
                    .max_by_key(|&y| {
                        (left..right)
                            .filter(|&x| is_ink(&pixels, size, x, y))
                            .count()
                    })
                    .expect("a non-empty icon")
            };
            // The bands are the knobs' own widths, which tile without overlapping.
            let first = knob_row(2.6, 6.2);
            let second = knob_row(6.2, 9.8);
            let third = knob_row(9.8, 13.4);
            assert!(
                second < third && third < first,
                "{size}px: knobs at rows {second}, {third}, {first} are not in the \
                 order the mark draws them (middle highest, then right, then left)"
            );
        }
    }

    /// The small sizes are redrawn rather than downscaled, and this is the rule that
    /// does it: a floor under the weight, and whole-pixel edges while the weight is a
    /// whole number.
    #[test]
    fn the_small_sizes_are_pixel_work() {
        let expected = [
            (16, 2.0, true),
            (20, 2.0, true),
            (24, 2.0, true),
            (32, 3.0, true),
            (40, 3.0, true),
            (48, 4.0, true),
            (64, 5.2, false),
            (128, 10.4, false),
            (256, 20.8, false),
        ];
        for (size, stroke, snap) in expected {
            let (actual, snapped) = weight(size);
            assert!(
                (actual - stroke).abs() < 0.001,
                "{size}px: stroke is {actual}, expected {stroke}"
            );
            assert_eq!(snapped, snap, "{size}px: wrong snapping");
            if snapped {
                assert_eq!(
                    actual,
                    actual.round(),
                    "{size}px: snapped a fractional stroke"
                );
            }
        }
    }
}
