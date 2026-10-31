//! Periodic system health lines in the journal (and so in VictoriaLogs): CPU use, load, memory,
//! CPU clock, temperature and kernel pressure. They are what explains a stuttering game after the
//! fact. Sampled every `MEDIAD_STATS_SECS` (30), and every `MEDIAD_STATS_BUSY_SECS` (2) while a
//! player (RetroArch or mpv) runs; 0 turns the sampler off.

use std::fs;
use std::time::Duration;

/// Processes that mean something is playing, matched against `/proc/<pid>/comm`.
const PLAYERS: [&str; 2] = ["retroarch", "mpv"];

/// Cumulative CPU jiffies from the first line of `/proc/stat`: (busy, total).
fn parse_cpu_times(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().next()?;
    let v: Vec<u64> = line.strip_prefix("cpu")?.split_whitespace().filter_map(|f| f.parse().ok()).collect();
    if v.len() < 5 {
        return None;
    }
    // user nice system idle iowait irq softirq steal: idle and iowait are not busy.
    let total: u64 = v.iter().take(8).sum();
    let idle = v[3] + v[4];
    Some((total - idle, total))
}

/// Utilisation in percent between two samples.
fn cpu_percent(prev: (u64, u64), now: (u64, u64)) -> f64 {
    let total = now.1.saturating_sub(prev.1);
    if total == 0 {
        return 0.0;
    }
    100.0 * now.0.saturating_sub(prev.0) as f64 / total as f64
}

/// (total, available) memory in MiB from `/proc/meminfo`.
fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let kb = |key: &str| -> Option<u64> {
        text.lines().find_map(|l| l.strip_prefix(key)?.trim().trim_end_matches("kB").trim().parse().ok())
    };
    Some((kb("MemTotal:")? / 1024, kb("MemAvailable:")? / 1024))
}

/// The `some avg10=` figure of a `/proc/pressure/*` file: percent of the last 10 s in which at
/// least one task was stalled waiting for the resource.
fn parse_psi_avg10(text: &str) -> Option<f64> {
    text.lines().find(|l| l.starts_with("some"))?.split_whitespace().find_map(|f| f.strip_prefix("avg10=")?.parse().ok())
}

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// Mean of the numeric contents of the files matching `dir/<prefix>N/<leaf>`, or the max.
fn sysfs_values(dir: &str, prefix: &str, leaf: &str) -> Vec<f64> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .filter_map(|e| read(&format!("{}/{leaf}", e.path().display())))
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

/// The player process running now, if any.
fn running_player() -> Option<&'static str> {
    let entries = fs::read_dir("/proc").ok()?;
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Some(comm) = read(&format!("{}/comm", e.path().display())) else { continue };
        if let Some(p) = PLAYERS.iter().find(|p| comm.trim() == **p) {
            return Some(p);
        }
    }
    None
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Log one line and return the sample's CPU totals and whether a player was running.
fn sample(prev: Option<(u64, u64)>) -> (Option<(u64, u64)>, bool) {
    let times = read("/proc/stat").as_deref().and_then(parse_cpu_times);
    let cpu = match (prev, times) {
        (Some(p), Some(n)) => Some(round1(cpu_percent(p, n))),
        _ => None,
    };
    let load: Vec<f64> = read("/proc/loadavg")
        .map(|s| s.split_whitespace().take(3).filter_map(|f| f.parse().ok()).collect())
        .unwrap_or_default();
    let mem = read("/proc/meminfo").as_deref().and_then(parse_meminfo);
    let mhz = sysfs_values("/sys/devices/system/cpu", "cpu", "cpufreq/scaling_cur_freq");
    let temps = sysfs_values("/sys/class/thermal", "thermal_zone", "temp");
    let psi = |name: &str| read(&format!("/proc/pressure/{name}")).as_deref().and_then(parse_psi_avg10);
    let player = running_player();

    tracing::info!(
        cpu_percent = cpu,
        load1 = load.first().copied(),
        load5 = load.get(1).copied(),
        load15 = load.get(2).copied(),
        mem_total_mb = mem.map(|m| m.0),
        mem_available_mb = mem.map(|m| m.1),
        mem_used_percent = mem.map(|(t, a)| round1(100.0 * t.saturating_sub(a) as f64 / t.max(1) as f64)),
        cpu_mhz = (!mhz.is_empty()).then(|| (mhz.iter().sum::<f64>() / mhz.len() as f64 / 1000.0).round()),
        temp_c = temps.iter().copied().reduce(f64::max).map(|t| round1(t / 1000.0)),
        cpu_pressure = psi("cpu"),
        memory_pressure = psi("memory"),
        io_pressure = psi("io"),
        player,
        "system stats"
    );
    (times, player.is_some())
}

/// Run the sampler until the process exits.
pub async fn run(idle: Duration, busy: Duration) {
    let mut prev = None;
    let mut interval = idle;
    loop {
        tokio::time::sleep(interval).await;
        match tokio::task::spawn_blocking(move || sample(prev)).await {
            Ok((times, playing)) => {
                prev = times;
                interval = if playing { busy } else { idle };
            }
            Err(e) => tracing::error!(error = %e, "system stats sample panicked"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_use_is_the_busy_share_between_two_samples() {
        let a = parse_cpu_times("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3 4 5 6 7 8 9 10\n").unwrap();
        assert_eq!(a, (150, 1000));
        let b = parse_cpu_times("cpu  200 0 100 850 50 0 0 0 0 0\n").unwrap();
        assert_eq!(b, (300, 1200));
        // 150 busy of 200 elapsed jiffies.
        assert_eq!(cpu_percent(a, b), 75.0);
        assert_eq!(cpu_percent(a, a), 0.0);
        assert_eq!(parse_cpu_times("intr 1 2 3"), None);
    }

    #[test]
    fn memory_is_total_and_available_in_mib() {
        let m = "MemTotal:       16384000 kB\nMemFree:         1000 kB\nMemAvailable:    8192000 kB\n";
        assert_eq!(parse_meminfo(m), Some((16000, 8000)));
        assert_eq!(parse_meminfo("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn pressure_reads_the_ten_second_average() {
        let p = "some avg10=1.25 avg60=0.50 avg300=0.10 total=12345\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
        assert_eq!(parse_psi_avg10(p), Some(1.25));
        assert_eq!(parse_psi_avg10(""), None);
    }
}
