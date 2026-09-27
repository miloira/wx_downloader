//! WeChat Windows installer downloader — GPUI Kit GUI.

// Release builds are a GUI app: don't open a console window alongside them.
// Debug builds keep the console so `println!`/panics stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assets;
mod cache;
mod download;
mod http;
mod legacy;
mod logo;
mod theme;
mod versions;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{
    App, AppContext as _, Bounds, Pixels, Point, Size, TitlebarOptions, WindowBounds,
    WindowOptions, application, init, point, px, size,
};

/// Client size the window opens at, in logical pixels.
///
/// Roomy enough that the download progress bar—which only appears once a
/// transfer starts—fits without pushing the bottom row down: the flexible
/// spacer above the controls absorbs the extra height instead.
const INITIAL_WIDTH: f32 = 760.;
const INITIAL_HEIGHT: f32 = 560.;

/// Where to place a window of `window` size on the primary display.
///
/// Centres within the display's *visible* bounds, so the window does not open
/// partly behind the taskbar. Falls back to a fixed offset when the platform
/// reports no display.
fn centered_origin(cx: &App, window: Size<Pixels>) -> Point<Pixels> {
    let Some(display) = cx.primary_display() else {
        return point(px(140.), px(140.));
    };
    let area = display.visible_bounds();

    // Work in `f32` rather than relying on `Pixels` arithmetic.
    let x = f32::from(area.origin.x) + (f32::from(area.size.width) - f32::from(window.width)) / 2.0;
    let y =
        f32::from(area.origin.y) + (f32::from(area.size.height) - f32::from(window.height)) / 2.0;
    point(px(x), px(y))
}

/// Smallest the window may be resized to, in logical pixels.
///
/// The height is held at the opening height: below it the flexible spacer runs
/// out and the download progress bar would push the bottom row down.
const MIN_WIDTH: f32 = 600.;
const MIN_HEIGHT: f32 = INITIAL_HEIGHT;

fn main() {
    application().with_assets(assets::AppAssets).run(|cx| {
        init(cx);
        theme::apply(cx);

        let window_size = size(px(INITIAL_WIDTH), px(INITIAL_HEIGHT));
        let min_size = size(px(MIN_WIDTH), px(MIN_HEIGHT));

        // `appears_transparent` hides the OS title bar so the app can draw its
        // own (see `app.rs`), but the title is still set: Windows uses it for
        // the taskbar entry and Alt-Tab.
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                centered_origin(cx, window_size),
                window_size,
            ))),
            window_min_size: Some(min_size),
            titlebar: Some(TitlebarOptions {
                title: Some("微信安装包下载器".into()),
                ..TitleBar::title_bar_options()
            }),
            ..TitleBar::window_options()
        };

        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
                let view = cx.new(|cx| app::WxApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        })
        .detach();
    });
}
