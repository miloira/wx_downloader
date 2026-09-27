//! Application assets.
//!
//! Two things live here:
//!
//! * **Extra icons.** The default component bundle (`gpui_kit::assets::Assets`)
//!   embeds only the icons GPUI Component draws itself. `Download` and `X` exist
//!   in the full Lucide catalog but not in that bundle, so referencing them
//!   renders nothing. Just those two are embedded and used as a fallback.
//! * **The logo.** Rendered from `assets/logo.svg` at build time (see
//!   `build.rs`) and served as `logo.png`.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

// Only these two paths are embedded; every other path falls through to `Assets`.
icon_assets!(ExtraIcons, [Download, X]);

/// The logo, rasterized from `assets/logo.svg` by the build script.
const LOGO_PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/logo.png"));

/// The logo SVG source, embedded for tests (the UI draws the PNG).
#[cfg(test)]
const LOGO_SVG: &[u8] = include_bytes!("../assets/logo.svg");

/// The logo SVG source, for tests.
#[cfg(test)]
pub fn logo_svg_bytes() -> &'static [u8] {
    LOGO_SVG
}

/// Asset path the logo is served under.
pub const LOGO_PATH: &str = "logo.png";

/// The generated logo PNG bytes (used by tests to check the raster is valid).
#[cfg(test)]
pub fn logo_png_bytes() -> &'static [u8] {
    LOGO_PNG
}

/// The default component icons, extended with the extras this app draws and
/// the generated logo.
#[derive(Clone, Copy, Debug, Default)]
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == LOGO_PATH {
            return Ok(Some(Cow::Borrowed(LOGO_PNG)));
        }
        match Assets.load(path) {
            Ok(Some(bytes)) => Ok(Some(bytes)),
            _ => ExtraIcons.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut names = Assets.list(path)?;
        for name in ExtraIcons.list(path)? {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        if LOGO_PATH.starts_with(path) && !names.iter().any(|n| n == LOGO_PATH) {
            names.push(LOGO_PATH.into());
        }
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::assets::IconName;

    /// The button icons must resolve, or GPUI renders an empty box.
    #[test]
    fn extras_are_embedded() {
        for icon in [IconName::Download, IconName::X] {
            let path = icon.path().to_string();
            let bytes = AppAssets
                .load(&path)
                .unwrap_or_else(|err| panic!("{path} failed to load: {err}"))
                .unwrap_or_else(|| panic!("{path} is missing from the asset source"));
            assert!(!bytes.is_empty(), "{path} is empty");
            assert!(
                bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml"),
                "{path} does not look like SVG data"
            );
        }
    }

    /// Icons the default bundle already provides must still resolve through the
    /// composed source.
    #[test]
    fn defaults_still_resolve() {
        for icon in [IconName::Check, IconName::Folder, IconName::FolderOpen] {
            let path = icon.path().to_string();
            assert!(
                AppAssets.load(&path).unwrap().is_some(),
                "{path} should come from the default bundle"
            );
        }
    }

    /// Every icon the UI references must resolve through `AppAssets`.
    #[test]
    fn all_used_icons_resolve() {
        let used = [
            IconName::Download,
            IconName::X,
            IconName::Folder,
            IconName::FolderOpen,
            IconName::Check,
            IconName::ChevronDown,
        ];
        for icon in used {
            assert!(AppAssets.load(&icon.path()).unwrap().is_some());
        }
    }
}
