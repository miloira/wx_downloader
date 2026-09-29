//! Rasterizes the logo SVG into the images the app and the OS shell need.
//!
//! Two artefacts are produced at build time, both from `assets/logo.svg`:
//!
//! * `logo.png` — a true-colour PNG drawn in the title bar. GPUI's SVG renderer
//!   paints a *monochrome alpha mask* tinted by a single colour, which would
//!   flatten the logo's greens and white to one silhouette; its image pipeline,
//!   by contrast, decodes PNGs in full colour.
//! * `logo.ico` — a multi-size icon compiled into a Windows resource and linked
//!   into the executable, so Explorer, the taskbar and Alt-Tab show the mark.
//!   GPUI's `WindowOptions::icon` is X11-only, so on Windows the shell takes the
//!   icon from the executable itself.
//!
//! Keeping the SVG as the only source of truth means neither artefact can drift
//! from it or from each other.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Supersampling factor: the title-bar PNG is rendered large and scaled down in
/// the UI, which keeps the curves smooth.
const SCALE: f32 = 8.0;

/// Sizes baked into the Windows icon. 256 is the largest Explorer thumbnail;
/// the small ends keep the taskbar and Alt-Tab crisp.
const ICON_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let svg_path = manifest_dir.join("assets/logo.svg");

    println!("cargo:rerun-if-changed=assets/logo.svg");
    println!("cargo:rerun-if-changed=build.rs");

    let svg = fs::read(&svg_path).expect("assets/logo.svg must exist");
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
        .expect("assets/logo.svg must be valid SVG");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));

    // Title-bar logo: rendered large, drawn small.
    let natural = tree.size().to_int_size();
    let width = (natural.width() as f32 * SCALE).round().max(1.0) as u32;
    let height = (natural.height() as f32 * SCALE).round().max(1.0) as u32;
    let out_path = out_dir.join("logo.png");
    fs::write(&out_path, render_png(&tree, width, height)).expect("write logo.png");
    println!("cargo:logo-png={}", out_path.display());

    // The shell icon is a Windows-only concern, and only MSVC builds get a
    // resource linked into the executable here.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        // Never fail the build over the icon: the app itself does not need it.
        if let Err(err) = embed_windows_icon(&tree, &out_dir) {
            println!("cargo:warning=Windows shell icon not embedded: {err}");
        }
    }
}

/// Renders `tree` into an exact `width`×`height` RGBA pixmap and returns the
/// encoded PNG.
fn render_png(tree: &resvg::usvg::Tree, width: u32, height: u32) -> Vec<u8> {
    let size = tree.size();
    let scale_x = width as f32 / size.width();
    let scale_y = height as f32 / size.height();

    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .expect("logo renders to non-zero, in-range dimensions");
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(scale_x, scale_y),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().expect("logo encodes to PNG")
}

/// Builds a multi-size `.ico`, compiles it into a resource, and links that
/// resource into the executable.
fn embed_windows_icon(tree: &resvg::usvg::Tree, out_dir: &Path) -> Result<(), String> {
    let images: Vec<(u32, Vec<u8>)> = ICON_SIZES
        .iter()
        .map(|&size| (size, render_png(tree, size, size)))
        .collect();
    fs::write(out_dir.join("logo.ico"), ico_bytes(&images)).map_err(|err| err.to_string())?;

    // `1 ICON` gives the icon the lowest resource id, which is the one Windows
    // uses as the executable's (and thus the taskbar's) icon.
    fs::write(out_dir.join("app.rc"), "1 ICON \"logo.ico\"\n").map_err(|e| e.to_string())?;

    let rc = find_rc().ok_or("rc.exe not found (install the Windows SDK, or set RC)")?;
    let res_path = out_dir.join("app.res");
    let output = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_path)
        .arg("app.rc")
        .current_dir(out_dir)
        .output()
        .map_err(|err| format!("failed to run {}: {err}", rc.display()))?;
    if !output.status.success() {
        return Err(format!(
            "rc.exe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    println!("cargo:rustc-link-arg-bins={}", res_path.display());
    Ok(())
}

/// Serialises PNG-encoded images into the ICO container format.
///
/// Every entry is a PNG (Vista and later decode these directly), so no BMP
/// encoding is needed. A 256-pixel dimension is written as 0, as the format
/// requires.
fn ico_bytes(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    // ICONDIR: reserved, type (1 = icon), image count.
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());

    let mut offset = 6 + 16 * images.len();
    for (size, data) in images {
        let dimension = if *size >= 256 { 0 } else { *size as u8 };
        out.push(dimension); // width
        out.push(dimension); // height
        out.push(0); // palette colour count (0 = truecolour)
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }

    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}

/// Locates `rc.exe`, preferring an explicit `RC` and then the Windows SDK.
fn find_rc() -> Option<PathBuf> {
    if let Some(path) = env::var_os("RC") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            let candidate = dir.join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x64",
        Ok("aarch64") => "arm64",
        _ => "x86",
    };

    let mut kits = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(base) = env::var_os(var) {
            kits.push(PathBuf::from(base).join("Windows Kits").join("10").join("bin"));
        }
    }

    // Pick the newest SDK version, compared numerically ("10.0.26100.0" beats
    // "10.0.9999.0", which string order would get wrong).
    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    for kit in kits {
        let Ok(entries) = fs::read_dir(&kit) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join(arch).join("rc.exe");
            if !candidate.is_file() {
                continue;
            }
            let version: Vec<u32> = entry
                .file_name()
                .to_string_lossy()
                .split('.')
                .filter_map(|part| part.parse().ok())
                .collect();
            if best.as_ref().is_none_or(|(current, _)| version > *current) {
                best = Some((version, candidate));
            }
        }
    }
    best.map(|(_, path)| path)
}
