//! The application view: version picker, download control and progress.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::base::SelectableText;
use gpui_kit::component::searchable_list::{SearchableGroup, SearchableVec};
use gpui_kit::component::{
    ActiveTheme, Disableable, TitleBar,
    button::{Button, ButtonVariants},
    progress::Progress,
    select::{Select, SelectEvent, SelectState},
    tooltip::Tooltip,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, AsyncApp, Context, ElementId, Entity, FocusHandle, FontWeight,
    Hsla, InteractiveElement, IntoElement, MouseButton, MouseMoveEvent, ParentElement,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
    div, img, px,
};

use crate::download::{self, CancelFlag, DownloadEvent};
use crate::versions::{self, VersionInfo, latest_tag};

/// The project's repository, shown in the footer and opened by its button.
const PROJECT_URL: &str = "https://github.com/miloira/wxhook";

/// Where the download currently stands.
enum Transfer {
    Idle,
    Running {
        version: String,
        downloaded: u64,
        total: Option<u64>,
        bytes_per_sec: f64,
        cancel: CancelFlag,
    },
    Done {
        path: PathBuf,
    },
    Failed {
        error: String,
    },
    Cancelled,
}

/// Whether a version-list load is in flight.
enum LoadState {
    Loading,
    Ready,
    /// A manual refresh is running while the current list stays on screen.
    Refreshing,
}

pub struct WxApp {
    versions: Vec<VersionInfo>,
    select: Entity<SelectState<SearchableVec<SearchableGroup<VersionInfo>>>>,
    /// The loaded catalog, with its source and any error. `None` until the
    /// first load finishes.
    catalog: Option<versions::Catalog>,
    load_state: LoadState,
    transfer: Transfer,
    save_dir: PathBuf,
    /// Keyboard focus for the whole view.
    ///
    /// `Root` binds `Ctrl+C` to the copy action under its own `"Root"` key
    /// context. With nothing focused, a key event dispatches only to the root
    /// node, whose path does not carry that context, so the binding never
    /// matches and copying a selection silently does nothing. Holding focus on
    /// this view keeps `"Root"` (an ancestor) on the dispatch path.
    focus: FocusHandle,
}

impl WxApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<SearchableGroup<VersionInfo>>::new()),
                None,
                window,
                cx,
            )
            .searchable(true)
        });

        // Re-render when the picker commits a new selection, and clear the
        // previous transfer's result so its message does not linger.
        cx.subscribe(
            &select,
            |this,
             _entity,
             _event: &SelectEvent<SearchableVec<SearchableGroup<VersionInfo>>>,
             cx| {
                if !matches!(this.transfer, Transfer::Running { .. }) {
                    this.transfer = Transfer::Idle;
                }
                cx.notify();
            },
        )
        .detach();

        let mut this = Self {
            versions: Vec::new(),
            select,
            catalog: None,
            load_state: LoadState::Loading,
            transfer: Transfer::Idle,
            save_dir: default_save_dir(),
            focus: cx.focus_handle(),
        };
        // Focus the view so key events reach `Root`'s `Ctrl+C` binding from the
        // start; the interactive controls below (the version picker, buttons)
        // take focus when the user uses them, and root is an ancestor of all of
        // them, so copy keeps working either way.
        window.focus(&this.focus, cx);

        // Moving or resizing the window is driven by a native modal loop that
        // swallows the mouse-up, so a text-selection gesture started by that
        // same press is never ended. It then lies dormant until the next click,
        // which paints a stray selection from where the drag began to the
        // pointer. The title bar suppresses the selection outright, but the
        // resize band belongs to `Root`'s own window border, which we cannot
        // reach — so clear any window selection whenever the bounds change,
        // which covers both moving and resizing.
        cx.observe_window_bounds(window, |_, window, cx| {
            gpui_kit::base::TextSelection::clear(window, cx);
        })
        .detach();

        this.load_versions(window, cx, false);
        this
    }

    /// Load the version list on a worker thread, then populate the picker.
    ///
    /// `force_refresh` skips the cache and asks GitHub for a fresh list; that is
    /// what the refresh button uses.
    fn load_versions(&mut self, window: &mut Window, cx: &mut Context<Self>, force_refresh: bool) {
        self.load_state = if self.catalog.is_some() {
            LoadState::Refreshing
        } else {
            LoadState::Loading
        };
        cx.notify();

        let (tx, rx) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("wx-versions".to_string())
            .spawn(move || {
                let _ = tx.send_blocking(versions::load_catalog(force_refresh));
            })
            .ok();

        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
            let Ok(catalog) = rx.recv().await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.versions = catalog.versions.clone();
                this.catalog = Some(catalog);
                this.load_state = LoadState::Ready;
                this.rebuild_select(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The refresh button: re-fetch from GitHub, bypassing the cache.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.load_state, LoadState::Refreshing) {
            return;
        }
        self.load_versions(window, cx, true);
    }

    /// Replace the picker's items and preselect the newest version.
    fn rebuild_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let groups = versions::group_by_series(&self.versions);
        let newest = versions::newest_index(&groups);
        self.select.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(groups), window, cx);
            state.set_selected_index(newest, window, cx);
        });
    }

    /// The selected version, resolved against the loaded catalog.
    fn selected(&self, cx: &App) -> Option<VersionInfo> {
        let version = self.select.read(cx).selected_value().cloned()?;
        self.versions.iter().find(|v| v.version == version).cloned()
    }

    fn start_download(&mut self, info: VersionInfo, cx: &mut Context<Self>) {
        let dest = unique_destination(&self.save_dir, &info.asset_name);
        let cancel: CancelFlag = Default::default();
        let (tx, rx) = async_channel::unbounded();
        download::spawn_download(info.download_url.clone(), dest, cancel.clone(), tx);

        self.transfer = Transfer::Running {
            version: info.version.clone(),
            downloaded: 0,
            total: info.size,
            bytes_per_sec: 0.0,
            cancel,
        };

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(event) = rx.recv().await {
                let terminal = event.is_terminal();
                this.update(cx, |this, cx| {
                    this.apply_event(event);
                    cx.notify();
                })
                .ok();
                if terminal {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_event(&mut self, event: DownloadEvent) {
        match event {
            DownloadEvent::Started { total } => {
                if let Transfer::Running { total: slot, .. } = &mut self.transfer {
                    *slot = total;
                }
            }
            DownloadEvent::Progress {
                downloaded,
                total,
                bytes_per_sec,
            } => {
                if let Transfer::Running {
                    downloaded: d,
                    total: t,
                    bytes_per_sec: s,
                    ..
                } = &mut self.transfer
                {
                    *d = downloaded;
                    *t = total;
                    *s = bytes_per_sec;
                }
            }
            DownloadEvent::Finished { path } => self.transfer = Transfer::Done { path },
            DownloadEvent::Cancelled => self.transfer = Transfer::Cancelled,
            DownloadEvent::Failed { error } => self.transfer = Transfer::Failed { error },
        }
    }

    fn cancel_download(&mut self, cx: &mut Context<Self>) {
        if let Transfer::Running { cancel, .. } = &self.transfer {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        cx.notify();
    }

    fn pick_directory(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择安装包保存目录".into()),
        });

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    this.save_dir = path;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    // MARK: rendering

    /// The custom title bar: logo and title on the left, window controls on the
    /// right. Owns dragging and double-click-to-maximize.
    fn title_bar(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        // The `TitleBar` handles press-to-drag itself but does not stop the
        // window-level text selection from also starting on the same press.
        // Dragging then moves the window through a native modal loop, so the
        // mouse-up never reaches GPUI and that selection gesture is left
        // dangling — the next click elsewhere paints a selection from the title
        // bar to the pointer. Suppress selection on the whole bar instead, the
        // way the component library's own controls do.
        div()
            .id("title-bar-guard")
            .w_full()
            .flex_shrink(0.)
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                gpui_kit::base::GlobalState::suppress_text_selection(cx);
            })
            .child(
                TitleBar::new()
                    .bg(theme.background)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                // The logo is a true-colour PNG generated from
                                // assets/logo.svg at build time. It must be drawn
                                // as an image: GPUI's SVG renderer would flatten
                                // it into a single-colour silhouette.
                                img(crate::assets::LOGO_PATH).size(px(20.)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child("微信安装包下载器"),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn version_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let running = matches!(self.transfer, Transfer::Running { .. });
        let refreshing = matches!(self.load_state, LoadState::Refreshing);
        let loading = matches!(self.load_state, LoadState::Loading);

        // Status line: the source of the list (live / cache / offline), or the
        // fetch error when there is one.
        let (status_text, status_color) = match (&self.catalog, loading) {
            (_, true) => ("正在获取版本列表…".to_string(), theme.muted_foreground),
            (Some(catalog), false) => match &catalog.error {
                Some(error) => (error.clone(), theme.warning),
                None => (catalog.status(), theme.muted_foreground),
            },
            (None, false) => ("正在获取版本列表…".to_string(), theme.muted_foreground),
        };

        let refresh_button = Button::new("refresh")
            .label(if refreshing { "刷新中…" } else { "刷新" })
            .ghost()
            .icon(IconName::RotateCw)
            .disabled(refreshing)
            .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx)));

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(copyable("version-heading", "微信版本号")),
                    )
                    .child(refresh_button),
            )
            .child(
                Select::new(&self.select)
                    .id("version-select")
                    .w_full()
                    .menu_width(px(340.))
                    .menu_max_h(px(320.))
                    .placeholder("请选择版本")
                    .search_placeholder("搜索版本号…")
                    .disabled(running),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(status_color)
                    .child(copyable("status", status_text)),
            )
            .into_any_element()
    }

    fn details(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(info) = self.selected(cx) else {
            return div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(copyable("no-selection", "未选择版本"))
                .into_any_element();
        };

        let mut rows: Vec<AnyElement> = vec![
            version_row(&info, theme.foreground, theme.muted_foreground),
            detail_row(
                "系列",
                info.series.label().to_string(),
                theme.foreground,
                theme.muted_foreground,
            ),
            detail_row(
                "文件名",
                info.asset_name.clone(),
                theme.foreground,
                theme.muted_foreground,
            ),
        ];
        if info.tag != format!("v{}", info.version) {
            rows.push(detail_row(
                "标签",
                info.tag.clone(),
                theme.foreground,
                theme.muted_foreground,
            ));
        }
        if let Some(date) = &info.published {
            rows.push(detail_row(
                "发布日期",
                date.clone(),
                theme.foreground,
                theme.muted_foreground,
            ));
        }
        if let Some(size) = info.size_label() {
            rows.push(detail_row(
                "大小",
                size,
                theme.foreground,
                theme.muted_foreground,
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .child(div().flex().flex_col().gap_2().children(rows))
            .into_any_element()
    }

    /// The progress / status area beneath the details card.
    ///
    /// Takes no height while idle. The flexible spacer above the controls
    /// absorbs that height when a transfer starts, so the controls stay pinned
    /// to the bottom and nothing moves — provided the window is tall enough for
    /// both (see `INITIAL_HEIGHT` in `main.rs`).
    fn progress_area(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        match &self.transfer {
            Transfer::Idle => div().into_any_element(),
            Transfer::Running {
                version,
                downloaded,
                total,
                bytes_per_sec,
                ..
            } => {
                let percent = total
                    .filter(|t| *t > 0)
                    .map(|t| (*downloaded as f32 / t as f32) * 100.0)
                    .unwrap_or(0.0);
                let detail = match total {
                    Some(total) => format!(
                        "{} / {}  ·  {}/s",
                        human_size(*downloaded),
                        human_size(*total),
                        human_size(*bytes_per_sec as u64),
                    ),
                    None => format!(
                        "已下载 {}  ·  {}/s",
                        human_size(*downloaded),
                        human_size(*bytes_per_sec as u64)
                    ),
                };

                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(copyable("progress-version", format!("正在下载 v{version}")))
                            .child(copyable("progress-detail", detail)),
                    )
                    .child(
                        Progress::new("download-progress")
                            .w_full()
                            .loading(total.is_none())
                            .value(percent),
                    )
                    .into_any_element()
            }
            Transfer::Done { path } => div()
                .text_sm()
                .text_color(theme.success)
                .child(copyable(
                    "done-path",
                    format!("下载完成：{}", path.display()),
                ))
                .into_any_element(),
            Transfer::Failed { error } => div()
                .text_sm()
                .text_color(theme.danger)
                .child(copyable("failed", format!("下载失败：{error}")))
                .into_any_element(),
            Transfer::Cancelled => div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(copyable("cancelled", "已取消下载"))
                .into_any_element(),
        }
    }

    fn controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let running = matches!(self.transfer, Transfer::Running { .. });
        let has_selection = self.selected(cx).is_some();

        let primary = if running {
            Button::new("cancel")
                .label("取消下载")
                .danger()
                .icon(IconName::X)
                .on_click(cx.listener(|this, _, _, cx| this.cancel_download(cx)))
        } else {
            Button::new("download")
                .label("开始下载")
                .primary()
                .icon(IconName::Download)
                .disabled(!has_selection)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(info) = this.selected(cx) {
                        this.start_download(info, cx);
                    }
                }))
        };

        let mut row = div().flex().items_center().gap_3().child(primary);

        if let Transfer::Done { path } = &self.transfer {
            let path = path.clone();
            row = row.child(
                Button::new("reveal")
                    .label("打开所在文件夹")
                    .icon(IconName::FolderOpen)
                    .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&path))),
            );
        }

        let full_path = format!("保存到：{}", self.save_dir.display());

        row.child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        // Long paths ellipsize; hovering reveals the full path
                        // in a tooltip, so nothing is lost to truncation.
                        div()
                            .id("save-path")
                            .min_w_0()
                            .max_w(px(360.))
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .tooltip({
                                let full_path = full_path.clone();
                                move |window, cx| {
                                    // Bounded so a very long path wraps inside
                                    // the popup instead of running off screen.
                                    let path = full_path.clone();
                                    Tooltip::element(move |_, _| {
                                        div().max_w(px(520.)).child(path.clone())
                                    })
                                    .build(window, cx)
                                }
                            })
                            .child(copyable("save-path-text", full_path.clone())),
                    )
                    .child(
                        Button::new("pick-dir")
                            .label("更改目录")
                            .ghost()
                            .icon(IconName::Folder)
                            .disabled(running)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_directory(cx))),
                    ),
            )
            .into_any_element()
    }

    /// The footer: the project link, selectable so it can be copied.
    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(copyable("project-link-label", "项目地址："))
            .child(
                div()
                    .id("project-link-text")
                    .min_w_0()
                    .truncate()
                    .tooltip({
                        move |window, cx| {
                            Tooltip::element(|_, _| div().max_w(px(520.)).child(PROJECT_URL))
                                .build(window, cx)
                        }
                    })
                    .child(copyable("project-link", PROJECT_URL)),
            )
            .into_any_element()
    }
}

impl Render for WxApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .id("wx-app-root")
            .size_full()
            .flex()
            .flex_col()
            // Track a focus handle so key events (the `Ctrl+C` copy binding)
            // dispatch through `Root`'s context even when no control is focused.
            .track_focus(&self.focus)
            // Repaint while the pointer drags. A text-selection drag is not an
            // OS drag, so GPUI does not refresh the window on pointer movement
            // by itself; without this the selection only appears on the *next*
            // unrelated repaint (so it looked like nothing was highlighted until
            // release, or at all). Refreshing here makes the highlight follow
            // the pointer live.
            .on_mouse_move(cx.listener(|_, event: &MouseMoveEvent, window, _| {
                if event.pressed_button.is_some() {
                    window.refresh();
                }
            }))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.title_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_5()
                    .p_6()
                    // Tighter bottom edge: the footer's own spacing is 10px, so
                    // the link sits 10px above the window edge too.
                    .pb(px(10.))
                    .child(self.version_picker(cx))
                    .child(self.details(cx))
                    .child(self.progress_area(cx))
                    .child(div().flex_1())
                    // Controls and the footer sit together as one bottom group,
                    // with a tighter gap than the sections above, so the link
                    // hugs the control row instead of floating on its own line.
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(self.controls(cx))
                            .child(self.footer(cx)),
                    ),
            )
    }
}

/// The `版本号` line: the version, with the green "最新" tag when it is newest.
fn version_row(info: &VersionInfo, value_color: Hsla, key_color: Hsla) -> AnyElement {
    let mut value = div().flex().items_center().gap_2().child(
        div()
            .text_sm()
            .text_color(value_color)
            .child(copyable("detail-version", format!("v{}", info.version))),
    );
    if info.latest {
        value = value.child(latest_tag());
    }

    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(72.))
                .text_xs()
                .text_color(key_color)
                .child("版本号"),
        )
        .child(value)
        .into_any_element()
}

/// A `key: value` detail line.
fn detail_row(key: &'static str, value: String, value_color: Hsla, key_color: Hsla) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(72.)).text_xs().text_color(key_color).child(key))
        .child(
            div()
                .text_sm()
                .text_color(value_color)
                .child(copyable(key, value)),
        )
        .into_any_element()
}

/// Text the user can drag-select and copy with Ctrl+C.
///
/// `Root` already paints the window-scoped selection layer and installs the
/// copy handler, so a plain [`SelectableText`] is all a call site needs. It
/// inherits the text style from the styled `div` it is placed in, so callers
/// keep styling the wrapper as before.
fn copyable(id: impl Into<ElementId>, text: impl Into<SharedString>) -> SelectableText {
    SelectableText::new(id, text)
}

/// `%USERPROFILE%\Downloads` when it exists, otherwise the current directory.
fn default_save_dir() -> PathBuf {
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let downloads = PathBuf::from(profile).join("Downloads");
        if downloads.is_dir() {
            return downloads;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Append ` (1)`, ` (2)`, … when the target file already exists.
fn unique_destination(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);
    let ext = Path::new(file_name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("exe");
    for n in 1..10_000 {
        let candidate = dir.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    candidate
}

/// Format a byte count as B / KB / MB / GB.
fn human_size(bytes: u64) -> String {
    const KB: f64 = 1_000.0;
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_uses_readable_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1_500), "2 KB");
        assert_eq!(human_size(256_624_952), "256.6 MB");
    }

    #[test]
    fn unique_destination_avoids_overwriting() {
        let dir = std::env::temp_dir().join(format!("wx-dl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let first = unique_destination(&dir, "weixin_4.1.15.13.exe");
        assert_eq!(first.file_name().unwrap(), "weixin_4.1.15.13.exe");
        std::fs::write(&first, b"x").unwrap();

        let second = unique_destination(&dir, "weixin_4.1.15.13.exe");
        assert_eq!(second.file_name().unwrap(), "weixin_4.1.15.13 (1).exe");

        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Drag-select + Ctrl+C, driven through GPUI's headless test harness.
///
/// Synthetic OS input on Windows is timing-flaky (the clipboard write races the
/// selection registration), so the copy path is pinned down here instead: the
/// harness dispatches the same pointer and keystroke events a real window does,
/// deterministically.
#[cfg(test)]
mod selection_tests {
    use gpui_kit::base::TextSelection;
    use gpui_kit::component::Root;
    use gpui_kit::{AppContext as _, Modifiers, MouseButton, Render, TestAppContext, Window, div, px};

    use super::*;

    /// Hosts a single detail row — the same element the details card renders —
    /// under a `Root`, which supplies the selection layer and the copy binding.
    struct Host {
        focus: gpui_kit::FocusHandle,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus).p_4().child(detail_row(
                "文件名",
                "weixin_4.1.15.13.exe".to_string(),
                gpui_kit::black(),
                gpui_kit::black(),
            ))
        }
    }

    /// Builds the host under a `Root` and focuses it.
    fn harness(cx: &mut TestAppContext) -> &mut gpui_kit::VisualTestContext {
        cx.update(gpui_kit::init);
        let host = cx.update(|cx| {
            cx.new(|cx| Host {
                focus: cx.focus_handle(),
            })
        });
        let handle = cx.update(|cx| host.read(cx).focus.clone());
        let (_, cx) = cx.add_window_view(move |window, cx| Root::new(host, window, cx));
        cx.update(|window, cx| {
            window.focus(&handle, cx);
            let _ = window.draw(cx);
        });
        cx
    }

    /// A horizontal drag across the value paints a selection and Ctrl+C copies
    /// exactly the dragged-over text.
    #[gpui_kit::test]
    fn dragging_selects_and_ctrl_c_copies(cx: &mut TestAppContext) {
        let cx = harness(cx);

        // The host pads by 16px; the key column is 72px plus a 12px gap, so the
        // value's text starts around x=104 and the row sits at y≈28.
        let start = gpui::point(px(104.), px(28.));
        let end = gpui::point(px(300.), px(28.));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());

        // The highlight is not just painted — it is the live window selection.
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert_eq!(
                TextSelection::selected_text(window, cx),
                "weixin_4.1.15.13.exe"
            );
        });

        cx.simulate_keystrokes("ctrl-c");

        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("Ctrl+C should have written the selection to the clipboard");
        assert_eq!(copied, "weixin_4.1.15.13.exe");
    }

    /// Clicking without dragging must not copy anything: the interaction is
    /// select-then-copy, not click-to-copy.
    #[gpui_kit::test]
    fn a_plain_click_does_not_copy(cx: &mut TestAppContext) {
        let cx = harness(cx);

        cx.simulate_click(gpui::point(px(120.), px(28.)), Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_keystrokes("ctrl-c");

        assert!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()
                .is_empty(),
            "a click alone must not put anything on the clipboard"
        );
    }

    // MARK: title-bar press must not start a selection

    /// A title-bar strip over a selectable row. `guarded` mirrors the real
    /// title bar's suppression of the window text selection on press.
    struct BarHost {
        focus: gpui_kit::FocusHandle,
        guarded: bool,
    }

    fn bar(guarded: bool) -> AnyElement {
        // Owning the press is the point; the guard is what suppresses the
        // window selection that would otherwise also start on this press.
        let element = div().id("bar").w_full().h(px(24.));
        if guarded {
            element
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    gpui_kit::base::GlobalState::suppress_text_selection(cx);
                })
                .into_any_element()
        } else {
            element
                .on_mouse_down(MouseButton::Left, |_, _, _| {})
                .into_any_element()
        }
    }

    impl Render for BarHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.focus)
                .flex()
                .flex_col()
                .size_full()
                .child(bar(self.guarded))
                .child(div().p_4().child(detail_row(
                    "文件名",
                    "weixin_4.1.15.13.exe".to_string(),
                    gpui_kit::black(),
                    gpui_kit::black(),
                )))
        }
    }

    fn bar_harness(cx: &mut TestAppContext, guarded: bool) -> &mut gpui_kit::VisualTestContext {
        cx.update(gpui_kit::init);
        let host = cx.update(|cx| {
            cx.new(|cx| BarHost {
                focus: cx.focus_handle(),
                guarded,
            })
        });
        let handle = cx.update(|cx| host.read(cx).focus.clone());
        let (_, cx) = cx.add_window_view(move |window, cx| Root::new(host, window, cx));
        cx.update(|window, cx| {
            window.focus(&handle, cx);
            let _ = window.draw(cx);
        });
        cx
    }

    /// Pressing the title bar and dragging off it (which is how the window is
    /// moved) must not start or extend a text selection. Without the guard the
    /// gesture is left dangling — the window move swallows the mouse-up — and
    /// the next interaction paints a selection from the bar to the pointer.
    #[gpui_kit::test]
    fn pressing_the_title_bar_does_not_start_a_selection(cx: &mut TestAppContext) {
        let cx = bar_harness(cx, true);

        // Press on the 24px-tall bar, then drag down onto the value text.
        cx.simulate_mouse_down(
            gpui::point(px(50.), px(12.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            gpui::point(px(160.), px(50.)),
            MouseButton::Left,
            Modifiers::default(),
        );

        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert_eq!(
                TextSelection::selected_text(window, cx),
                "",
                "a press-drag that starts on the title bar must not select text"
            );
        });
    }

    /// The counterpart that documents the failure: the very same drag *does*
    /// select when the bar does not suppress selection, so the guard above is
    /// what makes the difference.
    #[gpui_kit::test]
    fn without_a_guard_the_same_drag_selects_text(cx: &mut TestAppContext) {
        let cx = bar_harness(cx, false);

        cx.simulate_mouse_down(
            gpui::point(px(50.), px(12.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            gpui::point(px(160.), px(50.)),
            MouseButton::Left,
            Modifiers::default(),
        );

        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(
                !TextSelection::selected_text(window, cx).is_empty(),
                "without the guard the drag should select text (this is the bug)"
            );
        });
    }
}
