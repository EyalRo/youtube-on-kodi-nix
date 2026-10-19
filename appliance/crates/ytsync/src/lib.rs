//! `ytsync`: download new videos of followed YouTube channels into the library.
//!
//! yt-dlp runs as an external process (never embedded), into a local work directory.
//! Finished videos are then moved to `<media>/youtube/<Channel>/` and mediad is asked to
//! rescan. A video whose delivery fails is removed from the download archive so the next
//! run retries it.

pub mod channels;
pub mod deliver;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub use channels::Channel;

#[derive(Debug, Clone)]
pub struct Config {
    pub media_root: PathBuf,
    /// Archive file and per-run work directories live here (a persisted local path).
    pub state_dir: PathBuf,
    pub ytdlp: PathBuf,
    pub max_per_channel: u32,
    pub max_height: u32,
    pub max_age_days: u32,
    /// 0 means all channels.
    pub max_channels: usize,
    pub mediad: Option<String>,
}

impl Config {
    fn archive(&self) -> PathBuf {
        self.state_dir.join("archive.txt")
    }
}

/// What the downloader is asked to do for one channel.
#[derive(Debug, Clone)]
pub struct Job {
    pub url: String,
    pub work_dir: PathBuf,
    pub archive: PathBuf,
}

pub trait Fetch {
    /// Download into `job.work_dir`. Returns an error only if nothing usable can be assumed.
    fn fetch(&self, job: &Job) -> Result<(), String>;
}

/// The real thing: yt-dlp as a child process.
pub struct YtDlp<'a>(pub &'a Config);

pub fn ytdlp_args(cfg: &Config, job: &Job) -> Vec<String> {
    let h = cfg.max_height;
    let mut a: Vec<String> = [
        "--ignore-errors",
        "--no-warnings",
        "--write-info-json",
        "--write-thumbnail",
        "--convert-thumbnails",
        "jpg",
        "--merge-output-format",
        "mkv",
        "--sleep-requests",
        "1",
        "--match-filters",
        "!is_live & original_url!*=/shorts/",
    ]
    .map(String::from)
    .to_vec();
    a.extend([
        "-f".into(),
        format!("bv*[height<={h}]+ba/b[height<={h}]"),
        "--playlist-end".into(),
        cfg.max_per_channel.to_string(),
        "--dateafter".into(),
        format!("today-{}days", cfg.max_age_days),
        "--download-archive".into(),
        job.archive.display().to_string(),
        "-o".into(),
        job.work_dir
            .join("%(upload_date>%Y-%m-%d)s - %(title).120B [%(id)s].%(ext)s")
            .display()
            .to_string(),
        job.url.clone(),
    ]);
    a
}

impl Fetch for YtDlp<'_> {
    fn fetch(&self, job: &Job) -> Result<(), String> {
        let status = Command::new(&self.0.ytdlp)
            .args(ytdlp_args(self.0, job))
            .status()
            .map_err(|e| format!("cannot run {}: {e}", self.0.ytdlp.display()))?;
        // yt-dlp exits 1 when any single video failed; what finished is still delivered.
        if status.success() {
            Ok(())
        } else {
            Err(format!("yt-dlp exited with {status}"))
        }
    }
}

fn remove_from_archive(archive: &Path, id: &str) {
    let Ok(text) = fs::read_to_string(archive) else {
        return;
    };
    let kept: String = text
        .lines()
        .filter(|l| l.split_whitespace().last() != Some(id))
        .map(|l| format!("{l}\n"))
        .collect();
    let tmp = archive.with_extension("tmp");
    if fs::write(&tmp, kept)
        .and_then(|_| fs::rename(&tmp, archive))
        .is_err()
    {
        let _ = fs::remove_file(&tmp);
        tracing::error!(
            id,
            "could not update the download archive; the video will not be retried"
        );
    }
}

/// Ask mediad to rescan: a bare HTTP/1.1 POST, since the daemon is on localhost.
pub fn notify_rescan(addr: &str) -> Result<(), String> {
    let mut s =
        TcpStream::connect(addr).map_err(|e| format!("cannot reach mediad at {addr}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(120))).ok();
    s.set_write_timeout(Some(Duration::from_secs(10))).ok();
    write!(
        s,
        "POST /rescan HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut reply = String::new();
    s.read_to_string(&mut reply).map_err(|e| e.to_string())?;
    let status = reply.lines().next().unwrap_or_default();
    if status.split_whitespace().nth(1) == Some("200") {
        Ok(())
    } else {
        Err(format!("mediad answered: {status}"))
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub channels: usize,
    pub delivered: usize,
    pub failed_channels: usize,
    pub failed_videos: usize,
}

/// One full pass over the followed channels.
pub fn run(cfg: &Config, channels: &[Channel], fetcher: &dyn Fetch) -> Summary {
    let mut summary = Summary::default();
    let youtube_root = cfg.media_root.join("youtube");
    if let Err(e) = fs::create_dir_all(&cfg.state_dir) {
        tracing::error!(dir = %cfg.state_dir.display(), error = %e, "cannot create state directory");
        summary.failed_channels = channels.len();
        return summary;
    }
    // An unmounted share would make every delivery land on the local disk underneath it.
    if !cfg.media_root.is_dir() {
        tracing::error!(media_root = %cfg.media_root.display(), "media root is missing; is /media mounted?");
        summary.failed_channels = channels.len();
        return summary;
    }
    let limit = if cfg.max_channels == 0 {
        channels.len()
    } else {
        cfg.max_channels.min(channels.len())
    };
    for channel in &channels[..limit] {
        summary.channels += 1;
        let work_dir = cfg.state_dir.join("work");
        let _ = fs::remove_dir_all(&work_dir);
        if let Err(e) = fs::create_dir_all(&work_dir) {
            tracing::error!(error = %e, "cannot create work directory");
            summary.failed_channels += 1;
            continue;
        }
        let job = Job {
            url: channel.url.clone(),
            work_dir: work_dir.clone(),
            archive: cfg.archive(),
        };
        let fetched = fetcher.fetch(&job);
        let outcome = deliver::deliver(&work_dir, &youtube_root, channel.name.as_deref());
        for d in &outcome.delivered {
            tracing::info!(channel = %channel.url, id = %d.id, dest = %d.dest.display(), "video delivered");
        }
        for f in &outcome.failed {
            tracing::warn!(channel = %channel.url, id = ?f.id, reason = %f.reason, "video not delivered; will retry next run");
            if let Some(id) = &f.id {
                remove_from_archive(&job.archive, id);
            }
        }
        summary.delivered += outcome.delivered.len();
        summary.failed_videos += outcome.failed.len();
        if let Err(e) = fetched {
            if outcome.delivered.is_empty() {
                tracing::error!(channel = %channel.url, error = %e, "channel sync failed");
                summary.failed_channels += 1;
            } else {
                tracing::warn!(channel = %channel.url, error = %e, "some videos failed; the rest were delivered");
            }
        }
        let _ = fs::remove_dir_all(&work_dir);
    }
    tracing::info!(
        channels = summary.channels,
        delivered = summary.delivered,
        failed_channels = summary.failed_channels,
        failed_videos = summary.failed_videos,
        "ytsync run complete"
    );
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::net::TcpListener;

    /// Writes one finished video per scripted id into the work directory.
    struct Fake {
        ids: Vec<&'static str>,
        fail: bool,
        seen: RefCell<Vec<String>>,
    }

    impl Fetch for Fake {
        fn fetch(&self, job: &Job) -> Result<(), String> {
            self.seen.borrow_mut().push(job.url.clone());
            for id in &self.ids {
                let stem = format!("2024-05-01 - T [{id}]");
                fs::write(
                    job.work_dir.join(format!("{stem}.info.json")),
                    format!(r#"{{"id":"{id}","channel":"Chan"}}"#),
                )
                .unwrap();
                fs::write(job.work_dir.join(format!("{stem}.mkv")), "v").unwrap();
                let mut a = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&job.archive)
                    .unwrap();
                writeln!(a, "youtube {id}").unwrap();
            }
            if self.fail {
                Err("boom".into())
            } else {
                Ok(())
            }
        }
    }

    fn cfg(media: &Path, state: &Path) -> Config {
        Config {
            media_root: media.to_path_buf(),
            state_dir: state.to_path_buf(),
            ytdlp: "yt-dlp".into(),
            max_per_channel: 5,
            max_height: 1080,
            max_age_days: 30,
            max_channels: 0,
            mediad: None,
        }
    }

    fn chan(n: &str) -> Channel {
        Channel {
            url: format!("https://www.youtube.com/@{n}/videos"),
            name: None,
        }
    }

    #[test]
    fn delivers_and_counts() {
        let (m, s) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let f = Fake {
            ids: vec!["a1", "b2"],
            fail: false,
            seen: RefCell::default(),
        };
        let sum = run(&cfg(m.path(), s.path()), &[chan("x")], &f);
        assert_eq!(
            sum,
            Summary {
                channels: 1,
                delivered: 2,
                failed_channels: 0,
                failed_videos: 0
            }
        );
        assert!(
            m.path()
                .join("youtube/Chan/2024-05-01 - T [a1].mkv")
                .exists()
        );
        assert!(!s.path().join("work").exists());
    }

    #[test]
    fn delivered_videos_are_what_the_scanner_expects() {
        let (m, s) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let f = Fake {
            ids: vec!["a1"],
            fail: false,
            seen: RefCell::default(),
        };
        run(&cfg(m.path(), s.path()), &[chan("x")], &f);
        let scan = scanner::scan_all(m.path(), &scanner::ReleaseDb::empty());
        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].kind, catalog::Kind::Channel);
        assert_eq!(scan.entries[0].title, "Chan");
        assert_eq!(scan.episodes.len(), 1);
    }

    #[test]
    fn partial_failure_with_deliveries_is_not_a_failed_channel() {
        let (m, s) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let f = Fake {
            ids: vec!["a1"],
            fail: true,
            seen: RefCell::default(),
        };
        let sum = run(&cfg(m.path(), s.path()), &[chan("x")], &f);
        assert_eq!((sum.delivered, sum.failed_channels), (1, 0));
    }

    #[test]
    fn failure_with_nothing_delivered_is_a_failed_channel() {
        let (m, s) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let f = Fake {
            ids: vec![],
            fail: true,
            seen: RefCell::default(),
        };
        let sum = run(&cfg(m.path(), s.path()), &[chan("x")], &f);
        assert_eq!((sum.delivered, sum.failed_channels), (0, 1));
    }

    #[test]
    fn failed_delivery_is_removed_from_the_archive_for_retry() {
        let (m, s) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::create_dir_all(m.path().join("youtube")).unwrap();
        fs::write(m.path().join("youtube/Chan"), "a file in the way").unwrap();
        let f = Fake {
            ids: vec!["a1", "b2"],
            fail: false,
            seen: RefCell::default(),
        };
        let sum = run(&cfg(m.path(), s.path()), &[chan("x")], &f);
        assert_eq!((sum.delivered, sum.failed_videos), (0, 2));
        assert_eq!(
            fs::read_to_string(s.path().join("archive.txt"))
                .unwrap()
                .trim(),
            ""
        );
    }

    #[test]
    fn missing_media_root_refuses_to_run() {
        let s = tempfile::tempdir().unwrap();
        let f = Fake {
            ids: vec!["a1"],
            fail: false,
            seen: RefCell::default(),
        };
        let sum = run(
            &cfg(Path::new("/nonexistent/media"), s.path()),
            &[chan("x")],
            &f,
        );
        assert_eq!(sum.failed_channels, 1);
        assert!(f.seen.borrow().is_empty());
    }

    #[test]
    fn max_channels_limits_a_run() {
        let (m, s) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut c = cfg(m.path(), s.path());
        c.max_channels = 2;
        let f = Fake {
            ids: vec![],
            fail: false,
            seen: RefCell::default(),
        };
        let sum = run(&c, &[chan("a"), chan("b"), chan("c")], &f);
        assert_eq!(sum.channels, 2);
    }

    #[test]
    fn ytdlp_args_carry_the_policy() {
        let c = cfg(Path::new("/m"), Path::new("/s"));
        let job = Job {
            url: "U".into(),
            work_dir: "/w".into(),
            archive: "/s/archive.txt".into(),
        };
        let a = ytdlp_args(&c, &job).join(" ");
        assert!(
            a.contains("--playlist-end 5")
                && a.contains("height<=1080")
                && a.contains("today-30days")
        );
        assert!(a.contains("--download-archive /s/archive.txt") && a.ends_with(" U"));
        assert!(a.contains("!is_live"));
    }

    #[test]
    fn notify_posts_rescan() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let t = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = [0u8; 512];
            let n = c.read(&mut buf).unwrap();
            c.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        notify_rescan(&addr).unwrap();
        assert!(t.join().unwrap().starts_with("POST /rescan HTTP/1.1"));
        assert!(notify_rescan("127.0.0.1:1").is_err());
    }
}
