use std::path::Path;

use sha2::{Digest, Sha256};

/// A stable id from a path relative to the media root, so ids survive restarts
/// and rescans and watch state stays attached to the file.
pub fn make(prefix: &str, media_root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(media_root).unwrap_or(path);
    let rel: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let digest = Sha256::digest(rel.join("/").as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("{prefix}-{hex}")
}
