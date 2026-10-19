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

#[cfg(test)]
mod tests {
    use super::*;

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
