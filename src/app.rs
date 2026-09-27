//! The application view: version picker, download control and progress.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::searchable_list::{SearchableGroup, SearchableVec};
use gpui_kit::component::{
    ActiveTheme, Disableable, TitleBar,
    button::{Button, ButtonVariants},
    progress::Progress,
    select::{Select, SelectEvent, SelectState},
};
use gpui_kit::{
    AnyElement, App, AppContext as _, AsyncApp, Context, Entity, FontWeight, Hsla, IntoElement,
    ParentElement, PathPromptOptions, Render, SharedString, Styled, WeakEntity, Window, div, img,
    px,
};

use crate::download::{self, CancelFlag, DownloadEvent};
use crate::versions::{self, VersionInfo, latest_tag};

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
        };
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
        TitleBar::new()
            .bg(theme.background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        // The logo is a true-colour PNG generated from
                        // assets/logo.svg at build time. It must be drawn as an
                        // image: GPUI's SVG renderer would flatten it into a
                        // single-colour silhouette.
                        img(crate::assets::LOGO_PATH).size(px(20.)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("微信安装包下载器"),
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
                            .child("微信版本号"),
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
            .child(div().text_xs().text_color(status_color).child(status_text))
            .into_any_element()
    }

    fn details(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(info) = self.selected(cx) else {
            return div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("未选择版本")
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
                            .child(format!("正在下载 v{version}"))
                            .child(detail),
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
                .child(format!("下载完成：{}", path.display()))
                .into_any_element(),
            Transfer::Failed { error } => div()
                .text_sm()
                .text_color(theme.danger)
                .child(format!("下载失败：{error}"))
                .into_any_element(),
            Transfer::Cancelled => div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("已取消下载")
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

        row.child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("保存到：{}", self.save_dir.display())),
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
}

impl Render for WxApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
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
                    .child(self.version_picker(cx))
                    .child(self.details(cx))
                    .child(self.progress_area(cx))
                    .child(div().flex_1())
                    .child(self.controls(cx)),
            )
    }
}

/// The `版本号` line: the version, with the green "最新" tag when it is newest.
fn version_row(info: &VersionInfo, value_color: Hsla, key_color: Hsla) -> AnyElement {
    let mut value = div().flex().items_center().gap_2().child(
        div()
            .text_sm()
            .text_color(value_color)
            .child(SharedString::from(format!("v{}", info.version))),
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
                .child(SharedString::from(value)),
        )
        .into_any_element()
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
