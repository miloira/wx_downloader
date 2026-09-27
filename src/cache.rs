//! On-disk cache for the fetched version list.
//!
//! GitHub's API allows only 60 unauthenticated requests per hour per IP, and on
//! a shared/NAT address that budget can be gone before the app starts. So the
//! live 4.x list is cached to disk:
//!
//! * While the cache is fresh, it is used directly and **no request is made** —
//!   restarting the app no longer costs quota.
//! * When it goes stale, the API is tried, and a success refreshes the cache.
//! * If the API fails, the stale cache still beats showing nothing, so it is
//!   used and the failure reported.
//!
//! The 2.x / 3.x data is not cached: it is baked into [`crate::legacy`].

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How long a cached list is considered current.
///
/// WeChat 4.x ships every few weeks, so a few hours of staleness is harmless,
/// while the reduced request rate keeps the app clear of the rate limit.
pub const MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);

/// Directory name under `%LOCALAPPDATA%`.
const APP_DIR: &str = "wx_downloader";
const CACHE_FILE: &str = "versions.json";

/// One cached entry. Deliberately its own type rather than the in-memory
/// `VersionInfo`, so the on-disk format does not shift when that struct does.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedVersion {
    pub version: String,
    pub tag: String,
    pub asset_name: String,
    pub download_url: String,
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// The cached catalog with the time it was fetched.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cache {
    /// Unix seconds at which the list was fetched.
    pub fetched_at: u64,
    pub versions: Vec<CachedVersion>,
}

impl Cache {
    /// Build a cache record dated now.
    pub fn new(versions: Vec<CachedVersion>) -> Self {
        Self {
            fetched_at: now_unix(),
            versions,
        }
    }

    /// Time since the list was fetched.
    ///
    /// `None` when the timestamp is in the future — a clock adjustment or a
    /// hand-edited file. Such a cache is neither fresh nor labelable.
    pub fn age(&self) -> Option<Duration> {
        let now = now_unix();
        now.checked_sub(self.fetched_at).map(Duration::from_secs)
    }

    /// Whether the cache is young enough to use without re-requesting.
    pub fn is_fresh(&self) -> bool {
        self.age().is_some_and(|age| age < MAX_AGE)
    }

    /// Human-readable age, e.g. `3 小时前`.
    pub fn age_label(&self) -> Option<String> {
        let seconds = self.age()?.as_secs();
        Some(if seconds < 60 {
            "刚刚".to_string()
        } else if seconds < 3600 {
            format!("{} 分钟前", seconds / 60)
        } else if seconds < 86_400 {
            format!("{} 小时前", seconds / 3600)
        } else {
            format!("{} 天前", seconds / 86_400)
        })
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Where the cache file lives: `%LOCALAPPDATA%\wx_downloader\versions.json`.
///
/// `None` when the environment gives us nowhere to write, in which case caching
/// is simply skipped.
pub fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)?;
    Some(base.join(APP_DIR).join(CACHE_FILE))
}

/// Read the cache, if a valid one exists.
///
/// A corrupt or unreadable file counts as "no cache": a bad cache file must not
/// stop the app from starting.
pub fn load() -> Option<Cache> {
    let path = cache_path()?;
    let data = std::fs::read(&path).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Write the cache, best-effort.
///
/// Written to a sibling `.tmp` first and renamed, so an interrupted write
/// cannot leave a half-file that reads back as corrupt.
pub fn save(cache: &Cache) {
    let Some(path) = cache_path() else { return };
    let Some(dir) = path.parent() else { return };

    if std::fs::create_dir_all(dir).is_err() {
        return;
    }

    let Ok(json) = serde_json::to_vec_pretty(cache) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CachedVersion {
        CachedVersion {
            version: "4.1.15.13".to_string(),
            tag: "v4.1.15.13".to_string(),
            asset_name: "weixin_4.1.15.13.exe".to_string(),
            download_url: "https://example.test/weixin_4.1.15.13.exe".to_string(),
            published: Some("2026-09-22".to_string()),
            size: Some(256_624_952),
        }
    }

    fn aged(seconds: u64) -> Cache {
        Cache {
            fetched_at: now_unix() - seconds,
            versions: vec![sample()],
        }
    }

    #[test]
    fn round_trips_through_json() {
        let cache = Cache::new(vec![sample()]);
        let json = serde_json::to_vec(&cache).unwrap();
        let back: Cache = serde_json::from_slice(&json).unwrap();

        assert_eq!(back.fetched_at, cache.fetched_at);
        assert_eq!(back.versions.len(), 1);
        assert_eq!(back.versions[0].version, "4.1.15.13");
        assert_eq!(back.versions[0].size, Some(256_624_952));
        assert_eq!(back.versions[0].published.as_deref(), Some("2026-09-22"));
    }

    #[test]
    fn a_just_written_cache_is_fresh() {
        let cache = Cache::new(vec![sample()]);
        assert!(cache.is_fresh());
        assert_eq!(cache.age_label().as_deref(), Some("刚刚"));
    }

    #[test]
    fn an_old_cache_is_stale_but_still_usable() {
        let cache = aged(MAX_AGE.as_secs() + 60);
        assert!(!cache.is_fresh(), "older than the TTL");
        // Still readable and labelled — that is what lets the app fall back to
        // it instead of showing nothing.
        assert_eq!(cache.age_label().as_deref(), Some("6 小时前"));
    }

    #[test]
    fn age_label_scales_with_the_gap() {
        for (seconds, expected) in [
            (30, "刚刚"),
            (5 * 60, "5 分钟前"),
            (3 * 3600, "3 小时前"),
            (2 * 86_400, "2 天前"),
        ] {
            assert_eq!(
                aged(seconds).age_label().as_deref(),
                Some(expected),
                "{seconds}s"
            );
        }
    }

    #[test]
    fn a_timestamp_in_the_future_is_neither_fresh_nor_labelled() {
        let cache = Cache {
            fetched_at: now_unix() + 10_000,
            versions: vec![sample()],
        };
        assert_eq!(cache.age_label(), None);
        assert!(
            !cache.is_fresh(),
            "a bogus timestamp must not read as fresh"
        );
    }

    #[test]
    fn missing_optional_fields_still_load() {
        // Compatibility: a cache written before `size`/`published` existed.
        let json = r#"{"fetched_at":1,"versions":[
            {"version":"4.1.15.13","tag":"v4.1.15.13",
             "asset_name":"weixin_4.1.15.13.exe","download_url":"https://e.test/x.exe"}]}"#;
        let cache: Cache = serde_json::from_str(json).unwrap();
        assert_eq!(cache.versions[0].size, None);
        assert_eq!(cache.versions[0].published, None);
    }

    #[test]
    fn corrupt_json_is_rejected_rather_than_panicking() {
        assert!(serde_json::from_slice::<Cache>(b"not json").is_err());
    }
}
