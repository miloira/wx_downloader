//! Rasterizes the logo SVG into a PNG the app can render.
//!
//! GPUI's SVG renderer paints a *monochrome alpha mask* tinted by one colour,
//! which would flatten the logo's greens and white to a single silhouette.
//! Its image pipeline, by contrast, decodes PNGs in full colour — so the logo
//! is rendered here at build time and embedded as `logo.png`.
//!
//! Keeping the SVG as the only source of truth means the artwork cannot drift
//! from a committed binary.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Supersampling factor: the PNG is rendered large and scaled down in the UI,
/// which keeps the curves smooth.
const SCALE: f32 = 8.0;

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let svg_path = manifest_dir.join("assets/logo.svg");

    println!("cargo:rerun-if-changed=assets/logo.svg");
    println!("cargo:rerun-if-changed=build.rs");

    let svg = fs::read(&svg_path).expect("assets/logo.svg must exist");
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
        .expect("assets/logo.svg must be valid SVG");

    let size = tree.size().to_int_size();
    let width = (size.width() as f32 * SCALE).round().max(1.0) as u32;
    let height = (size.height() as f32 * SCALE).round().max(1.0) as u32;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .expect("logo renders to non-zero, in-range dimensions");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(SCALE, SCALE),
        &mut pixmap.as_mut(),
    );

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let out_path = out_dir.join("logo.png");
    let png = pixmap.encode_png().expect("logo encodes to PNG");
    fs::write(&out_path, png).expect("write logo.png");

    println!("cargo:logo-png={}", out_path.display());
}
