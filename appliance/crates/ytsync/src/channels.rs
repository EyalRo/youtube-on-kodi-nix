//! The followed-channels file: the same format the Kodi add-on used.
//!
//! One channel per line: a handle (`@name`), a channel URL, a channel id (`UC...`),
//! optionally followed by `| display name`. Plain names cannot be resolved without a
//! search, so they are reported and skipped.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    /// The `/videos` page yt-dlp is pointed at.
    pub url: String,
    /// Used for the folder name when yt-dlp reports none.
    pub name: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub channels: Vec<Channel>,
    pub skipped: Vec<String>,
}

fn is_channel_id(s: &str) -> bool {
    s.len() == 24
        && s.starts_with("UC")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn videos_url(query: &str) -> Option<String> {
    let q = query.trim();
    if let Some(handle) = q.strip_prefix('@') {
        return (!handle.is_empty() && !handle.contains(char::is_whitespace))
            .then(|| format!("https://www.youtube.com/@{handle}/videos"));
    }
    if is_channel_id(q) {
        return Some(format!("https://www.youtube.com/channel/{q}/videos"));
    }
    if q.starts_with("http://") || q.starts_with("https://") {
        let base = q
            .split(['?', '#'])
            .next()
            .unwrap_or(q)
            .trim_end_matches('/');
        let known = [
            "youtube.com/@",
            "youtube.com/channel/",
            "youtube.com/c/",
            "youtube.com/user/",
        ];
        if !known.iter().any(|k| base.contains(k)) {
            return None;
        }
        let base = base.strip_suffix("/videos").unwrap_or(base);
        return Some(format!("{base}/videos"));
    }
    None
}

/// Parse the file's text. Duplicates (same target, compared case-insensitively) keep the first.
pub fn parse(text: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut seen = HashSet::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (query, name) = match line.split_once('|') {
            Some((q, n)) => (
                q.trim(),
                Some(n.trim().to_string()).filter(|n| !n.is_empty()),
            ),
            None => (line, None),
        };
        match videos_url(query) {
            Some(url) => {
                if seen.insert(url.to_lowercase()) {
                    out.channels.push(Channel { url, name });
                }
            }
            None => out.skipped.push(query.to_string()),
        }
    }
    out
}

/// The text with `entry` added (as `entry | name` when a name is given), or `None` when that
/// channel is already followed. Rejects entries `parse` would skip.
pub fn follow(text: &str, entry: &str, name: Option<&str>) -> Result<Option<String>, String> {
    let entry = entry.trim();
    let url = videos_url(entry)
        .ok_or_else(|| format!("{entry:?} is not a @handle, channel URL or UC... id"))?;
    if parse(text).channels.iter().any(|c| c.url.eq_ignore_ascii_case(&url)) {
        return Ok(None);
    }
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => out.push_str(&format!("{entry} | {n}\n")),
        None => out.push_str(&format!("{entry}\n")),
    }
    Ok(Some(out))
}

/// The text without the channel `what` names (a handle, URL or id, or the display name,
/// case-insensitive), or `None` when nothing matched. Comments and other lines are kept.
pub fn unfollow(text: &str, what: &str) -> Option<String> {
    let what = what.trim();
    let want = videos_url(what).map(|u| u.to_lowercase());
    let mut hit = false;
    let kept: Vec<&str> = text
        .lines()
        .filter(|raw| {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                return true;
            }
            let (query, name) = match line.split_once('|') {
                Some((q, n)) => (q.trim(), n.trim()),
                None => (line, ""),
            };
            let same_channel = want.is_some() && videos_url(query).map(|u| u.to_lowercase()) == want;
            let same_name = !name.is_empty() && name.eq_ignore_ascii_case(what);
            let matched = same_channel || same_name;
            hit |= matched;
            !matched
        })
        .collect();
    hit.then(|| kept.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_adds_once_and_unfollow_removes_by_handle_or_name() {
        let text = "# mine\n@Foo | Foo Channel\n";
        assert_eq!(follow(text, "@foo", None), Ok(None));
        let added = follow(text, "@Bar", Some("The Bar")).unwrap().unwrap();
        assert_eq!(added, "# mine\n@Foo | Foo Channel\n@Bar | The Bar\n");
        assert!(follow(text, "Foo Channel", None).is_err());
        assert_eq!(unfollow(&added, "the bar").unwrap(), text);
        assert_eq!(unfollow(&added, "https://www.youtube.com/@FOO/").unwrap(), "# mine\n@Bar | The Bar\n");
        assert_eq!(unfollow(&added, "@nobody"), None);
    }

    #[test]
    fn handles_ids_and_urls() {
        let p = parse(
            "# comment\n\n@Foo | Foo Channel\nUCabcdefghijklmnopqrstuv\nhttps://www.youtube.com/@Bar/\nhttps://www.youtube.com/channel/UCxxxxxxxxxxxxxxxxxxxxxx/videos?x=1\n",
        );
        let urls: Vec<_> = p.channels.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://www.youtube.com/@Foo/videos",
                "https://www.youtube.com/channel/UCabcdefghijklmnopqrstuv/videos",
                "https://www.youtube.com/@Bar/videos",
                "https://www.youtube.com/channel/UCxxxxxxxxxxxxxxxxxxxxxx/videos",
            ]
        );
        assert_eq!(p.channels[0].name.as_deref(), Some("Foo Channel"));
        assert!(p.skipped.is_empty());
    }

    #[test]
    fn duplicates_collapse_case_insensitively() {
        let p = parse("@Foo\n@foo | Again\nhttps://www.youtube.com/@FOO/videos\n");
        assert_eq!(p.channels.len(), 1);
    }

    #[test]
    fn plain_names_and_foreign_urls_are_skipped() {
        let p = parse("Some Channel Name\nhttps://example.com/x\n@\n");
        assert!(p.channels.is_empty());
        assert_eq!(
            p.skipped,
            ["Some Channel Name", "https://example.com/x", "@"]
        );
    }
}
