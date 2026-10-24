//! Fetch missing poster artwork into a local cache directory (never into the media library).
//!
//! mediad lists every item; the ones with no artwork and no cache entry get a lookup:
//! movies on iTunes and shows on TVmaze (no keys; TMDB first when a key is given), games in the libretro thumbnail repository
//! (no key). A hit is stored as `<cache>/<item id>.<ext>`, a miss as an empty
//! `<cache>/<item id>.miss` so it is not retried for a week. mediad picks the cache up on its
//! next scan, which `run` asks for when anything new arrived.
//!
//! Network access goes through `curl` (a request's URL and credentials are handed over on
//! stdin, so keys never show up in the process list).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use serde_json::Value;

const RETRY_MISSES_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);

pub struct Config {
    pub mediad: String,
    pub cache: PathBuf,
    /// TMDB v3 API key or v4 read token; `None` skips movies and shows.
    pub tmdb_key: Option<String>,
    /// Stop after this many lookups per run; 0 means no limit.
    pub max_lookups: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub year: Option<String>,
    pub system: Option<String>,
    pub has_art: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub wanted: usize,
    pub fetched: usize,
    pub missed: usize,
    pub failed: usize,
}

// ---- pure helpers -------------------------------------------------------------------------

/// `"The Way Home (2023)"` -> (`"The Way Home"`, `Some("2023")`): folder names carry the year to
/// tell remakes apart, but no source lists a title that way.
pub fn split_year(title: &str) -> (String, Option<String>) {
    let t = title.trim();
    if let Some(open) = t.rfind(" (")
        && let Some(y) = t[open + 2..].strip_suffix(')')
        && y.len() == 4
        && y.chars().all(|c| c.is_ascii_digit())
    {
        return (t[..open].trim().to_string(), Some(y.to_string()));
    }
    (t.to_string(), None)
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The libretro thumbnail folder for one of our system ids.
pub fn libretro_dir(system: &str) -> Option<&'static str> {
    Some(match system {
        "nes" => "Nintendo - Nintendo Entertainment System",
        "snes" => "Nintendo - Super Nintendo Entertainment System",
        "genesis" => "Sega - Mega Drive - Genesis",
        "sms" => "Sega - Master System - Mark III",
        "gamegear" => "Sega - Game Gear",
        "gb" => "Nintendo - Game Boy",
        "gbc" => "Nintendo - Game Boy Color",
        "gba" => "Nintendo - Game Boy Advance",
        "n64" => "Nintendo - Nintendo 64",
        _ => return None,
    })
}

/// File names (`*.png`) listed on a libretro directory index page.
pub fn parse_listing(html: &str) -> Vec<String> {
    html.split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(percent_decode)
        .filter(|n| n.to_ascii_lowercase().ends_with(".png"))
        .collect()
}

/// The listing entry that matches `title`: same letters and digits once region and dump tags are
/// dropped. Among several regions, prefer USA, then World, then Europe, then the shortest name.
pub fn best_thumbnail(listing: &[String], title: &str) -> Option<String> {
    let want = catalog::title_key(title);
    if want.is_empty() {
        return None;
    }
    let rank = |n: &str| {
        let r = ["(USA", "(World", "(Europe"]
            .iter()
            .position(|t| n.contains(t))
            .unwrap_or(3);
        (r, n.len())
    };
    listing
        .iter()
        .filter(|n| catalog::title_key(n.trim_end_matches(".png")) == want)
        .min_by_key(|n| rank(n))
        .cloned()
}

/// `poster_path` of the first search result that has one.
pub fn tmdb_poster(body: &Value) -> Option<String> {
    body["results"]
        .as_array()?
        .iter()
        .find_map(|r| r["poster_path"].as_str().map(str::to_string))
}

/// Letters-and-digits key for comparing titles across sources ("Alien: Earth" == "Alien - Earth").
fn same_title(a: &str, b: &str) -> bool {
    let (a, b) = (catalog::title_key(a), catalog::title_key(b));
    !a.is_empty() && a == b
}

/// A movie poster from the iTunes Search API (no key): the first hit with the same title whose
/// year is the folder's year, give or take one (release dates differ by region).
pub fn itunes_poster(body: &Value, title: &str, year: Option<&str>) -> Option<String> {
    let year: Option<i32> = year.and_then(|y| y.parse().ok());
    body["results"].as_array()?.iter().find_map(|r| {
        let name = r["trackName"].as_str()?;
        let ry: Option<i32> = r["releaseDate"].as_str().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok());
        let year_ok = match (year, ry) {
            (Some(a), Some(b)) => (a - b).abs() <= 1,
            _ => true,
        };
        (same_title(name, title) && year_ok)
            .then(|| r["artworkUrl100"].as_str().map(|u| u.replace("100x100bb", "600x900bb")))?
    })
}

/// A show poster from TVmaze (no key): the first search hit with exactly the same title.
pub fn tvmaze_poster(body: &Value, title: &str) -> Option<String> {
    body.as_array()?.iter().find_map(|r| {
        let show = &r["show"];
        if !same_title(show["name"].as_str()?, title) {
            return None;
        }
        show["image"]["original"].as_str().map(str::to_string)
    })
}

/// The lead image of the best-matching Wikipedia article (a film's is normally its poster): the
/// first search hit whose title, minus a "(1979 film)"-style suffix, equals ours.
pub fn wikipedia_poster(body: &Value, title: &str) -> Option<String> {
    let mut pages: Vec<&Value> = body["query"]["pages"].as_object()?.values().collect();
    pages.sort_by_key(|p| p["index"].as_i64().unwrap_or(i64::MAX));
    pages.into_iter().find_map(|p| {
        let name = p["title"].as_str()?;
        let base = name.split(" (").next().unwrap_or(name);
        if !same_title(base, title) {
            return None;
        }
        p["thumbnail"]["source"].as_str().map(str::to_string)
    })
}

fn cache_hit(cache: &Path, id: &str) -> bool {
    ["jpg", "png", "webp"]
        .iter()
        .any(|e| cache.join(format!("{id}.{e}")).exists())
}

fn recent_miss(cache: &Path, id: &str) -> bool {
    std::fs::metadata(cache.join(format!("{id}.miss")))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < RETRY_MISSES_AFTER)
}

// ---- mediad -------------------------------------------------------------------------------

fn http(addr: &str, method: &str, path: &str) -> Result<(u16, Vec<u8>), String> {
    let mut s = TcpStream::connect(addr).map_err(|e| format!("cannot reach mediad at {addr}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(120))).ok();
    write!(s, "{method} {path} HTTP/1.0\r\nHost: {addr}\r\nContent-Length: 0\r\n\r\n").map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("malformed reply")?;
    let status = String::from_utf8_lossy(&raw[..split])
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or("malformed status line")?;
    Ok((status, raw[split + 4..].to_vec()))
}

/// Every item in the library, as mediad lists them (the "everything" row).
pub fn list_items(addr: &str) -> Result<Vec<Item>, String> {
    let (status, body) = http(addr, "GET", "/rows/everything")?;
    if status != 200 {
        return Err(format!("/rows/everything: HTTP {status}"));
    }
    let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(v["items"]
        .as_array()
        .ok_or("no items")?
        .iter()
        .map(|i| {
            let (title, title_year) = split_year(i["title"].as_str().unwrap_or_default());
            Item {
            id: i["id"].as_str().unwrap_or_default().to_string(),
            kind: i["kind"].as_str().unwrap_or_default().to_string(),
            title,
            year: title_year.or_else(|| i["date"].as_str().and_then(|d| d.get(..4)).map(str::to_string)),
            system: i["system"].as_str().map(str::to_string),
            has_art: !i["art"].is_null(),
            }
        })
        .collect())
}

pub fn notify_rescan(addr: &str) -> Result<(), String> {
    match http(addr, "POST", "/rescan")? {
        (200, _) => Ok(()),
        (s, _) => Err(format!("mediad answered HTTP {s}")),
    }
}

// ---- network ------------------------------------------------------------------------------

/// GET `url` with curl. `bearer` becomes an Authorization header. Request details go on stdin.
fn fetch(url: &str, bearer: Option<&str>) -> Result<Vec<u8>, String> {
    let mut cfg = format!("url = \"{url}\"\n");
    if let Some(t) = bearer {
        cfg.push_str(&format!("header = \"Authorization: Bearer {t}\"\n"));
    }
    let mut child = Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "--location", "--max-time", "30", "--user-agent", "media-appliance-artsync/0.1 (self-hosted home media box)", "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    child.stdin.take().expect("piped").write_all(cfg.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Minimum gap between requests to a host: iTunes allows roughly 20 a minute and TVmaze 20 per
/// 10 s per address; going over gets 403/429 answers, so pace ourselves instead of failing.
fn min_gap(url: &str) -> Duration {
    if url.contains("itunes.apple.com") {
        Duration::from_millis(3200)
    } else if url.contains("tvmaze.com") {
        Duration::from_millis(600)
    } else if url.contains("wikipedia.org") || url.contains("wikimedia.org") {
        Duration::from_millis(1000)
    } else {
        Duration::ZERO
    }
}

/// `fetch`, paced per host, retrying once after a minute when the host says "slow down".
fn fetch_polite(url: &str, bearer: Option<&str>) -> Result<Vec<u8>, String> {
    static LAST: std::sync::Mutex<Option<std::collections::HashMap<&'static str, std::time::Instant>>> =
        std::sync::Mutex::new(None);
    let host: &'static str = ["itunes.apple.com", "tvmaze.com", "wikipedia.org", "wikimedia.org"].into_iter().find(|h| url.contains(h)).unwrap_or("other");
    let pace = |gap: Duration| {
        let mut g = LAST.lock().unwrap();
        let map = g.get_or_insert_with(Default::default);
        if let Some(t) = map.get(host)
            && let Some(wait) = gap.checked_sub(t.elapsed())
        {
            std::thread::sleep(wait);
        }
        map.insert(host, std::time::Instant::now());
    };
    static BLOCKED: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    if BLOCKED.lock().unwrap().contains(&host) {
        return Err(format!("{host} refused us earlier in this run; skipped"));
    }
    pace(min_gap(url));
    match fetch(url, bearer) {
        Err(e) if e.contains("403") || e.contains("429") => {
            tracing::info!(host, "rate limited; waiting a minute");
            std::thread::sleep(Duration::from_secs(60));
            pace(min_gap(url));
            let retried = fetch(url, bearer);
            if retried.is_err() {
                // Still refused: stop asking this host for the rest of the run.
                BLOCKED.lock().unwrap().push(host);
            }
            retried
        }
        other => other,
    }
}

/// A v4 read token is a long JWT; a v3 key is 32 hex characters passed as a query parameter.
fn tmdb_get(key: &str, path_and_query: &str) -> Result<Value, String> {
    let (bearer, extra) = if key.len() > 40 { (Some(key), String::new()) } else { (None, format!("&api_key={key}")) };
    let body = fetch(&format!("https://api.themoviedb.org/3/{path_and_query}{extra}"), bearer)?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

enum Found {
    Image(&'static str, Vec<u8>),
    Nothing,
}

fn lookup_tmdb(key: &str, item: &Item) -> Result<Found, String> {
    let (kind, year_param) = match item.kind.as_str() {
        "movie" => ("movie", "year"),
        _ => ("tv", "first_air_date_year"),
    };
    let q = percent_encode(&item.title);
    let mut poster = None;
    // Movies carry a release year in their folder name; try with it, then without.
    if item.kind == "movie"
        && let Some(y) = &item.year
    {
        poster = tmdb_poster(&tmdb_get(key, &format!("search/{kind}?query={q}&{year_param}={y}"))?);
    }
    if poster.is_none() {
        poster = tmdb_poster(&tmdb_get(key, &format!("search/{kind}?query={q}"))?);
    }
    match poster {
        Some(p) => Ok(Found::Image("jpg", fetch(&format!("https://image.tmdb.org/t/p/w500{p}"), None)?)),
        None => Ok(Found::Nothing),
    }
}

fn get_json(url: &str) -> Result<Value, String> {
    serde_json::from_slice(&fetch_polite(url, None)?).map_err(|e| e.to_string())
}

fn image(url: &str, _ext: &'static str) -> Result<Found, String> {
    let bytes = fetch_polite(url, None)?;
    let ext = if bytes.starts_with(b"\x89PNG") { "png" } else if bytes.starts_with(b"RIFF") { "webp" } else { "jpg" };
    Ok(Found::Image(ext, bytes))
}

fn lookup_itunes(item: &Item) -> Result<Found, String> {
    let body = get_json(&format!(
        "https://itunes.apple.com/search?media=movie&entity=movie&limit=15&country=us&term={}",
        percent_encode(&item.title)
    ))?;
    match itunes_poster(&body, &item.title, item.year.as_deref()) {
        Some(u) => image(&u, "jpg"),
        None => Ok(Found::Nothing),
    }
}

fn lookup_tvmaze(item: &Item) -> Result<Found, String> {
    let body = get_json(&format!("https://api.tvmaze.com/search/shows?q={}", percent_encode(&item.title)))?;
    match tvmaze_poster(&body, &item.title) {
        Some(u) => image(&u, "jpg"),
        None => Ok(Found::Nothing),
    }
}

fn lookup_wikipedia(item: &Item) -> Result<Found, String> {
    let q = match (item.kind.as_str(), &item.year) {
        ("movie", Some(y)) => format!("{} {y} film", item.title),
        ("movie", None) => format!("{} film", item.title),
        _ => format!("{} TV series", item.title),
    };
    let body = get_json(&format!(
        "https://en.wikipedia.org/w/api.php?action=query&format=json&generator=search&gsrlimit=5&gsrsearch={}&prop=pageimages&piprop=thumbnail&pithumbsize=500&pilicense=any",
        percent_encode(&q)
    ))?;
    match wikipedia_poster(&body, &item.title) {
        Some(u) => image(&u, "jpg"),
        None => Ok(Found::Nothing),
    }
}

fn lookup_libretro(listings: &mut std::collections::HashMap<String, Vec<String>>, item: &Item) -> Result<Found, String> {
    let Some(dir) = item.system.as_deref().and_then(libretro_dir) else { return Ok(Found::Nothing) };
    if !listings.contains_key(dir) {
        let page = fetch(&format!("https://thumbnails.libretro.com/{}/Named_Boxarts/", percent_encode(dir)), None)?;
        listings.insert(dir.to_string(), parse_listing(&String::from_utf8_lossy(&page)));
    }
    let Some(name) = best_thumbnail(&listings[dir], &item.title) else { return Ok(Found::Nothing) };
    let img = fetch(
        &format!("https://thumbnails.libretro.com/{}/Named_Boxarts/{}", percent_encode(dir), percent_encode(&name)),
        None,
    )?;
    Ok(Found::Image("png", img))
}

// ---- the run ------------------------------------------------------------------------------

/// Try each source for the item's kind in turn: the first image wins; a failing source does not stop the chain.
fn lookup(cfg: &Config, listings: &mut std::collections::HashMap<String, Vec<String>>, item: &Item) -> Result<Found, String> {
    let mut sources: Vec<Box<dyn FnMut() -> Result<Found, String> + '_>> = Vec::new();
    match item.kind.as_str() {
        "movie" => {
            if let Some(k) = &cfg.tmdb_key { sources.push(Box::new(move || lookup_tmdb(k, item))); }
            sources.push(Box::new(|| lookup_itunes(item)));
            sources.push(Box::new(|| lookup_wikipedia(item)));
        }
        "show" => {
            if let Some(k) = &cfg.tmdb_key { sources.push(Box::new(move || lookup_tmdb(k, item))); }
            sources.push(Box::new(|| lookup_tvmaze(item)));
            sources.push(Box::new(|| lookup_wikipedia(item)));
        }
        _ => sources.push(Box::new(|| lookup_libretro(listings, item))),
    }
    let mut last_err = None;
    for mut s in sources {
        match s() {
            found @ Ok(Found::Image(..)) => return found,
            Err(e) => {
                tracing::warn!(id = %item.id, title = %item.title, error = %e, "source failed; trying the next");
                last_err = Some(e);
            }
            Ok(Found::Nothing) => {}
        }
    }
    last_err.map_or(Ok(Found::Nothing), Err)
}

pub fn run(cfg: &Config, items: &[Item]) -> Summary {
    let mut sum = Summary::default();
    let items: Vec<&Item> = items.iter().filter(|i| !i.has_art).collect();
    let _ = std::fs::create_dir_all(&cfg.cache);
    let mut listings = std::collections::HashMap::new();
    let mut lookups = 0;
    for item in items {
        if item.kind == "channel" || cache_hit(&cfg.cache, &item.id) || recent_miss(&cfg.cache, &item.id) {
            continue;
        }
        sum.wanted += 1;
        if cfg.max_lookups > 0 && lookups >= cfg.max_lookups {
            break;
        }
        lookups += 1;
        let found = lookup(cfg, &mut listings, item);
        match found {
            Ok(Found::Image(ext, bytes)) if !bytes.is_empty() => {
                let tmp = cfg.cache.join(format!(".{}.part", item.id));
                if std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, cfg.cache.join(format!("{}.{ext}", item.id)))).is_ok() {
                    tracing::info!(id = %item.id, title = %item.title, kind = %item.kind, "artwork cached");
                    sum.fetched += 1;
                } else {
                    sum.failed += 1;
                }
            }
            Ok(_) => {
                tracing::info!(id = %item.id, title = %item.title, kind = %item.kind, "no artwork found");
                let _ = std::fs::write(cfg.cache.join(format!("{}.miss", item.id)), b"");
                sum.missed += 1;
            }
            Err(e) => {
                tracing::warn!(id = %item.id, title = %item.title, error = %e, "artwork lookup failed");
                sum.failed += 1;
            }
        }
    }
    sum
}

/// `(season, number) -> image URL` from TVmaze's episode list of a show.
pub fn tvmaze_episode_images(body: &Value) -> std::collections::HashMap<(i64, i64), String> {
    body.as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some((
                (e["season"].as_i64()?, e["number"].as_i64()?),
                e["image"]["original"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// Episodes of a library show as mediad lists them: `(episode id, season, number, has thumbnail)`.
fn show_episodes(addr: &str, id: &str) -> Result<Vec<(String, i64, i64, bool)>, String> {
    let (status, body) = http(addr, "GET", &format!("/items/{id}"))?;
    if status != 200 {
        return Err(format!("/items/{id}: HTTP {status}"));
    }
    let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(v["episodes"]
        .as_array()
        .ok_or("no episodes")?
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap_or_default().to_string(),
                e["season"].as_i64().unwrap_or(0),
                e["episode"].as_i64().unwrap_or(0),
                e["has_art"].as_bool().unwrap_or(false),
            )
        })
        .collect())
}

/// Episode thumbnails for TV shows, from TVmaze, cached as `<episode id>.jpg`. A show is only
/// looked up when some episode still lacks one, and a show TVmaze does not know is not retried
/// for a week. Returns how many thumbnails were stored.
pub fn run_episodes(cfg: &Config, shows: &[Item]) -> usize {
    let mut stored = 0;
    for show in shows.iter().filter(|s| s.kind == "show") {
        let marker = format!("{}.eps-miss", show.id);
        let missing: Vec<_> = match show_episodes(&cfg.mediad, &show.id) {
            Ok(eps) => eps.into_iter().filter(|e| !e.3 && !cache_hit(&cfg.cache, &e.0)).collect(),
            Err(e) => {
                tracing::warn!(show = %show.title, error = %e, "cannot list episodes");
                continue;
            }
        };
        if missing.is_empty() {
            continue;
        }
        if std::fs::metadata(cfg.cache.join(&marker)).and_then(|m| m.modified()).ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age < RETRY_MISSES_AFTER)
        {
            continue;
        }
        let found = (|| -> Result<Option<std::collections::HashMap<(i64, i64), String>>, String> {
            let hits = get_json(&format!("https://api.tvmaze.com/search/shows?q={}", percent_encode(&show.title)))?;
            let Some(tv_id) = hits.as_array().into_iter().flatten().find_map(|r| {
                same_title(r["show"]["name"].as_str()?, &show.title).then(|| r["show"]["id"].as_i64())?
            }) else {
                return Ok(None);
            };
            Ok(Some(tvmaze_episode_images(&get_json(&format!("https://api.tvmaze.com/shows/{tv_id}/episodes"))?)))
        })();
        let images = match found {
            Ok(Some(m)) => m,
            Ok(None) => {
                tracing::info!(show = %show.title, "show not found on TVmaze; no episode thumbnails");
                let _ = std::fs::write(cfg.cache.join(&marker), b"");
                continue;
            }
            Err(e) => {
                tracing::warn!(show = %show.title, error = %e, "episode lookup failed");
                continue;
            }
        };
        let mut got = 0;
        for (id, season, number, _) in missing {
            let Some(url) = images.get(&(season, number)) else { continue };
            match image(url, "jpg") {
                Ok(Found::Image(ext, bytes)) if !bytes.is_empty() => {
                    let tmp = cfg.cache.join(format!(".{id}.part"));
                    if std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, cfg.cache.join(format!("{id}.{ext}")))).is_ok() {
                        got += 1;
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(show = %show.title, error = %e, "episode thumbnail failed"),
            }
        }
        tracing::info!(show = %show.title, thumbnails = got, "episode thumbnails cached");
        stored += got;
    }
    stored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_years_are_split_off_titles() {
        assert_eq!(split_year("The Way Home (2023)"), ("The Way Home".into(), Some("2023".into())));
        assert_eq!(split_year("Alien - Earth"), ("Alien - Earth".into(), None));
        assert_eq!(split_year("Show (Part 2)"), ("Show (Part 2)".into(), None));
    }

    #[test]
    fn percent_round_trip() {
        let s = "Alien³ & Co (1992)/סרט";
        assert_eq!(percent_decode(&percent_encode(s)), s);
        assert_eq!(percent_encode("a b"), "a%20b");
    }

    #[test]
    fn libretro_listing_and_matching() {
        let html = r#"<a href="../">..</a><a href="Aladdin%20(Europe).png">x</a><a href="Aladdin%20(USA).png">x</a>
            <a href="Aladdin%20II%20(USA).png">x</a><a href="readme.txt">x</a>"#;
        let l = parse_listing(html);
        assert_eq!(l, ["Aladdin (Europe).png", "Aladdin (USA).png", "Aladdin II (USA).png"]);
        assert_eq!(best_thumbnail(&l, "Aladdin").as_deref(), Some("Aladdin (USA).png"));
        assert_eq!(best_thumbnail(&l, "aladdin ii").as_deref(), Some("Aladdin II (USA).png"));
        assert_eq!(best_thumbnail(&l, "Zelda"), None);
    }

    #[test]
    fn tmdb_takes_the_first_poster() {
        let v: Value = serde_json::json!({"results":[{"poster_path":null},{"poster_path":"/a.jpg"},{"poster_path":"/b.jpg"}]});
        assert_eq!(tmdb_poster(&v).as_deref(), Some("/a.jpg"));
        assert_eq!(tmdb_poster(&serde_json::json!({"results":[]})), None);
    }

    #[test]
    fn itunes_and_tvmaze_match_titles_exactly() {
        let it = serde_json::json!({"results":[
            {"trackName":"Alien: Romulus","releaseDate":"2024-08-16T07:00:00Z","artworkUrl100":"https://x/a/100x100bb.jpg"},
            {"trackName":"Alien","releaseDate":"1979-05-25T07:00:00Z","artworkUrl100":"https://x/b/100x100bb.jpg"}]});
        assert_eq!(itunes_poster(&it, "Alien", Some("1979")).as_deref(), Some("https://x/b/600x900bb.jpg"));
        assert_eq!(itunes_poster(&it, "Alien", Some("2010")), None);
        let tv = serde_json::json!([{"show":{"name":"Alien: Earth","image":{"original":"https://t/o.jpg"}}},{"show":{"name":"Alien","image":null}}]);
        assert_eq!(tvmaze_poster(&tv, "Alien - Earth").as_deref(), Some("https://t/o.jpg"));
        assert_eq!(tvmaze_poster(&tv, "Alien Planet"), None);
    }

    #[test]
    fn wikipedia_strips_the_disambiguation_suffix() {
        let v = serde_json::json!({"query":{"pages":{
            "2":{"title":"Alien (franchise)","index":2,"thumbnail":{"source":"https://w/f.jpg"}},
            "1":{"title":"Alien (film)","index":1,"thumbnail":{"source":"https://w/p.jpg"}}}}});
        assert_eq!(wikipedia_poster(&v, "Alien").as_deref(), Some("https://w/p.jpg"));
        assert_eq!(wikipedia_poster(&v, "Aliens"), None);
    }

    #[test]
    fn cached_and_recently_missed_items_are_not_looked_up_again() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), b"x").unwrap();
        std::fs::write(dir.path().join("b.miss"), b"").unwrap();
        let cfg = Config { mediad: String::new(), cache: dir.path().into(), tmdb_key: None, max_lookups: 0 };
        let item = |id: &str, kind: &str| Item { id: id.into(), kind: kind.into(), title: "T".into(), year: None, system: None, has_art: false };
        let sum = run(&cfg, &[item("a", "game"), item("b", "game"), item("c", "game")]);
        assert_eq!(sum.wanted, 1);
    }
}
