//! The application logo.
//!
//! `assets/logo.svg` is the single source of truth for the brand mark: a
//! WeChat-green circle holding the white chat bubble, with a download badge in
//! the bottom-right corner.
//!
//! `build.rs` turns it into two artefacts at compile time: a true-colour
//! `logo.png` that the title bar draws (GPUI's SVG renderer only paints a
//! monochrome alpha mask tinted by a single colour), and a multi-size `logo.ico`
//! that is compiled into a Windows resource and linked into the executable, so
//! Explorer, the taskbar and Alt-Tab show the same mark. To review the artwork,
//! open the generated `target/*/build/wx_downloader-*/out/logo.png`.

#[cfg(test)]
mod tests {
    use crate::assets;

    fn svg_text() -> &'static str {
        std::str::from_utf8(assets::logo_svg_bytes()).expect("logo.svg is UTF-8")
    }

    /// Byte offset of the white chat-bubble path.
    fn bubble_path_start(svg: &str) -> usize {
        svg.find(r##"fill="#FFFFFF""##)
            .expect("the white chat bubble")
    }

    /// The badge: a disc holding a download arrow, in the bottom-right corner.
    #[test]
    fn logo_has_a_download_badge_in_the_corner() {
        let svg = svg_text();

        let badge = svg
            .find(r##"cx="96" cy="96" r="21""##)
            .expect("a badge disc in the bottom-right");
        // The badge sits over the bubble, so it must be drawn after it.
        assert!(
            badge > bubble_path_start(svg),
            "the badge must be drawn on top of the bubble"
        );
        // A downward arrow inside the disc is what makes it a *download*.
        let arrow = svg.find("M96 85 V99").expect("download arrow not found");
        assert!(arrow > badge, "the arrow belongs to the badge");
    }

    /// The disc is centred in the 128-unit viewBox's bottom-right quadrant and
    /// nudged inward so it stays inside the green circle.
    #[test]
    fn the_badge_is_offset_inward_from_the_corner() {
        let svg = svg_text();
        assert!(
            svg.contains(r#"<g transform="translate(-8,-8)">"#),
            "the badge should be pulled in from the corner"
        );
    }

    /// The brand green circle is the background; the bubble is white on top.
    #[test]
    fn background_is_a_green_circle_with_a_white_bubble() {
        let svg = svg_text();

        let circle = svg
            .find(r##"<circle cx="64" cy="64" r="62" fill="url(#bg)"/>"##)
            .expect("a full-size background circle");
        assert!(
            bubble_path_start(svg) > circle,
            "the bubble must be drawn on top of the circle"
        );
    }

    /// The mark uses the WeChat greens, not arbitrary ones.
    #[test]
    fn logo_uses_the_brand_greens() {
        let svg = svg_text();
        assert!(svg.contains("#2DC84D"), "missing the lighter brand green");
        assert!(svg.contains("#06B44C"), "missing the darker brand green");
    }

    /// A malformed SVG would render nothing, leaving a blank slot in the title
    /// bar. The structure is checked here; `build.rs` additionally fails the
    /// build if the SVG does not parse.
    #[test]
    fn logo_svg_is_well_formed() {
        let svg = svg_text();
        assert!(svg.trim_start().starts_with("<svg"), "missing <svg> root");
        assert!(svg.contains("viewBox="), "missing viewBox");
        assert!(svg.trim_end().ends_with("</svg>"), "unclosed <svg>");
        assert_eq!(svg.matches("<svg").count(), 1, "expected one <svg> root");
    }

    /// The generated PNG must be a decodable, non-empty square image — a blank
    /// or corrupt raster would show as an empty box in the title bar.
    #[test]
    fn rendered_png_is_decodable_and_square() {
        let png = assets::logo_png_bytes();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");

        // IHDR width/height are big-endian u32 at offsets 16 and 20.
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!(width, height, "logo PNG should be square");
        assert!(width >= 192, "logo PNG too small to look sharp: {width}");
        assert!(png.len() > 1000, "logo PNG looks empty");

        // The raster is complete (not cut off mid-stream).
        let iend = png.len() - 12;
        assert_eq!(&png[iend + 4..iend + 8], b"IEND", "PNG is truncated");
    }
}
