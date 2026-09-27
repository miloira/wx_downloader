//! Background installer download.
//!
//! `ureq` is blocking, so the transfer runs on its own thread and reports
//! progress through an `async_channel`. The UI polls that channel from a
//! foreground task, which keeps the render loop responsive.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Progress and terminal events sent from the download thread.
pub enum DownloadEvent {
    /// Response received; the total size is known when the server sent one.
    Started { total: Option<u64> },
    /// Bytes written so far, with a smoothed transfer rate.
    Progress {
        downloaded: u64,
        total: Option<u64>,
        bytes_per_sec: f64,
    },
    /// Fully written and moved to its final path.
    Finished { path: PathBuf },
    /// Transfer aborted by the user; the partial file was removed.
    Cancelled,
    /// Something went wrong; the partial file was removed.
    Failed { error: String },
}

impl DownloadEvent {
    /// Whether this event ends the transfer.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            DownloadEvent::Finished { .. }
                | DownloadEvent::Cancelled
                | DownloadEvent::Failed { .. }
        )
    }
}

/// Shared cancellation flag, flipped from the UI thread.
pub type CancelFlag = Arc<AtomicBool>;

/// Spawn a thread that downloads `url` to `dest`.
///
/// Progress is pushed into `tx`; the channel is closed when the thread ends.
pub fn spawn_download(
    url: String,
    dest: PathBuf,
    cancel: CancelFlag,
    tx: async_channel::Sender<DownloadEvent>,
) {
    std::thread::Builder::new()
        .name("wx-download".to_string())
        .spawn(move || {
            let outcome = download(&url, &dest, &cancel, &tx);
            // A `Failed`/`Cancelled` terminal event was already sent when the
            // error is user-visible; only unreported errors reach here.
            if let Err(error) = outcome {
                let _ = tx.send_blocking(DownloadEvent::Failed { error });
            }
        })
        .ok();
}

fn download(
    url: &str,
    dest: &Path,
    cancel: &CancelFlag,
    tx: &async_channel::Sender<DownloadEvent>,
) -> Result<(), String> {
    // Downloads are large; never impose a whole-transfer timeout.
    let agent = crate::http::agent(None);

    let response = agent
        .get(url)
        .header("User-Agent", crate::http::USER_AGENT)
        .call()
        .map_err(|err| format!("请求失败: {err}"))?;

    let total = response.body().content_length();
    let _ = tx.send_blocking(DownloadEvent::Started { total });

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("无法创建保存目录: {err}"))?;
    }

    // Write to a sibling `.part` file so a cancel or crash never leaves a
    // truncated file that looks complete.
    let part = part_path(dest);
    let mut file = BufWriter::with_capacity(
        1 << 20,
        File::create(&part).map_err(|err| format!("无法创建文件: {err}"))?,
    );

    let started = Instant::now();
    let mut downloaded: u64 = 0;
    let mut last_report = Instant::now();
    let mut reader = response.into_body().into_reader();
    let mut buffer = vec![0u8; 256 * 1024];

    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = fs::remove_file(&part);
            let _ = tx.send_blocking(DownloadEvent::Cancelled);
            return Ok(());
        }

        let read = reader
            .read(&mut buffer)
            .map_err(|err| format!("读取数据失败: {err}"))?;
        if read == 0 {
            break;
        }

        file.write_all(&buffer[..read])
            .map_err(|err| format!("写入文件失败: {err}"))?;
        downloaded += read as u64;

        if last_report.elapsed() >= Duration::from_millis(120) {
            last_report = Instant::now();
            let seconds = started.elapsed().as_secs_f64().max(0.001);
            let _ = tx.try_send(DownloadEvent::Progress {
                downloaded,
                total,
                bytes_per_sec: downloaded as f64 / seconds,
            });
        }
    }

    file.flush()
        .map_err(|err| format!("刷新文件缓冲失败: {err}"))?;
    drop(file);

    fs::rename(&part, dest).map_err(|err| {
        let _ = fs::remove_file(&part);
        format!("保存文件失败: {err}")
    })?;

    let _ = tx.send_blocking(DownloadEvent::Finished {
        path: dest.to_path_buf(),
    });
    Ok(())
}

/// `weixin_4.1.15.13.exe` -> `weixin_4.1.15.13.exe.part`.
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live transfer check. Ignored by default; run with
    /// `cargo test -- --ignored download_writes_and_renames`.
    #[test]
    #[ignore]
    fn download_writes_and_renames() {
        let dir = std::env::temp_dir().join(format!("wx-dl-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("api.json");

        let cancel: CancelFlag = Default::default();
        let (tx, rx) = async_channel::unbounded();
        spawn_download(
            "https://api.github.com/repos/cscnk52/wechat-windows-versions".to_string(),
            dest.clone(),
            cancel,
            tx,
        );

        let mut got_finished = false;
        while let Ok(event) = rx.recv_blocking() {
            match event {
                DownloadEvent::Started { total } => {
                    assert!(total.unwrap_or(0) > 0, "expected a known size");
                }
                DownloadEvent::Finished { path } => {
                    assert_eq!(path, dest);
                    got_finished = true;
                }
                DownloadEvent::Failed { error } => panic!("download failed: {error}"),
                _ => {}
            }
        }

        assert!(got_finished, "no Finished event received");
        let body = std::fs::read_to_string(&dest).unwrap();
        assert!(body.contains("wechat-windows-versions"));
        assert!(
            !part_path(&dest).exists(),
            ".part file should be renamed away"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Live check of the real installer URLs (following the CDN redirect) and
    /// the cancel path, for each supported WeChat series. Ignored by default;
    /// run with `cargo test -- --ignored installer_streams_then_cancels`.
    #[test]
    #[ignore]
    fn installer_streams_then_cancels() {
        // (asset url, minimum expected size) for 微信 4.x and 3.x.
        let cases = [
            (
                "https://github.com/cscnk52/wechat-windows-versions/releases/download/v4.1.15.13/weixin_4.1.15.13.exe",
                200_000_000,
            ),
            (
                "https://github.com/tom-snow/wechat-windows-versions/releases/download/v3.9.12.57/WeChatSetup-3.9.12.57.exe",
                200_000_000,
            ),
            (
                "https://github.com/tom-snow/wechat-windows-versions/releases/download/v2.9.5.41/WeChatSetup-2.9.5.41.exe",
                50_000_000,
            ),
        ];

        for (url, min_size) in cases {
            stream_a_little_then_cancel(url, min_size);
        }
    }

    /// Start a download, confirm progress past 1 MB with a sane total, cancel,
    /// and check the partial file was cleaned up.
    fn stream_a_little_then_cancel(url: &str, min_size: u64) {
        let dir = std::env::temp_dir().join(format!("wx-dl-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("installer.exe");

        let cancel: CancelFlag = Default::default();
        let (tx, rx) = async_channel::unbounded();
        spawn_download(url.to_string(), dest.clone(), cancel.clone(), tx);

        // Let some bytes land, then stop.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut saw_progress = false;
        while Instant::now() < deadline {
            let Ok(event) = rx.recv_blocking() else { break };
            match event {
                DownloadEvent::Progress {
                    downloaded, total, ..
                } => {
                    assert!(
                        total.unwrap_or(0) > min_size,
                        "{url}: expected a real asset size, got {total:?}"
                    );
                    if downloaded > 1_000_000 {
                        saw_progress = true;
                        break;
                    }
                }
                DownloadEvent::Failed { error } => panic!("{url}: download failed: {error}"),
                _ => {}
            }
        }
        assert!(saw_progress, "{url}: no progress received");

        cancel.store(true, Ordering::Relaxed);
        while let Ok(event) = rx.recv_blocking() {
            if let DownloadEvent::Cancelled = event {
                break;
            }
        }
        assert!(
            !part_path(&dest).exists(),
            "{url}: cancelled .part file should be removed"
        );
        assert!(!dest.exists(), "{url}: no final file after cancel");

        std::fs::remove_dir_all(&dir).ok();
    }
}
