//! Embeds the app icons from `packaging/icons/`:
//! - the 64x64 PNG as raw RGBA for the window icon (all platforms);
//! - `terminalrooster.ico` as the executable's icon on Windows.

use std::path::Path;

fn main() {
    let icons = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/icons");

    let png = icons.join("terminalrooster-64.png");
    println!("cargo:rerun-if-changed={}", png.display());
    let icon = image::open(&png).unwrap_or_else(|e| panic!("cannot read {}: {e}", png.display()));
    let rgba = icon.resize_exact(64, 64, image::imageops::FilterType::Lanczos3).to_rgba8();
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("icon-64.rgba");
    std::fs::write(out, rgba.as_raw()).unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let ico = icons.join("terminalrooster.ico");
        println!("cargo:rerun-if-changed={}", ico.display());
        let mut res = winresource::WindowsResource::new();
        res.set_icon(&ico.to_string_lossy());
        res.set("ProductName", "Terminal Rooster");
        res.set("FileDescription", "Terminal Rooster");
        if let Err(e) = res.compile() {
            // Missing resource compiler: build anyway, just without the .exe icon.
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
