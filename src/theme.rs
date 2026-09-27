//! WeChat brand theming.
//!
//! GPUI Component ships a neutral (near-black) primary. We override the light
//! theme so the primary button, focused input/select borders, focus ring,
//! progress bar and hover accents all use WeChat's brand green.
//!
//! The overrides go through the theme config rather than per-component styling,
//! so every control that reads a semantic token picks them up — buttons,
//! inputs, the select trigger and the dropdown.

use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Hsla, hsla};

/// WeChat brand green (`#07C160`), used for the primary action colour.
pub const BRAND: Hsla = hsla(0.413_082_45, 0.93, 0.392_156_87, 1.0);

/// The brand green as a `#RRGGBB` string, for the theme config.
const BRAND_HEX: &str = "#07C160";
/// Pressed/hover variants, darkened from the brand green.
const BRAND_HOVER_HEX: &str = "#06AD56";
const BRAND_ACTIVE_HEX: &str = "#059748";
/// Very light green for menu/list hover backgrounds.
const BRAND_TINT_HEX: &str = "#E8F8EF";
/// Light green for selected text.
const BRAND_SELECTION_HEX: &str = "#B4EACB";
const WHITE_HEX: &str = "#FFFFFF";

/// Apply the WeChat theme to the running application.
pub fn apply(cx: &mut App) {
    // Start from the built-in light theme so every colour we do not touch
    // keeps its tuned default, then override the brand-related ones.
    let base = ThemeRegistry::global(cx).default_light_theme().clone();
    let config = wechat_theme(&base);

    Theme::global_mut(cx).light_theme = Rc::new(config);
    Theme::change(ThemeMode::Light, None, cx);
}

/// Derive the WeChat theme from a base light theme.
///
/// Kept separate from [`apply`] so the resulting colours can be asserted in
/// unit tests without an `App`.
fn wechat_theme(base: &ThemeConfig) -> ThemeConfig {
    let mut config = base.clone();
    config.name = "WeChat".into();
    config.is_default = false;

    let colors = &mut config.colors;
    // Primary surface: the primary Button, and the fallback behind the progress
    // bar, caret and selection.
    colors.primary = Some(BRAND_HEX.into());
    colors.primary_hover = Some(BRAND_HOVER_HEX.into());
    colors.primary_active = Some(BRAND_ACTIVE_HEX.into());
    colors.primary_foreground = Some(WHITE_HEX.into());
    // Button colours explicitly, in case a theme sets them apart from `primary`.
    colors.button_primary = Some(BRAND_HEX.into());
    colors.button_primary_hover = Some(BRAND_HOVER_HEX.into());
    colors.button_primary_active = Some(BRAND_ACTIVE_HEX.into());
    colors.button_primary_foreground = Some(WHITE_HEX.into());
    // Focused input/select border and the focus ring. The default falls back to
    // blue, so this one must be set explicitly.
    colors.ring = Some(BRAND_HEX.into());
    // Progress bar fill and text caret.
    colors.progress_bar = Some(BRAND_HEX.into());
    colors.caret = Some(BRAND_HEX.into());
    // Text selection and menu/list hover backgrounds.
    colors.selection = Some(BRAND_SELECTION_HEX.into());
    colors.accent = Some(BRAND_TINT_HEX.into());

    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::white;

    fn parsed(hex: &str) -> Hsla {
        gpui_kit::component::try_parse_color(hex).expect("valid test colour")
    }

    /// The brand green must reach every surface the user sees: primary button,
    /// focus ring / focused input border, and the progress bar.
    #[test]
    fn brand_green_reaches_the_semantic_surfaces() {
        let base = ThemeConfig {
            mode: ThemeMode::Light,
            ..Default::default()
        };
        let mut theme = Theme::default();
        theme.apply_config(&Rc::new(wechat_theme(&base)));

        let brand = parsed(BRAND_HEX);
        assert_eq!(theme.primary, brand, "primary button");
        assert_eq!(theme.ring, brand, "focused input / select border");
        assert_eq!(theme.tokens.progress_bar.color, brand, "progress bar");
        assert_eq!(
            theme.tokens.button_primary.color, brand,
            "primary button token"
        );
        assert_eq!(theme.primary_foreground, white(), "primary button text");
        assert_eq!(theme.tokens.button_primary_foreground.color, white());

        // `BRAND` is what the "最新" pill paints directly; it must match the
        // colour the theme resolves to.
        assert!((BRAND.h - brand.h).abs() < 1e-6, "hue");
        assert!((BRAND.s - brand.s).abs() < 1e-6, "saturation");
        assert!((BRAND.l - brand.l).abs() < 1e-6, "lightness");
    }

    /// Hover/active must be darker greens, not the brand colour repeated, or the
    /// button would not react visibly to the pointer.
    #[test]
    fn hover_and_active_are_distinct_greens() {
        let primary = parsed(BRAND_HEX);
        let hover = parsed(BRAND_HOVER_HEX);
        let active = parsed(BRAND_ACTIVE_HEX);

        assert_ne!(primary, hover);
        assert_ne!(hover, active);
        // Each step is darker (lower lightness) than the previous.
        assert!(hover.l < primary.l, "hover should be darker than brand");
        assert!(active.l < hover.l, "active should be darker than hover");
    }
}
