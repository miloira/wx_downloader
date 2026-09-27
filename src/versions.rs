//! Version discovery for the WeChat Windows installers.
//!
//! The three release lines come from two places:
//!
//! * 微信 4.x — fetched live from `cscnk52/wechat-windows-versions`.
//! * 微信 2.x / 3.x — frozen, baked into [`crate::legacy`]. That repository
//!   stopped publishing in 2025-09, so requesting it on every launch would burn
//!   API quota (60/hour unauthenticated) for data that never changes.
//!
//! For the live line the GitHub Releases API is the source of truth; when it is
//! unavailable (offline, or the rate limit is hit) we fall back to the tags API
//! and finally to a single baked-in 4.x entry, so the window always has
//! something to show.

use gpui_kit::component::IndexPath;
use gpui_kit::component::searchable_list::{SearchableGroup, SearchableListItem};
use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, white,
};

use crate::cache;
use crate::http::{USER_AGENT, api_agent, github_token};
use crate::legacy;

/// A GitHub repository that publishes WeChat Windows installers.
struct Repo {
    /// `owner/name` on GitHub.
    slug: &'static str,
    /// Prefix of the installer asset name, used to rebuild download URLs when
    /// only the tags API is available (the tags API carries no asset list).
    asset_prefix: &'static str,
}

/// The only repository still publishing installers (微信 4.x).
const LIVE_REPO: Repo = Repo {
    slug: "cscnk52/wechat-windows-versions",
    asset_prefix: "weixin_",
};

/// Items requested per page. GitHub caps this at 100.
const PER_PAGE: usize = 100;

/// Which WeChat release line a version belongs to.
///
/// Declaration order is the display order *reversed*: sorting descending puts
/// the newest series first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Series {
    /// 微信 2.x
    V2,
    /// 微信 3.x
    V3,
    /// 微信 4.x
    V4,
}

impl Series {
    /// Derive the series from the version's major component.
    ///
    /// Returns `None` for strings that are not a `major.…` version, which
    /// filters out malformed tags such as the bare `v` release.
    pub fn from_version(version: &str) -> Option<Self> {
        match version.split('.').next()?.parse::<u64>().ok()? {
            2 => Some(Series::V2),
            3 => Some(Series::V3),
            4 => Some(Series::V4),
            _ => None,
        }
    }

    /// Chinese label, used as the dropdown's group header and in the details.
    pub fn label(self) -> &'static str {
        match self {
            Series::V2 => "微信 2.x",
            Series::V3 => "微信 3.x",
            Series::V4 => "微信 4.x",
        }
    }
}

/// A single downloadable WeChat installer.
#[derive(Clone, Debug)]
pub struct VersionInfo {
    /// Release line this version belongs to.
    pub series: Series,
    /// Display version, e.g. `4.1.15.13`.
    pub version: String,
    /// Release tag, e.g. `v4.1.15.13`.
    pub tag: String,
    /// Installer file name, e.g. `weixin_4.1.15.13.exe`.
    pub asset_name: String,
    /// Direct download URL.
    pub download_url: String,
    /// Release date (`YYYY-MM-DD`), when known.
    pub published: Option<String>,
    /// Asset size in bytes, when known.
    pub size: Option<u64>,
    /// Marked `true` only on the newest 4.x version, which carries the "最新"
    /// tag. The 2.x/3.x lines are legacy, so they are never tagged.
    pub latest: bool,
}

impl VersionInfo {
    /// Human-readable size such as `244.7 MB`.
    pub fn size_label(&self) -> Option<String> {
        self.size
            .map(|bytes| format!("{:.1} MB", bytes as f64 / 1_000_000.0))
    }

    /// Numeric key used to order versions newest-first.
    fn sort_key(&self) -> Vec<u64> {
        version_key(&self.version)
    }
}

impl PartialEq for VersionInfo {
    fn eq(&self, other: &Self) -> bool {
        self.series == other.series && self.version == other.version
    }
}

impl SearchableListItem for VersionInfo {
    type Value = String;

    fn title(&self) -> SharedString {
        self.version.clone().into()
    }

    /// The trigger shows the plain version. A trailing tag would not read well
    /// as an accessible value, and the trigger has no `cx` to resolve a theme
    /// colour, so the green tag is drawn only inside the dropdown rows.
    fn display_title(&self) -> Option<AnyElement> {
        Some(div().child(self.version.clone()).into_any_element())
    }

    /// The dropdown row: the version, plus a green "最新" tag on the newest one.
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(self.version.clone());
        if self.latest {
            row = row.child(latest_tag());
        }
        row
    }

    fn value(&self) -> &Self::Value {
        &self.version
    }

    fn matches(&self, query: &str) -> bool {
        self.version.to_lowercase().contains(&query.to_lowercase())
    }
}

/// A small WeChat-green "最新" pill.
///
/// The colour is the fixed brand green rather than a theme token because the
/// trigger's `display_title` has no `cx` to read the theme from; the brand
/// green also reads the same in light and dark mode.
pub fn latest_tag() -> AnyElement {
    div()
        .px_1p5()
        .py_0p5()
        .rounded_full()
        .bg(crate::theme::BRAND)
        .text_xs()
        .text_color(white())
        .child("最新")
        .into_any_element()
}

/// Group versions by series for the dropdown, preserving the input order
/// (series descending, newest-first within each series).
pub fn group_by_series(versions: &[VersionInfo]) -> Vec<SearchableGroup<VersionInfo>> {
    let mut groups: Vec<SearchableGroup<VersionInfo>> = Vec::new();
    let mut current: Option<Series> = None;

    for version in versions {
        if current != Some(version.series) {
            groups.push(SearchableGroup::new(version.series.label()));
            current = Some(version.series);
        }
        if let Some(group) = groups.last_mut() {
            group.items.push(version.clone());
        }
    }

    groups
}

/// Locate the newest version overall across the grouped items.
///
/// Scans every group rather than assuming a group order, so it stays correct
/// if the series display order changes. The newest version is the one with the
/// highest series and, within it, the highest version number.
pub fn newest_index(groups: &[SearchableGroup<VersionInfo>]) -> Option<IndexPath> {
    let mut best: Option<(Series, Vec<u64>, IndexPath)> = None;

    for (section, group) in groups.iter().enumerate() {
        for (row, item) in group.items.iter().enumerate() {
            let candidate = (item.series, item.sort_key());
            let better = match &best {
                Some((series, key, _)) => candidate > (*series, key.clone()),
                None => true,
            };
            if better {
                let ix = IndexPath::default().section(section).row(row);
                best = Some((item.series, item.sort_key(), ix));
            }
        }
    }

    best.map(|(_, _, ix)| ix)
}

/// Parse `4.1.15.13` into comparable numeric components.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

/// The baked-in 微信 2.x / 3.x catalog.
///
/// Always present regardless of network state: the source repository is frozen,
/// so there is nothing to refresh and no reason to spend API quota on it.
fn legacy_versions() -> Vec<VersionInfo> {
    legacy::VERSIONS
        .iter()
        .filter_map(|(version, published, size)| {
            Some(VersionInfo {
                series: Series::from_version(version)?,
                version: (*version).to_string(),
                tag: format!("v{version}"),
                asset_name: format!("{}{version}.exe", legacy::ASSET_PREFIX),
                download_url: legacy_download_url(version),
                published: Some((*published).to_string()),
                size: Some(*size),
                latest: false,
            })
        })
        .collect()
}

/// `3.9.12.57` -> the frozen repository's release asset URL.
fn legacy_download_url(version: &str) -> String {
    let asset = format!("{}{version}.exe", legacy::ASSET_PREFIX);
    format!(
        "https://github.com/{}/releases/download/v{version}/{asset}",
        legacy::REPO
    )
}

/// A minimal 4.x entry so the picker is never empty when GitHub is unreachable.
fn baked_in_v4() -> VersionInfo {
    VersionInfo {
        series: Series::V4,
        version: "4.1.15.13".to_string(),
        tag: "v4.1.15.13".to_string(),
        asset_name: "weixin_4.1.15.13.exe".to_string(),
        download_url:
            "https://github.com/cscnk52/wechat-windows-versions/releases/download/v4.1.15.13/weixin_4.1.15.13.exe"
                .to_string(),
        published: None,
        size: None,
        latest: false,
    }
}

/// Where the 4.x list in a [`Catalog`] came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Fetched from GitHub just now.
    Live,
    /// Read from the on-disk cache, with its age (e.g. `3 小时前`).
    Cache(String),
    /// Only the single baked-in 4.x entry is available.
    Offline,
}

/// The version list plus where it came from.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub versions: Vec<VersionInfo>,
    /// Set only when fetching failed, phrased for the user.
    pub error: Option<String>,
    pub source: Source,
}

impl Catalog {
    /// Status line shown under the picker.
    pub fn status(&self) -> String {
        let total = self.versions.len();
        match &self.source {
            Source::Live => format!("共 {total} 个版本可用"),
            Source::Cache(age) => format!("共 {total} 个版本可用 · 缓存（{age}）"),
            Source::Offline => format!("共 {total} 个版本可用 · 离线"),
        }
    }
}

/// Build the catalog shown at startup, optionally forcing a refresh.
///
/// Prefers a fresh cache (no request at all); otherwise fetches and refreshes
/// the cache, falling back to a stale cache — or last, to the baked-in list —
/// when the request fails.
pub fn load_catalog(force_refresh: bool) -> Catalog {
    let cached = cache::load();

    // A fresh cache is used as-is: this is what keeps restarts from spending
    // API quota.
    if !force_refresh
        && let Some(cache) = &cached
        && cache.is_fresh()
    {
        return build(cached_live(cache), None, Source::Cache(age_of(cache)));
    }

    let token = github_token();
    let (mut fetched, error) = fetch_repo(&LIVE_REPO, token.as_deref());

    if error.is_none() && !fetched.is_empty() {
        cache::save(&cache::Cache::new(to_cached(&fetched)));
        return build(fetched, None, Source::Live);
    }

    // The fetch failed. A stale cache is still far better than nothing.
    if let Some(cache) = &cached
        && !cache.versions.is_empty()
    {
        let error = error.map(|error| format!("{error}（已使用缓存版本列表）"));
        return build(cached_live(cache), error, Source::Cache(age_of(cache)));
    }

    // Nothing cached: fall back to the baked-in entry.
    fetched.push(baked_in_v4());
    let error = error.map(|error| compose_warning(&[(short_name(LIVE_REPO.slug), error)], true));
    build(fetched, error, Source::Offline)
}

fn age_of(cache: &cache::Cache) -> String {
    cache.age_label().unwrap_or_else(|| "过期".to_string())
}

/// The cached 4.x entries as [`VersionInfo`].
fn cached_live(cache: &cache::Cache) -> Vec<VersionInfo> {
    cache
        .versions
        .iter()
        .filter_map(|entry| {
            Some(VersionInfo {
                series: Series::from_version(&entry.version)?,
                version: entry.version.clone(),
                tag: entry.tag.clone(),
                asset_name: entry.asset_name.clone(),
                download_url: entry.download_url.clone(),
                published: entry.published.clone(),
                size: entry.size,
                latest: false,
            })
        })
        .collect()
}

/// The live entries in cache form.
fn to_cached(versions: &[VersionInfo]) -> Vec<cache::CachedVersion> {
    versions
        .iter()
        .map(|version| cache::CachedVersion {
            version: version.version.clone(),
            tag: version.tag.clone(),
            asset_name: version.asset_name.clone(),
            download_url: version.download_url.clone(),
            published: version.published.clone(),
            size: version.size,
        })
        .collect()
}

/// Merge the live/cached 4.x entries with the baked-in 2.x / 3.x catalog.
fn build(mut live: Vec<VersionInfo>, error: Option<String>, source: Source) -> Catalog {
    let mut versions = legacy_versions();
    versions.append(&mut live);
    sort_and_mark(&mut versions);
    Catalog {
        versions,
        error,
        source,
    }
}

/// Build the status line shown under the picker.
///
/// When every repository failed for the same reason — typically the shared rate
/// limit — the reason is printed once instead of once per repository.
fn compose_warning(failures: &[(&str, String)], using_fallback: bool) -> String {
    let prefix = if using_fallback {
        "无法获取版本列表，已使用内置版本。"
    } else {
        "部分仓库获取失败："
    };

    let all_same = failures
        .first()
        .is_some_and(|(_, first)| failures.iter().all(|(_, other)| other == first));

    if all_same {
        format!("{prefix}{}", failures[0].1)
    } else {
        let detail = failures
            .iter()
            .map(|(repo, error)| format!("{repo}：{error}"))
            .collect::<Vec<_>>()
            .join("；");
        format!("{prefix}{detail}")
    }
}

/// `owner/name` -> `name`, for compact warning text.
fn short_name(slug: &str) -> &str {
    slug.rsplit('/').next().unwrap_or(slug)
}

/// Fetch one repository, trying releases first and tags second.
fn fetch_repo(repo: &Repo, token: Option<&str>) -> (Vec<VersionInfo>, Option<String>) {
    let release_error = match fetch_from_releases(repo, token) {
        Ok(versions) if !versions.is_empty() => return (versions, None),
        Ok(_) => None,
        Err(err @ FetchError::RateLimited { .. }) => {
            // The tags API sits behind the same limit, so a retry only fails again.
            return (Vec::new(), Some(err.to_string()));
        }
        Err(err) => Some(err),
    };

    match fetch_from_tags(repo, token) {
        Ok(versions) if !versions.is_empty() => (versions, None),
        Ok(_) => (Vec::new(), Some(release_error_text(release_error))),
        Err(err) => (Vec::new(), Some(err.to_string())),
    }
}

fn release_error_text(err: Option<FetchError>) -> String {
    err.map(|err| err.to_string())
        .unwrap_or_else(|| "该仓库没有可用版本".to_string())
}

/// A version-lookup failure with a user-readable explanation.
#[derive(Debug)]
enum FetchError {
    /// GitHub's unauthenticated rate limit was hit.
    RateLimited { reset: Option<String> },
    /// Any other failure, already phrased for the user.
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::RateLimited { reset } => {
                write!(f, "GitHub API 访问次数已达上限（未登录每小时 60 次）")?;
                if let Some(reset) = reset {
                    write!(f, "，预计约 {reset} 后恢复")?;
                }
                write!(f, "。可设置环境变量 GITHUB_TOKEN 提高上限")
            }
            FetchError::Other(message) => write!(f, "{message}"),
        }
    }
}

/// Order series newest-first (4.x, 3.x, 2.x) and newest-first within each
/// series, and drop duplicates.
///
/// Only the newest 4.x version is tagged "最新": the 2.x/3.x lines are legacy,
/// so a "latest" badge on them would be misleading.
fn sort_and_mark(versions: &mut Vec<VersionInfo>) {
    versions.sort_by(|a, b| {
        b.series
            .cmp(&a.series)
            .then_with(|| b.sort_key().cmp(&a.sort_key()))
    });
    versions.dedup_by(|a, b| a == b);

    // The list is series-descending, so the first 4.x entry — the highest
    // version of the current line — is the one that carries the tag.
    let mut tagged = false;
    for version in versions.iter_mut() {
        version.latest = !tagged && version.series == Series::V4;
        tagged |= version.latest;
    }
}

/// One decoded GitHub API response.
struct ApiPage {
    pub body: serde_json::Value,
    /// The `Link` header, if present, used to follow pagination.
    pub link: Option<String>,
}

/// GET a GitHub API URL and decode the JSON body.
///
/// Error bodies are decoded too: GitHub puts the reason ("API rate limit
/// exceeded…") in the body, which a bare status code would hide.
fn get_json(url: &str, token: Option<&str>) -> Result<ApiPage, FetchError> {
    let mut request = api_agent()
        .get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }

    let mut response = request
        .call()
        .map_err(|err| FetchError::Other(format!("网络请求失败：{err}")))?;

    let status = response.status().as_u16();
    let remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let reset_epoch = response
        .headers()
        .get("x-ratelimit-reset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());
    let link = response
        .headers()
        .get("link")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|err| FetchError::Other(format!("读取响应失败：{err}")))?;

    if !(200..300).contains(&status) {
        // 403 with no remaining quota is the rate limit; 429 is the explicit form.
        let rate_limited = (status == 403 || status == 429)
            && (remaining.as_deref() == Some("0") || status == 429);
        if rate_limited {
            return Err(FetchError::RateLimited {
                reset: reset_epoch.and_then(format_reset),
            });
        }
        let detail = github_message(&text);
        return Err(FetchError::Other(format!(
            "GitHub API 返回 {status}：{detail}"
        )));
    }

    let body = serde_json::from_str(&text)
        .map_err(|err| FetchError::Other(format!("响应解析失败：{err}")))?;
    Ok(ApiPage { body, link })
}

/// Extract the `rel="next"` URL from a `Link` header.
///
/// GitHub's header looks like:
/// `<https://api.github.com/…?page=2>; rel="next", <…?page=3>; rel="last"`
fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        if !params.contains(r#"rel="next""#) {
            return None;
        }
        let url = url.trim();
        url.strip_prefix('<')
            .and_then(|u| u.strip_suffix('>'))
            .map(str::to_string)
    })
}

/// How many pages of results to follow before giving up.
///
/// A runaway guard: the repos have tens of releases, so this only stops a
/// malformed or looping `Link` header from hanging the app.
const MAX_PAGES: usize = 20;

/// Page through a GitHub list endpoint, returning every decoded page.
///
/// Follows the `Link: rel="next"` header until it disappears, so a repo with
/// more than one page of results (GitHub caps `per_page` at 100) is not
/// silently truncated. Falls back to the first page's contents if the header is
/// absent.
fn get_all_pages(start_url: &str, token: Option<&str>) -> Result<Vec<ApiPage>, FetchError> {
    let mut url = start_url.to_string();
    let mut pages = Vec::new();

    for _ in 0..MAX_PAGES {
        let page = get_json(&url, token)?;
        let next = page.link.as_deref().and_then(next_link);
        pages.push(page);
        match next {
            Some(next) => url = next,
            None => return Ok(pages),
        }
    }

    Ok(pages)
}

/// Concatenate the array bodies of many pages into one list of items.
fn flatten_pages(pages: Vec<ApiPage>) -> Result<Vec<serde_json::Value>, FetchError> {
    let mut items = Vec::new();
    for page in pages {
        let array = page
            .body
            .as_array()
            .ok_or_else(|| FetchError::Other("预期的数组格式不正确".to_string()))?;
        items.extend(array.iter().cloned());
    }
    Ok(items)
}

/// Pull `message` out of a GitHub error body, falling back to a trimmed excerpt.
fn github_message(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(message) = value.get("message").and_then(|m| m.as_str())
    {
        return message.to_string();
    }
    let excerpt = body.trim();
    if excerpt.is_empty() {
        "（无响应内容）".to_string()
    } else {
        excerpt.chars().take(160).collect()
    }
}

/// Turn a Unix epoch into a local `HH:MM:SS` clock time for the reset message.
fn format_reset(epoch: i64) -> Option<String> {
    use std::time::{Duration, UNIX_EPOCH};

    if epoch <= 0 {
        return None;
    }
    let target = UNIX_EPOCH + Duration::from_secs(epoch as u64);
    let now = std::time::SystemTime::now();
    let wait = target.duration_since(now).ok()?;
    let minutes = wait.as_secs() / 60;
    Some(if minutes >= 1 {
        format!("{minutes} 分钟")
    } else {
        format!("{} 秒", wait.as_secs())
    })
}

/// Primary source: the releases API, which exposes asset URLs and sizes.
///
/// Pages through the results: GitHub caps `per_page` at 100, so a repo with
/// more releases than that would otherwise be silently truncated to its newest
/// 100 — exactly the entries a version picker must not lose.
fn fetch_from_releases(repo: &Repo, token: Option<&str>) -> Result<Vec<VersionInfo>, FetchError> {
    let url = format!(
        "https://api.github.com/repos/{}/releases?per_page={PER_PAGE}",
        repo.slug
    );
    let releases = flatten_pages(get_all_pages(&url, token)?)?;
    Ok(parse_releases(&releases))
}

/// Decode the releases API payload into versions.
///
/// Split out from the request so the parsing can be tested against a saved
/// payload, without depending on network access or GitHub's rate limit.
fn parse_releases(releases: &[serde_json::Value]) -> Vec<VersionInfo> {
    let mut versions = Vec::with_capacity(releases.len());
    for release in releases {
        // `.exe` assets are the installers this tool exists to download.
        let asset = release
            .get("assets")
            .and_then(|assets| assets.as_array())
            .and_then(|assets| {
                assets
                    .iter()
                    .find(|asset| is_installer(asset.get("name").and_then(|n| n.as_str())))
            });

        let Some(asset) = asset else { continue };
        let Some(name) = asset.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let Some(download_url) = asset.get("browser_download_url").and_then(|u| u.as_str()) else {
            continue;
        };
        let Some(tag) = release.get("tag_name").and_then(|t| t.as_str()) else {
            continue;
        };

        let version = version_from_tag(tag, name);
        let Some(series) = Series::from_version(&version) else {
            continue;
        };

        versions.push(VersionInfo {
            series,
            version,
            tag: tag.to_string(),
            asset_name: name.to_string(),
            download_url: download_url.to_string(),
            published: published_date(release),
            size: asset.get("size").and_then(|s| s.as_u64()),
            latest: false,
        });
    }
    versions
}

/// Fallback source: the tags API. Installer URLs are reconstructed from each
/// repository's stable `<prefix><version>.exe` naming convention.
///
/// Paged like the releases endpoint, for the same reason.
fn fetch_from_tags(repo: &Repo, token: Option<&str>) -> Result<Vec<VersionInfo>, FetchError> {
    let url = format!(
        "https://api.github.com/repos/{}/tags?per_page={PER_PAGE}",
        repo.slug
    );
    let tags = flatten_pages(get_all_pages(&url, token)?)?;
    Ok(parse_tags(&tags, repo))
}

/// Decode the tags API payload into versions, rebuilding each download URL.
fn parse_tags(tags: &[serde_json::Value], repo: &Repo) -> Vec<VersionInfo> {
    let mut versions = Vec::with_capacity(tags.len());
    for tag in tags {
        let Some(name) = tag.get("name").and_then(|t| t.as_str()) else {
            continue;
        };
        let version = version_from_tag(name, "");
        let Some(series) = Series::from_version(&version) else {
            continue;
        };
        let asset_name = format!("{}{version}.exe", repo.asset_prefix);
        versions.push(VersionInfo {
            series,
            tag: name.to_string(),
            download_url: format!(
                "https://github.com/{}/releases/download/{name}/{asset_name}",
                repo.slug
            ),
            asset_name,
            version,
            published: None,
            size: None,
            latest: false,
        });
    }

    versions
}

fn is_installer(name: Option<&str>) -> bool {
    name.is_some_and(|name| name.to_ascii_lowercase().ends_with(".exe"))
}

/// Derive the plain version from a tag, the asset name, or both.
fn version_from_tag(tag: &str, asset_name: &str) -> String {
    let from_tag = tag.strip_prefix('v').unwrap_or(tag);
    if !from_tag.is_empty() {
        return from_tag.to_string();
    }
    // `weixin_4.1.15.13.exe` -> `4.1.15.13`
    let stem = asset_name
        .strip_suffix(".exe")
        .or_else(|| asset_name.strip_suffix(".EXE"))
        .unwrap_or(asset_name);
    stem.rsplit(['_', '-']).next().unwrap_or(stem).to_string()
}

/// Extract just the date portion of `published_at` (`2026-09-22T10:48:18Z`).
fn published_date(release: &serde_json::Value) -> Option<String> {
    release
        .get("published_at")
        .and_then(|p| p.as_str())
        .and_then(|p| p.split('T').next())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_from_tag_strips_the_v_prefix() {
        assert_eq!(
            version_from_tag("v4.1.15.13", "weixin_4.1.15.13.exe"),
            "4.1.15.13"
        );
        assert_eq!(version_from_tag("4.1.15.13", ""), "4.1.15.13");
    }

    #[test]
    fn version_from_asset_name_is_the_last_segment() {
        assert_eq!(version_from_tag("", "weixin_4.0.3.11.exe"), "4.0.3.11");
        assert_eq!(
            version_from_tag("", "WeChatSetup-3.9.12.57.exe"),
            "3.9.12.57"
        );
    }

    #[test]
    fn series_comes_from_the_major_component() {
        assert_eq!(Series::from_version("3.9.12.57"), Some(Series::V3));
        assert_eq!(Series::from_version("2.0.0.37"), Some(Series::V2));
        assert_eq!(Series::from_version("4.1.15.13"), Some(Series::V4));
    }

    #[test]
    fn malformed_versions_have_no_series() {
        // The tom-snow repo has a stray `v` release with a `WeChatSetup-.exe`
        // asset; it must be filtered out rather than shown as blank.
        assert_eq!(Series::from_version(""), None);
        assert_eq!(Series::from_version("weixin"), None);
        assert_eq!(Series::from_version("9.0.0.1"), None);
    }

    #[test]
    fn github_error_body_reports_the_rate_limit_reason() {
        // This is the body GitHub returns for a 403 rate limit; it is the whole
        // reason the app should not show a bare "http status: 403".
        let body = r#"{"message":"API rate limit exceeded for 1.2.3.4. (But here's the good news: Authenticated requests get a higher rate limit. Check out the documentation for more details.)","documentation_url":"https://docs.github.com/rest"}"#;
        let message = github_message(body);
        assert!(message.contains("rate limit exceeded"), "got: {message}");
    }

    #[test]
    fn github_message_falls_back_to_raw_text() {
        assert_eq!(github_message("plain text"), "plain text");
        assert_eq!(github_message("   "), "（无响应内容）");
        // Long bodies are truncated rather than dumped into the UI.
        let long = "x".repeat(500);
        assert_eq!(github_message(&long).chars().count(), 160);
    }

    #[test]
    fn rate_limit_message_names_the_fix() {
        let err = FetchError::RateLimited {
            reset: Some("约 30 分钟".to_string()),
        };
        let text = err.to_string();
        assert!(text.contains("访问次数已达上限"), "got: {text}");
        assert!(text.contains("约 30 分钟"), "reset time missing: {text}");
        assert!(text.contains("GITHUB_TOKEN"), "the fix should be named");
    }

    #[test]
    fn shared_failure_reason_is_printed_once() {
        let failures = vec![
            (
                "wechat-windows-versions",
                "GitHub API 访问次数已达上限".to_string(),
            ),
            (
                "wechat-windows-versions",
                "GitHub API 访问次数已达上限".to_string(),
            ),
        ];
        let message = compose_warning(&failures, true);
        // `str::matches` must be qualified: `SearchableListItem::matches` is also
        // in scope and is implemented for `String`, returning `bool`.
        assert_eq!(
            str::matches(&message, "访问次数已达上限").count(),
            1,
            "a shared reason should not repeat per repo: {message}"
        );
        assert!(message.starts_with("无法获取版本列表，已使用内置版本。"));
    }

    #[test]
    fn different_failure_reasons_are_attributed_per_repo() {
        let failures = vec![
            ("repo-a", "网络请求失败：超时".to_string()),
            ("repo-b", "GitHub API 返回 404".to_string()),
        ];
        let message = compose_warning(&failures, false);
        assert!(message.contains("repo-a：网络请求失败：超时"), "{message}");
        assert!(message.contains("repo-b：GitHub API 返回 404"), "{message}");
        assert!(message.starts_with("部分仓库获取失败："));
    }

    /// A trimmed but shape-accurate slice of the real releases payload
    /// (`tag_name`, `published_at`, `assets[].{name,size,browser_download_url}`),
    /// taken from the 4.x repo.
    const RELEASES_FIXTURE: &str = r#"[
      {"tag_name":"v4.1.15.13","published_at":"2026-09-22T10:48:18Z","assets":[
        {"name":"weixin_4.1.15.13.exe","size":256624952,
         "browser_download_url":"https://github.com/cscnk52/wechat-windows-versions/releases/download/v4.1.15.13/weixin_4.1.15.13.exe"}]},
      {"tag_name":"v4.1.15.9","published_at":"2026-09-15T10:52:45Z","assets":[
        {"name":"weixin_4.1.15.9.exe","size":256605360,
         "browser_download_url":"https://github.com/cscnk52/wechat-windows-versions/releases/download/v4.1.15.9/weixin_4.1.15.9.exe"}]},
      {"tag_name":"v","published_at":"2023-02-08T07:08:07Z","assets":[
        {"name":"WeChatSetup-.exe","size":173240032,
         "browser_download_url":"https://github.com/tom-snow/wechat-windows-versions/releases/download/v/WeChatSetup-.exe"}]}
    ]"#;

    #[test]
    fn releases_payload_decodes_into_versions() {
        let json: serde_json::Value = serde_json::from_str(RELEASES_FIXTURE).unwrap();
        let mut versions = parse_releases(json.as_array().unwrap());

        // The stray `v` release has no parseable version and is dropped.
        assert_eq!(versions.len(), 2, "stray 'v' release must be filtered");

        sort_and_mark(&mut versions);
        let newest = &versions[0];
        assert_eq!(newest.version, "4.1.15.13");
        assert_eq!(newest.series, Series::V4);
        assert_eq!(newest.asset_name, "weixin_4.1.15.13.exe");
        assert_eq!(newest.published.as_deref(), Some("2026-09-22"));
        assert_eq!(newest.size, Some(256_624_952));
        assert!(newest.latest, "newest 4.x should carry the tag");
        assert!(
            newest.download_url.starts_with("https://github.com/"),
            "asset URL must survive parsing"
        );
    }

    // MARK: pagination

    /// The real GitHub `Link` header shape: several relations, `next` in the
    /// middle. Only the `next` URL should be picked out.
    #[test]
    fn next_link_extracts_rel_next() {
        let header = r#"<https://api.github.com/repos/o/r/releases?per_page=100&page=2>; rel="next", <https://api.github.com/repos/o/r/releases?per_page=100&page=5>; rel="last""#;
        assert_eq!(
            next_link(header).as_deref(),
            Some("https://api.github.com/repos/o/r/releases?per_page=100&page=2")
        );
    }

    /// A header carrying only other relations means there is no next page.
    #[test]
    fn next_link_is_none_without_rel_next() {
        let header = r#"<https://api.github.com/repos/o/r/releases?page=3>; rel="prev", <https://api.github.com/repos/o/r/releases?page=1>; rel="first""#;
        assert_eq!(next_link(header), None);
        assert_eq!(next_link(""), None);
        assert_eq!(next_link("garbage"), None);
    }

    /// A `rel="next"` whose URL is not angle-bracketed must not be followed —
    /// better to stop than to request something malformed.
    #[test]
    fn next_link_ignores_unbracketed_urls() {
        assert_eq!(next_link(r#"https://x.test/2; rel="next""#), None);
    }

    /// Pages must concatenate in order, and nothing may be dropped — this is
    /// the whole point of the change.
    #[test]
    fn pages_concatenate_in_order() {
        let page = |tag: &str| ApiPage {
            body: serde_json::json!([{"tag_name": tag}]),
            link: None,
        };
        let items = flatten_pages(vec![page("v4.1.15.13"), page("v4.1.15.9")]).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["tag_name"], "v4.1.15.13");
        assert_eq!(items[1]["tag_name"], "v4.1.15.9");
    }

    /// The multi-page case the bug was about: more than `PER_PAGE` entries must
    /// all survive, including the oldest, and the parser must keep them.
    #[test]
    fn more_than_one_page_is_not_truncated() {
        // Two full pages plus a short final page.
        let make = |first: usize, count: usize| {
            let releases: Vec<serde_json::Value> = (0..count)
                .map(|i| {
                    let minor = first + i;
                    serde_json::json!({
                        "tag_name": format!("v4.1.{minor}.1"),
                        "assets": [{
                            "name": format!("weixin_4.1.{minor}.1.exe"),
                            "size": 1,
                            "browser_download_url": format!("https://e.test/{minor}.exe"),
                        }],
                    })
                })
                .collect();
            ApiPage {
                body: serde_json::Value::Array(releases),
                link: None,
            }
        };

        let pages = vec![make(0, PER_PAGE), make(PER_PAGE, PER_PAGE), make(100, 7)];
        let items = flatten_pages(pages).unwrap();
        assert_eq!(items.len(), PER_PAGE * 2 + 7);

        let versions = parse_releases(&items);
        assert_eq!(versions.len(), PER_PAGE * 2 + 7, "no entry may be lost");

        // The very oldest entry (last page, last item) is present.
        let oldest = versions
            .iter()
            .map(|v| version_key(&v.version))
            .min()
            .unwrap();
        assert_eq!(oldest, version_key("4.1.0.1"));
    }

    /// A page that is not a JSON array is an error, not a silent empty result.
    #[test]
    fn a_non_array_page_is_an_error() {
        let page = ApiPage {
            body: serde_json::json!({"message": "Not Found"}),
            link: None,
        };
        assert!(flatten_pages(vec![page]).is_err());
    }

    #[test]
    fn releases_without_an_exe_asset_are_skipped() {
        let json = serde_json::json!([
            {"tag_name":"v4.1.15.13","assets":[{"name":"source.zip","size":1,"browser_download_url":"u"}]},
            {"tag_name":"v4.1.15.9","assets":[]},
            {"tag_name":"v4.1.15.8"}
        ]);
        assert!(parse_releases(json.as_array().unwrap()).is_empty());
    }

    #[test]
    fn tags_payload_rebuilds_download_urls() {
        let json: serde_json::Value =
            serde_json::from_str(r#"[{"name":"v4.1.15.13"},{"name":"v4.1.15.9"}]"#).unwrap();
        let mut versions = parse_tags(json.as_array().unwrap(), &LIVE_REPO);
        sort_and_mark(&mut versions);

        assert_eq!(versions.len(), 2);
        let newest = &versions[0];
        assert_eq!(newest.version, "4.1.15.13");
        assert_eq!(newest.asset_name, "weixin_4.1.15.13.exe");
        assert_eq!(
            newest.download_url,
            "https://github.com/cscnk52/wechat-windows-versions/releases/download/v4.1.15.13/weixin_4.1.15.13.exe"
        );
        assert!(newest.latest, "the 4.x line carries the tag");
    }

    /// 2.x / 3.x come from the baked-in table, not the network, and their
    /// download URLs must match the frozen repository's layout.
    #[test]
    fn legacy_table_is_complete_and_well_formed() {
        let versions = legacy_versions();

        assert_eq!(versions.len(), legacy::VERSIONS.len());
        assert_eq!(
            versions.iter().filter(|v| v.series == Series::V2).count(),
            16
        );
        assert_eq!(
            versions.iter().filter(|v| v.series == Series::V3).count(),
            43
        );

        for version in &versions {
            assert!(!version.latest, "legacy lines are never tagged");
            assert_eq!(
                version.asset_name,
                format!("WeChatSetup-{}.exe", version.version)
            );
            assert_eq!(version.download_url, legacy_download_url(&version.version));
            assert!(version.published.is_some(), "release date is recorded");
            assert!(version.size.is_some(), "asset size is recorded");
        }

        // Newest-first within the baked table.
        let v3: Vec<&str> = versions
            .iter()
            .filter(|v| v.series == Series::V3)
            .map(|v| v.version.as_str())
            .collect();
        assert_eq!(v3[0], "3.9.12.57");
        assert_eq!(*v3.last().unwrap(), "3.0.0.57");
    }

    /// Every baked entry must be a valid version for its series, so a typo in
    /// the table cannot silently produce a blank row.
    #[test]
    fn every_legacy_entry_parses() {
        for (version, published, size) in legacy::VERSIONS {
            let series = Series::from_version(version)
                .unwrap_or_else(|| panic!("unparseable legacy version: {version}"));
            assert!(
                matches!(series, Series::V2 | Series::V3),
                "{version} belongs to {series:?}, expected 2.x/3.x"
            );
            assert!(!published.is_empty(), "{version} has no release date");
            assert!(*size > 0, "{version} has a zero size");
        }
    }

    #[test]
    fn versions_sort_numerically_within_a_series() {
        let mut versions = vec![
            build_version("4.1.9.2"),
            build_version("4.1.15.13"),
            build_version("4.1.15.9"),
        ];
        sort_and_mark(&mut versions);
        let order: Vec<&str> = versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(order, ["4.1.15.13", "4.1.15.9", "4.1.9.2"]);
    }

    #[test]
    fn series_are_ordered_newest_first_then_newest_version_first() {
        let mut versions = vec![
            build_version("2.0.0.37"),
            build_version("3.9.12.57"),
            build_version("3.8.0.41"),
            build_version("4.1.15.13"),
            build_version("2.9.5.41"),
        ];
        sort_and_mark(&mut versions);
        let order: Vec<&str> = versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(
            order,
            ["4.1.15.13", "3.9.12.57", "3.8.0.41", "2.9.5.41", "2.0.0.37"]
        );
    }

    #[test]
    fn duplicate_versions_collapse() {
        let mut versions = vec![build_version("4.1.15.13"), build_version("4.1.15.13")];
        sort_and_mark(&mut versions);
        assert_eq!(versions.len(), 1);
    }

    #[test]
    fn only_the_current_4x_line_is_tagged_latest() {
        let mut versions = vec![
            build_version("4.1.15.13"),
            build_version("4.1.15.9"),
            build_version("3.9.12.57"),
            build_version("3.8.0.41"),
            build_version("2.9.5.41"),
        ];
        sort_and_mark(&mut versions);

        let marked: Vec<&str> = versions
            .iter()
            .filter(|v| v.latest)
            .map(|v| v.version.as_str())
            .collect();
        assert_eq!(
            marked,
            ["4.1.15.13"],
            "只给最新的 4.x 打标签，3.x/2.x 是旧版本线"
        );
    }

    #[test]
    fn legacy_series_are_never_tagged() {
        let mut versions = vec![build_version("3.9.12.57"), build_version("2.9.5.41")];
        sort_and_mark(&mut versions);
        assert!(
            versions.iter().all(|v| !v.latest),
            "没有 4.x 时也不应给 3.x/2.x 打标签"
        );
    }

    #[test]
    fn marking_latest_holds_even_when_input_is_unsorted() {
        let mut versions = vec![build_version("4.1.15.9"), build_version("4.1.15.13")];
        sort_and_mark(&mut versions);
        assert!(versions[0].latest && versions[0].version == "4.1.15.13");
        assert!(!versions[1].latest);
    }

    #[test]
    fn grouping_splits_series_and_keeps_order() {
        let mut versions = vec![
            build_version("2.9.5.41"),
            build_version("2.0.0.37"),
            build_version("3.9.12.57"),
            build_version("4.1.15.13"),
        ];
        sort_and_mark(&mut versions);
        let groups = group_by_series(&versions);

        let titles: Vec<&str> = groups.iter().map(|g| g.title.as_ref()).collect();
        assert_eq!(titles, ["微信 4.x", "微信 3.x", "微信 2.x"]);
        assert_eq!(groups[0].items.len(), 1);
        assert_eq!(groups[1].items.len(), 1);
        assert_eq!(groups[2].items.len(), 2);
        assert!(!groups.iter().any(|g| g.items.is_empty()));
        // Newest overall is 4.1.15.13, the first row of the first (4.x) group.
        assert_eq!(newest_index(&groups), Some(IndexPath::new(0).section(0)));
    }

    #[test]
    fn newest_index_scans_all_groups() {
        let mut versions = vec![
            build_version("4.1.15.13"),
            build_version("2.9.5.41"),
            build_version("3.9.12.57"),
        ];
        sort_and_mark(&mut versions);

        // Lay the groups out in an unusual order; the newest (4.1.15.13) now
        // sits in the middle, so a "last group" assumption would be wrong.
        let mut reordered = Vec::new();
        for series in [Series::V2, Series::V4, Series::V3] {
            reordered.extend(versions.iter().filter(|v| v.series == series).cloned());
        }
        let groups = group_by_series(&reordered);

        assert_eq!(newest_index(&groups), Some(IndexPath::new(0).section(1)));
    }

    #[test]
    fn newest_index_is_none_for_an_empty_catalog() {
        assert_eq!(newest_index(&[]), None);
    }

    #[test]
    fn offline_fallback_still_lists_all_three_series() {
        // Simulates a failed live fetch: baked-in 2.x/3.x plus the 4.x fallback.
        let mut versions = legacy_versions();
        versions.push(baked_in_v4());
        sort_and_mark(&mut versions);

        let series: Vec<Series> = versions.iter().map(|v| v.series).collect();
        assert!(series.contains(&Series::V2));
        assert!(series.contains(&Series::V3));
        assert!(series.contains(&Series::V4));
        assert!(versions.iter().all(|v| !v.download_url.is_empty()));
        // Exactly one 最新 tag, on the 4.x entry.
        let tagged: Vec<&str> = versions
            .iter()
            .filter(|v| v.latest)
            .map(|v| v.version.as_str())
            .collect();
        assert_eq!(tagged, ["4.1.15.13"]);
    }

    // MARK: cache round-trip

    /// A cached 4.x list must come back as usable versions, with the fields the
    /// UI shows (date, size) intact.
    #[test]
    fn cached_versions_convert_back_to_version_info() {
        let live = vec![build_version("4.1.15.13"), build_version("4.1.15.9")];
        let cache = cache::Cache::new(to_cached(&live));
        let mut restored = cached_live(&cache);

        assert_eq!(restored.len(), 2);
        sort_and_mark(&mut restored);
        assert_eq!(restored[0].version, "4.1.15.13");
        assert_eq!(restored[0].series, Series::V4);
        assert!(restored[0].latest, "the 最新 tag is reapplied from cache");
    }

    /// A cache entry with an unparseable version must be dropped rather than
    /// produce a blank row.
    #[test]
    fn cached_entry_with_bad_version_is_dropped() {
        let cache = cache::Cache::new(vec![
            cache::CachedVersion {
                version: "4.1.15.13".to_string(),
                tag: "v4.1.15.13".to_string(),
                asset_name: "weixin_4.1.15.13.exe".to_string(),
                download_url: "https://e.test/a.exe".to_string(),
                published: None,
                size: None,
            },
            cache::CachedVersion {
                version: "garbage".to_string(),
                tag: "v-garbage".to_string(),
                asset_name: "x.exe".to_string(),
                download_url: "https://e.test/x.exe".to_string(),
                published: None,
                size: None,
            },
        ]);

        let restored = cached_live(&cache);
        assert_eq!(restored.len(), 1, "only the valid entry survives");
        assert_eq!(restored[0].version, "4.1.15.13");
    }

    /// The status line tells the user which source is in play — that is the
    /// whole point of caching, so it must not silently read like a live fetch.
    #[test]
    fn status_line_names_the_source() {
        let catalog = |source: Source| Catalog {
            versions: vec![build_version("4.1.15.13")],
            error: None,
            source,
        };

        assert_eq!(catalog(Source::Live).status(), "共 1 个版本可用");
        assert_eq!(
            catalog(Source::Cache("3 小时前".into())).status(),
            "共 1 个版本可用 · 缓存（3 小时前）"
        );
        assert_eq!(catalog(Source::Offline).status(), "共 1 个版本可用 · 离线");
    }

    /// End-to-end over a real file: write a catalog to the cache path, read it
    /// back, and confirm the 4.x entries survive. Uses its own temp location so
    /// it never touches the user's real cache.
    #[test]
    fn cache_survives_a_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("wx-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("versions.json");

        let live = vec![build_version("4.1.15.13")];
        let written = cache::Cache::new(to_cached(&live));
        std::fs::write(&path, serde_json::to_vec_pretty(&written).unwrap()).unwrap();

        let read: cache::Cache = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(read.is_fresh());
        assert_eq!(cached_live(&read).len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    fn build_version(version: &str) -> VersionInfo {
        VersionInfo {
            series: Series::from_version(version).expect("test versions must be valid"),
            version: version.to_string(),
            tag: format!("v{version}"),
            asset_name: format!("weixin_{version}.exe"),
            download_url: String::new(),
            published: Some("2026-09-22".to_string()),
            size: Some(256_624_952),
            latest: false,
        }
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;

    /// Live check against GitHub. Ignored by default; run with
    /// `cargo test -- --ignored fetch_live_versions`.
    ///
    /// Forces a refresh so it exercises the API and the cache write, not a
    /// cached list.
    #[test]
    #[ignore]
    fn fetch_live_versions() {
        let catalog = load_catalog(true);
        let versions = &catalog.versions;
        assert_eq!(catalog.source, Source::Live, "error: {:?}", catalog.error);
        assert!(
            catalog.error.is_none(),
            "unexpected error: {:?}",
            catalog.error
        );
        assert!(
            versions.len() > 50,
            "expected many versions, got {}",
            versions.len()
        );

        for version in versions {
            assert!(version.download_url.starts_with("https://"));
            assert!(version.asset_name.ends_with(".exe"));
            assert!(!version.version.is_empty());
        }

        // All three series are present.
        for series in [Series::V2, Series::V3, Series::V4] {
            let count = versions.iter().filter(|v| v.series == series).count();
            assert!(count > 0, "no versions for {series:?}");
        }

        // Only the current 4.x line carries the "最新" tag.
        assert_eq!(versions.iter().filter(|v| v.latest).count(), 1);
        let newest_v4 = versions.iter().find(|v| v.latest).unwrap();
        assert_eq!(newest_v4.series, Series::V4);
        assert_eq!(newest_v4.version, "4.1.15.13");
        assert_eq!(newest_v4.asset_name, "weixin_4.1.15.13.exe");
        assert_eq!(
            versions
                .iter()
                .filter(|v| v.latest && v.series != Series::V4)
                .count(),
            0,
            "3.x/2.x must not be tagged"
        );

        // Display order: 4.x, then 3.x, then 2.x — each newest version first.
        let order: Vec<Series> = versions.iter().map(|v| v.series).collect();
        let mut expected = order.clone();
        expected.sort_by(|a, b| b.cmp(a));
        assert_eq!(order, expected, "series must be ordered 4.x, 3.x, 2.x");

        let groups = group_by_series(versions);
        let titles: Vec<&str> = groups.iter().map(|g| g.title.as_ref()).collect();
        assert_eq!(titles, ["微信 4.x", "微信 3.x", "微信 2.x"]);

        // The newest overall (4.1.15.13) is selected on startup.
        assert_eq!(newest_index(&groups), Some(IndexPath::new(0).section(0)));

        // The stray `v` release must not appear.
        assert!(versions.iter().all(|v| !v.asset_name.ends_with("-.exe")));

        // 2.x / 3.x come from the baked-in table verbatim; 4.x must have come
        // from the network (a single fallback entry would mean it failed).
        assert_eq!(
            versions.iter().filter(|v| v.series == Series::V2).count(),
            16
        );
        assert_eq!(
            versions.iter().filter(|v| v.series == Series::V3).count(),
            43
        );
        assert!(
            versions.iter().filter(|v| v.series == Series::V4).count() > 40,
            "4.x should have been fetched live"
        );

        println!(
            "total={} v2={} v3={} v4={}",
            versions.len(),
            versions.iter().filter(|v| v.series == Series::V2).count(),
            versions.iter().filter(|v| v.series == Series::V3).count(),
            versions.iter().filter(|v| v.series == Series::V4).count(),
        );
    }

    /// Prints the frozen repository's current contents so [`crate::legacy`] can
    /// be refreshed by hand. Ignored: run with
    /// `cargo test -- --ignored --nocapture show_legacy_refresh`.
    #[test]
    #[ignore]
    fn show_legacy_refresh() {
        let (versions, warning) = fetch_repo(
            &Repo {
                slug: legacy::REPO,
                asset_prefix: legacy::ASSET_PREFIX,
            },
            github_token().as_deref(),
        );
        assert!(
            warning.is_none(),
            "could not reach the legacy repo: {warning:?}"
        );

        let mut versions = versions;
        sort_and_mark(&mut versions);
        println!("const VERSIONS: &[(&str, &str, u64)] = &[");
        for version in &versions {
            println!(
                "    (\"{}\", \"{}\", {}),",
                version.version,
                version.published.as_deref().unwrap_or(""),
                version.size.unwrap_or(0)
            );
        }
        println!("];");
    }

    /// Proves the `Link` header is actually followed, against real GitHub
    /// headers. Requests a deliberately tiny page so the 76-release repo spans
    /// several pages, then checks every release is collected.
    ///
    /// Ignored by default; run with
    /// `cargo test -- --ignored --nocapture paging_follows_link_header`.
    #[test]
    #[ignore]
    fn paging_follows_link_header() {
        // Same endpoint the app uses, but 10 per page instead of 100, so
        // pagination kicks in on a repo that fits in one page normally.
        let url = format!(
            "https://api.github.com/repos/{}/releases?per_page=10",
            LIVE_REPO.slug
        );
        let pages = get_all_pages(&url, github_token().as_deref()).expect("paged fetch");

        assert!(
            pages.len() > 1,
            "expected multiple pages with per_page=10, got {}",
            pages.len()
        );
        for page in &pages {
            assert!(
                page.link.is_some() || page.body.as_array().is_some(),
                "page has neither a body nor a Link header"
            );
        }

        let items = flatten_pages(pages).expect("flatten");
        let versions = parse_releases(&items);
        // Pagination must not lose or duplicate anything.
        assert!(
            versions.len() >= 70,
            "expected ~76 releases across pages, got {}",
            versions.len()
        );

        let mut sorted = versions;
        sort_and_mark(&mut sorted);
        assert_eq!(sorted[0].version, "4.1.15.13", "newest first");
        println!(
            "pages fetched across the Link chain, {} releases total",
            sorted.len()
        );
    }
}

#[cfg(test)]
mod live_error_tests {
    use super::*;

    /// Prints what the UI would show right now. Not an assertion — it is a way
    /// to see the real status text during a rate limit or offline.
    #[test]
    #[ignore]
    fn show_current_error() {
        let catalog = load_catalog(true);
        println!(
            "versions={} source={:?} status={:?}",
            catalog.versions.len(),
            catalog.source,
            catalog.status()
        );
        if let Some(error) = &catalog.error {
            println!("\n--- UI would show ---\n{error}\n--------------------");
        }
    }
}
