//! Puts the design system's `audio-mixer` icon into `assets/app.ico` and embeds it in
//! the executable.
//!
//! Azur's icon brief wants the mark on three surfaces, and this is the one that has to
//! happen before the program runs: the `.ico` in the executable's PE resources, which
//! is what Explorer, a shortcut and a pinned taskbar button show without ever starting
//! the process. The other two — the notification area and the panel's own header — are
//! drawn at run time from [`mark`](../src/mark.rs), from the same geometry.
//!
//! The file is copied rather than generated. `azur-egui-theme/app-icons/` ships a
//! drawn `.ico` per application; taking it as it is means the executable carries the
//! design system's own bytes, and that its nine PNG-compressed entries cost 15KB of
//! resources where re-encoding them as uncompressed BMP was 360KB. It is copied into
//! the tree rather than read straight out of the sibling directory so that `assets/`
//! is what the linker, a reviewer and an installer all see, and so that a build still
//! has an icon if the design system is not checked out beside us.

/// The design system's audio mixer, relative to this crate.
///
/// A sibling path, which is how `azur-egui-theme` is depended on in the first place.
const SOURCE: &str = "../azur-egui-theme/app-icons/audio-mixer/app.ico";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={SOURCE}");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    // The file this script writes, watched as well as written: deleting a build
    // script's own output does not otherwise make Cargo re-run it, so `assets/app.ico`
    // could go missing from the tree while the build kept happily linking the resource
    // it compiled last time. Watching it costs one extra re-run after each change —
    // the write-if-changed below is what stops that from becoming a loop.
    println!("cargo:rerun-if-changed=assets/app.ico");

    let assets = std::path::Path::new("assets");
    if let Err(error) = std::fs::create_dir_all(assets) {
        println!("cargo:warning=could not create assets/: {error}");
        return;
    }
    let icon = assets.join("app.ico");

    // Refreshed from the design system when it is there, and only when the bytes
    // differ: a build should not touch the working tree, and `rerun-if-changed` on a
    // file this script writes could otherwise loop with itself.
    match std::fs::read(SOURCE) {
        Ok(shipped) => {
            let unchanged = std::fs::read(&icon)
                .map(|old| old == shipped)
                .unwrap_or(false);
            if !unchanged {
                if let Err(error) = std::fs::write(&icon, &shipped) {
                    println!("cargo:warning=could not write {}: {error}", icon.display());
                }
            }
        }
        // Not a failure: whatever is already in `assets/` is still the design system's
        // icon from the last build that could see it.
        Err(error) => println!("cargo:warning=could not read {SOURCE}: {error}"),
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
