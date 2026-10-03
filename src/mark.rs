//! The application's mark: three faders and a speaker, blue on a near-black plate,
//! on the three surfaces Azur's icon brief asks for.
//!
//! The art is a render, kept in `assets/` as one PNG per size — `assets/icons.py` cuts
//! them out of it — and every surface is served from those ten files:
//!
//! | surface | how it gets here |
//! | --- | --- |
//! | Explorer, shortcuts, a pinned taskbar button | `build.rs` assembles the ten into `assets/app.ico` and the linker embeds it |
//! | the notification area, and the window icon | [`bitmap`], at whatever size the display scale asks for |
//! | the panel's own header | [`bitmap`] again, uploaded as a texture — see `App::mark` |
//!
//! # Why this module is a table of bitmaps
//!
//! Because the two run-time surfaces cannot be a file, and decoding one would cost
//! more than carrying it. The notification area wants an `HICON` at `SM_CXSMICON` —
//! 16, 20, 24, 28, 32, 40, 48 or 64px, depending on a display scale that is not known
//! until the program is running — and the window icon and the header texture want
//! straight RGBA. `build.rs` unpacks exactly those eight sizes into `$OUT_DIR` and
//! this includes them, so there is no PNG decoder in a binary built with
//! `opt-level = "s"`, no resampler, and nothing to do at startup but hand out a
//! slice. It costs 44KB of `.rodata`; the 128 and the 256, which only the shell ever
//! reads, stay compressed in the `.ico`.
//!
//! One bitmap per size, rather than one master scaled: 16px is not a small 64px. Each
//! was scaled from the render with its own filter pass and the small ones sharpened
//! after it, which is the difference between three knobs and three smudges.
//!
//! # Colour
//!
//! [`PLATE`] is `#14171C`, two steps of blue off `palette::GRAY_2` — the dark theme's
//! `bg.layer`, the panel's own surface. The icon really is the colour of the window it
//! opens, which is checked in [`crate::icons`] rather than claimed here.
//!
//! [`INK`] is `#3682FB`, the face of the middle knob. It is **not** the design
//! system's `accent.mark` (`AZURE_70`, `#3AA0FF`): the render's blue is deeper and
//! more saturated, and the brief's own line — that an icon blue nearly the app's blue
//! is worse than an obviously different one — is why this is written down instead of
//! being rounded to the token it is near. On the plate it measures 4.9:1, which
//! clears the design system's `contrast::TEXT` with little to spare — the old pair
//! managed 6.6:1 — and [`crate::icons`] pins the figure so the prose cannot drift.
//!
//! What the plate costs is worth knowing: near-black on Windows 11's dark taskbar is
//! about 1.1:1. On a light taskbar this is a deliberate dark tile; on a dark one the
//! plate disappears and the icon reads as blue faders floating on the chrome — which
//! is how a good many Windows 11 icons look, and here it is the art's own choice.

/// `#14171C` — the plate. Two steps of blue off the dark theme's `bg.layer`.
///
/// Not drawn with; see [`INK`].
#[allow(dead_code)]
pub const PLATE: [u8; 3] = [0x14, 0x17, 0x1c];

/// `#3682FB` — the face of the middle knob, and the icon's blue.
///
/// Neither this nor [`PLATE`] is drawn with, which is what the compiler has to be told
/// to accept: the art carries its own colour, and both are here to be *checked*. The
/// tests below find them in the pixels, and [`crate::icons`] measures them against the
/// theme and against each other. Two constants nothing reads are the price of the
/// paragraphs above them not being able to rot.
#[allow(dead_code)]
pub const INK: [u8; 3] = [0x36, 0x82, 0xfb];

/// Every size the binary carries, ascending — `SM_CXSMICON` at each display scale from
/// 100% to 400%, and the 64 the window icon is handed at startup.
///
/// A prefix of `build.rs`'s own list, which has the `.ico`'s 128 and 256 on the end.
pub const SIZES: [u32; 8] = [16, 20, 24, 28, 32, 40, 48, 64];

/// The pixels, in [`SIZES`] order: straight (non-premultiplied) RGBA rows, top row
/// first, `size * size * 4` bytes each.
///
/// Written by `build.rs` out of `assets/app-*.png`. Spelled out one line per size
/// rather than generated, so that what the binary carries is legible here and a size
/// cannot be added to `assets/` and silently not arrive.
const BITMAPS: [&[u8]; SIZES.len()] = [
    include_bytes!(concat!(env!("OUT_DIR"), "/mark-16.rgba")),
    include_bytes!(concat!(env!("OUT_DIR"), "/mark-20.rgba")),
    include_bytes!(concat!(env!("OUT_DIR"), "/mark-24.rgba")),
    include_bytes!(concat!(env!("OUT_DIR"), "/mark-28.rgba")),
    include_bytes!(concat!(env!("OUT_DIR"), "/mark-32.rgba")),
    include_bytes!(concat!(env!("OUT_DIR"), "/mark-40.rgba")),
    include_bytes!(concat!(env!("OUT_DIR"), "/mark-48.rgba")),
    include_bytes!(concat!(env!("OUT_DIR"), "/mark-64.rgba")),
];

/// The mark at the smallest size that is at least `wanted`, and the size it came out
/// at — which the caller has to use, because it is not always the one it asked for.
///
/// Rounding *up* and letting the consumer scale down is deliberate: a 40px icon shown
/// at 36 is soft, and a 32px one shown at 36 is soft and blocky. Above 64 there is
/// nothing to round up to and the largest is handed back; nothing asks for that today
/// — `SM_CXSMICON` stops at 64 — and an icon at 96px is a wrong-looking icon rather
/// than a missing one.
pub fn bitmap(wanted: u32) -> (u32, &'static [u8]) {
    let index = SIZES
        .iter()
        .position(|&size| size >= wanted)
        .unwrap_or(SIZES.len() - 1);
    (SIZES[index], BITMAPS[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes in `assets/`, and their bytes: `app-16.png` and its nine siblings.
    ///
    /// Read off the directory rather than from a list, so that a size added to
    /// `assets/` and left out of `build.rs` is a failure here instead of a file nobody
    /// notices is unused.
    fn shipped() -> Vec<(u32, Vec<u8>)> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
        let mut out: Vec<(u32, Vec<u8>)> = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("{dir}: {error}"))
            .map(|entry| entry.expect("a directory entry").path())
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?;
                let size = name.strip_prefix("app-")?.strip_suffix(".png")?;
                let size = size.parse().ok()?;
                Some((size, std::fs::read(&path).expect("an icon")))
            })
            .collect();
        out.sort_by_key(|(size, _)| *size);
        assert!(!out.is_empty(), "{dir}: no app-*.png — run assets/icons.py");
        out
    }

    /// What [`bitmap`] hands out is the size it says it is.
    ///
    /// The whole of this module's contract with `resident.rs`, which builds a GDI
    /// bitmap of exactly these dimensions out of the slice: a blob one row short is a
    /// tray icon with a line of the heap along the bottom.
    #[test]
    fn every_bitmap_is_the_size_it_claims() {
        for (size, pixels) in SIZES.iter().zip(BITMAPS) {
            assert_eq!(
                pixels.len(),
                (size * size * 4) as usize,
                "{size}px: {} bytes",
                pixels.len()
            );
        }
    }

    /// **The point of this module.** The pixels the binary carries are the PNGs in
    /// `assets/`, which is `build.rs`'s decode checked end to end.
    #[test]
    fn the_pixels_are_the_art_in_assets() {
        for (size, png) in shipped() {
            let Some(index) = SIZES.iter().position(|&s| s == size) else {
                continue; // the .ico's own sizes; `the_ico_holds_every_size` has those
            };
            let decoder = png::Decoder::new(std::io::Cursor::new(png));
            let mut reader = decoder.read_info().expect("a PNG header");
            let mut expected = vec![0u8; reader.output_buffer_size().expect("a bounded image")];
            let info = reader.next_frame(&mut expected).expect("PNG pixels");
            expected.truncate(info.buffer_size());
            assert_eq!(
                (info.width, info.height),
                (size, size),
                "app-{size}.png: not {size}x{size}"
            );
            assert_eq!(
                info.color_type,
                png::ColorType::Rgba,
                "app-{size}.png: not RGBA — an ICO's PNG entries have to be 32bpp"
            );
            if let Some((i, _)) = BITMAPS[index]
                .chunks_exact(4)
                .zip(expected.chunks_exact(4))
                .enumerate()
                .find(|(_, (ours, theirs))| ours != theirs)
            {
                let (x, y) = (i as u32 % size, i as u32 / size);
                panic!("{size}px: pixel {x},{y} is not the one in app-{size}.png");
            }
        }
    }

    /// Rounding up, and the two ends of it.
    #[test]
    fn a_size_between_two_is_taken_from_the_larger() {
        for size in SIZES {
            assert_eq!(bitmap(size).0, size, "{size}px is a size we have");
            assert_eq!(bitmap(size - 1).0, size, "{size}px should cover {}", size - 1);
        }
        assert_eq!(bitmap(0).0, 16, "nothing under 16 to fall to");
        assert_eq!(bitmap(65).0, 64, "nothing over 64 to reach for");
        assert_eq!(bitmap(4096).0, 64);
    }

    fn pixel(pixels: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        pixels[i..i + 4].try_into().expect("four channels")
    }

    /// The plate fills the square, and its corners are cut.
    ///
    /// There is no margin: the plate *is* the square, and the art keeps its own margin
    /// inside it. A transparent border here would show up as an icon a size too small
    /// beside every other one in the notification area.
    #[test]
    fn the_plate_fills_the_square_with_rounded_corners() {
        for (size, pixels) in SIZES.iter().copied().zip(BITMAPS) {
            let middle = size / 2;
            let alpha = |x, y| pixel(pixels, size, x, y)[3];
            assert_eq!(alpha(middle, middle), 255, "{size}px: the plate has a hole");
            assert_eq!(alpha(0, middle), 255, "{size}px: it misses the left edge");
            assert_eq!(alpha(middle, 0), 255, "{size}px: it misses the top edge");
            assert_eq!(
                alpha(size - 1, middle),
                255,
                "{size}px: it misses the right edge"
            );
            assert_eq!(alpha(0, 0), 0, "{size}px: the corner is not rounded");
        }
    }

    /// Away from the corners there is no partial alpha, at any size.
    ///
    /// The corners are the only place the art is allowed to be see-through: anywhere
    /// else, a pixel that is 40% opaque is 60% of whatever wallpaper is behind the
    /// taskbar, which is how an icon ends up with a different colour on every machine.
    #[test]
    fn nothing_inside_the_plate_is_translucent() {
        for (size, pixels) in SIZES.iter().copied().zip(BITMAPS) {
            // The corner squares, plus a pixel of slack for the arc's own edge.
            let corner = (size as f32 * 0.207) as u32 + 2;
            for y in 0..size {
                for x in 0..size {
                    let near = |v: u32| v <= corner || v >= size - 1 - corner;
                    if near(x) && near(y) {
                        continue;
                    }
                    let alpha = pixel(pixels, size, x, y)[3];
                    assert_eq!(alpha, 255, "{size}px: pixel {x},{y} is {alpha} opaque");
                }
            }
        }
    }

    /// [`PLATE`] and [`INK`] are colours the art is actually drawn in.
    ///
    /// Prose about an icon's palette rots quietly, and [`crate::icons`] goes on to
    /// measure these two against the theme and against each other — so if they are not
    /// in the art, three files are describing an icon that no longer exists.
    ///
    /// The plate is exact: it is a flat fill, and it survives every downscale. The ink
    /// is within a few steps, because it never survives one — the small sizes are a
    /// 20:1 reduction through an unsharp mask, and no pixel of a gradient comes out of
    /// that untouched. What the tolerance still catches is the art being redrawn in a
    /// different colour, which is the thing worth catching.
    #[test]
    fn the_art_is_drawn_in_the_colours_this_module_names() {
        const SLACK: i32 = 12;
        for (size, pixels) in SIZES.iter().copied().zip(BITMAPS) {
            let nearest = |to: [u8; 3]| {
                pixels
                    .chunks_exact(4)
                    .filter(|pixel| pixel[3] == 255)
                    .map(|pixel| {
                        (0..3)
                            .map(|c| (pixel[c] as i32 - to[c] as i32).abs())
                            .max()
                            .expect("three channels")
                    })
                    .min()
                    .expect("a non-empty icon")
            };
            assert_eq!(nearest(PLATE), 0, "{size}px: the plate is not PLATE");
            let ink = nearest(INK);
            assert!(ink <= SLACK, "{size}px: the nearest blue to INK is {ink} off");
        }
    }

    /// `assets/app.ico` holds every size in `assets/`, once, byte for byte.
    ///
    /// The `.ico` is the surface with no run-time check at all — Explorer either shows
    /// the icon or shows a blank page, on a machine that is not this one — so its
    /// header is parsed back here. Entries carrying a re-encoding rather than the
    /// shipped file would be a silently different icon in the one place a user sees it
    /// first.
    #[test]
    fn the_ico_holds_every_size() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/app.ico");
        let ico = std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"));
        let word = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]) as usize;
        let long = |at: usize| u32::from_le_bytes(ico[at..at + 4].try_into().unwrap()) as usize;

        assert_eq!(word(0), 0, "reserved");
        assert_eq!(word(2), 1, "not an icon");
        let count = word(4);
        let shipped = shipped();
        assert_eq!(count, shipped.len(), "{count} entries for {} sizes", shipped.len());

        for (index, (size, png)) in shipped.iter().enumerate() {
            let entry = 6 + 16 * index;
            // 256 is spelled 0: the field is one byte and the format is older than the
            // size.
            let side = match ico[entry] {
                0 => 256,
                side => side as u32,
            };
            assert_eq!(side, *size, "entry {index} is {side}px, expected {size}");
            assert_eq!(ico[entry + 1], ico[entry], "entry {index} is not square");
            assert_eq!(word(entry + 6), 32, "{size}px: not 32bpp");
            let (length, offset) = (long(entry + 8), long(entry + 12));
            assert_eq!(length, png.len(), "{size}px: {length} bytes for {}", png.len());
            assert_eq!(
                &ico[offset..offset + length],
                png.as_slice(),
                "{size}px: the entry is not app-{size}.png"
            );
        }
    }
}
