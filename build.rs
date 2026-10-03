//! Turns `assets/app-*.png` into the two forms the program needs: `assets/app.ico`,
//! which the linker puts in the executable's PE resources, and one raw-pixel blob per
//! small size for [`mark`](../src/mark.rs) to hand to Windows at run time.
//!
//! Azur's icon brief wants the mark on three surfaces, and all three come from the
//! same ten PNGs — `assets/icons.py` writes them, one per size, off a single render.
//! Two of the surfaces are decided while the program runs, so what this script does is
//! give both of them their pixels ahead of time:
//!
//! | surface | what it gets |
//! | --- | --- |
//! | Explorer, a shortcut, a pinned taskbar button | `assets/app.ico`, embedded in the executable's resources |
//! | the notification area, and the window icon | `$OUT_DIR/mark-*.rgba`, straight RGBA at each size the shell asks for |
//! | the panel's own header | the same blobs, uploaded as a texture |
//!
//! # Why the pixels are unpacked here
//!
//! Because the alternative is a PNG decoder in the shipped binary. The notification
//! area wants an `HICON` at `SM_CXSMICON` — 16, 20, 24, 28, 32, 40, 48 or 64px,
//! depending on the display scale, and not known until the program is running — and
//! the window icon and the header texture want straight RGBA. A decoder for that is
//! several crates and a good deal of code in a binary built with `opt-level = "s"`;
//! decoding at build time instead costs [`RESIDENT`]'s 44KB of `.rodata` and nothing
//! at startup, and the sizes over 64px stay in the `.ico`, where they are compressed
//! and where only the shell ever reads them.
//!
//! # Why the `.ico` is assembled rather than drawn
//!
//! An `.ico` is a header, a table and a run of payloads, and since Vista each payload
//! may be a whole PNG file. So the PNGs go in verbatim: no encoder here, nothing to
//! re-compress, and the bytes Explorer decodes are byte for byte the bytes
//! `assets/icons.py` wrote. That is also what keeps it small — 102KB of PE resources
//! where the same ten images as uncompressed BMP entries would be 363KB.
//!
//! It is written into `assets/` rather than `$OUT_DIR` so that the file the linker
//! reads is the file a reviewer and an installer can see, and rewritten only when its
//! bytes change, so a build never dirties the working tree for nothing.

/// Every size in `assets/`, and the same list `assets/icons.py` writes.
///
/// All ten go into the `.ico`; the shell picks. 16/32/48/256 are the sizes Windows
/// shows in its own views, 128 is the large-icon one, and the rest are the display
/// scaling steps the notification area asks for.
const SIZES: [u32; 10] = [16, 20, 24, 28, 32, 40, 48, 64, 128, 256];

/// The sizes whose pixels the binary carries, which is every size the shell can ask
/// for a tray icon at: `SM_CXSMICON` is 16px at 100% and 64 at 400%.
///
/// A prefix of [`SIZES`], and asserted to be one — the split is "small enough that
/// Windows may want it live", not a second list of sizes.
const RESIDENT: usize = 8;
const _: () = assert!(
    SIZES[RESIDENT - 1] == 64 && SIZES[RESIDENT] == 128,
    "RESIDENT has to end where the tray's sizes do"
);

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for size in SIZES {
        println!("cargo:rerun-if-changed=assets/{}", png_name(size));
    }
    // The file this script writes, watched as well as written: deleting a build
    // script's own output does not otherwise make Cargo re-run it, so `assets/app.ico`
    // could go missing from the tree while the build kept happily linking the resource
    // it compiled last time. Watching it costs one extra re-run after each change —
    // the write-if-changed below is what stops that from becoming a loop.
    println!("cargo:rerun-if-changed=assets/app.ico");

    let assets = std::path::Path::new("assets");
    let shipped: Vec<Vec<u8>> = SIZES
        .iter()
        .map(|&size| {
            let path = assets.join(png_name(size));
            // A hard failure, unlike the two below: these files *are* the icon. A
            // build that could not read them would go on to fail in `src/mark.rs`,
            // where `include_bytes!` would name a file in `$OUT_DIR` instead of the
            // one that is actually missing.
            std::fs::read(&path).unwrap_or_else(|error| {
                panic!("{}: {error} — run assets/icons.py", path.display())
            })
        })
        .collect();

    // The tray's and the window's pixels, unpacked into `$OUT_DIR` where `mark.rs`
    // includes them from.
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    for (&size, png) in SIZES.iter().zip(&shipped).take(RESIDENT) {
        let pixels = decode(png, size, &png_name(size));
        let path = out.join(format!("mark-{size}.rgba"));
        std::fs::write(&path, &pixels)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let icon = assets.join("app.ico");
    let assembled = ico(&SIZES, &shipped);
    // Only when the bytes differ: a build should not touch the working tree, and
    // `rerun-if-changed` on a file this script writes could otherwise loop with itself.
    let unchanged = std::fs::read(&icon).map(|old| old == assembled).unwrap_or(false);
    if !unchanged {
        if let Err(error) = std::fs::write(&icon, &assembled) {
            // A warning rather than a failure: a read-only checkout still builds, and
            // whatever `.ico` is already there is this same file from the last build.
            println!("cargo:warning=could not write {}: {error}", icon.display());
        }
    }

    if let Err(error) = winresource::WindowsResource::new()
        .set_icon(&icon.to_string_lossy())
        .compile()
    {
        // A warning rather than a failure: an executable with the generic icon still
        // switches audio devices, and this step needs a resource compiler that not
        // every toolchain on Windows has.
        println!("cargo:warning=could not embed {}: {error}", icon.display());
    }
}

fn png_name(size: u32) -> String {
    format!("app-{size}.png")
}

/// One PNG, as straight (non-premultiplied) RGBA rows, top row first.
///
/// Every assertion here is a promise `mark.rs` relies on and cannot check itself: it
/// indexes the blob as `size * size * 4` bytes of RGBA, and `resident.rs` builds a
/// bitmap of exactly `size` pixels a side out of it.
fn decode(png: &[u8], size: u32, name: &str) -> Vec<u8> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder
        .read_info()
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    let mut buffer = vec![0u8; reader.output_buffer_size().expect("a bounded image")];
    let info = reader
        .next_frame(&mut buffer)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(
        (info.width, info.height),
        (size, size),
        "{name}: not {size}x{size}"
    );
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "{name}: not RGBA — an ICO's PNG entries have to be 32bpp"
    );
    assert_eq!(info.bit_depth, png::BitDepth::Eight, "{name}: not 8bpc");
    buffer.truncate(info.buffer_size());
    buffer
}

/// An `.ico` holding each PNG as it is: a header, one 16-byte entry per image, then
/// the payloads.
fn ico(sizes: &[u32], images: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // 1 = icon, 2 = cursor
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());

    // Offsets are absolute and the table is fixed-width, so where the payloads start
    // is known before any of them is written.
    let mut offset = 6 + 16 * images.len() as u32;
    for (&size, image) in sizes.iter().zip(images) {
        assert!(size <= 256, "{size}px is past what an .ico can name");
        // 256 is spelled 0: the field is one byte and the format is older than the
        // size. Anything larger has no spelling at all, hence the assertion.
        let side = (size % 256) as u8;
        out.extend_from_slice(&[
            side, side, // width, height
            0,    // palette entries, 0 for a direct-colour image
            0,    // reserved
        ]);
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in images {
        out.extend_from_slice(image);
    }
    out
}
