//! HTTP API over the store. Nothing here draws or plays; the UI and the player
//! are separate processes that talk to this one.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use scanner::ReleaseDb;
use serde::Deserialize;
use serde_json::{Value, json};
use store::{Store, StoreError};

#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<Store>>,
    media_root: Arc<PathBuf>,
    release_db: Arc<ReleaseDb>,
    /// Where artsync stores fetched posters (`<item id>.jpg|png|webp`).
    art_cache: Option<Arc<PathBuf>>,
}

impl AppState {
    pub fn new(store: Store, media_root: PathBuf, release_db: ReleaseDb) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            media_root: Arc::new(media_root),
            release_db: Arc::new(release_db),
            art_cache: None,
        }
    }

    pub fn with_art_cache(mut self, dir: PathBuf) -> Self {
        self.art_cache = Some(Arc::new(dir));
        self
    }

    /// Scan the media root and replace the catalog. Blocking: call from `spawn_blocking`.
    pub fn rescan(&self) -> Result<store::ScanStats, StoreError> {
        let mut scan = scanner::scan_all(&self.media_root, &self.release_db);
        if let Some(dir) = &self.art_cache {
            use_art_cache(&mut scan, dir);
        }
        for w in &scan.warnings {
            tracing::warn!(warning = %w, "scan warning");
        }
        let counts = library_counts(&scan);
        let stats = self
            .store
            .lock()
            .expect("store lock poisoned")
            .apply_scan(&scan)?;
        tracing::info!(
            added = stats.added,
            removed = stats.removed,
            entries = stats.entries,
            episodes = stats.episodes,
            "rescan complete"
        );
        tracing::info!(
            movies = counts.movies,
            movies_with_art = counts.movies_with_art,
            shows = counts.shows,
            shows_with_art = counts.shows_with_art,
            show_episodes = counts.show_episodes,
            youtube_channels = counts.channels,
            youtube_channels_with_art = counts.channels_with_art,
            youtube_videos = counts.youtube_videos,
            games = counts.games,
            games_with_art = counts.games_with_art,
            "library detected"
        );
        Ok(stats)
    }
}

/// Entries with no artwork next to their media get the cached poster, if artsync fetched one.
fn use_art_cache(scan: &mut scanner::Scan, dir: &std::path::Path) {
    for e in scan.entries.iter_mut().filter(|e| e.art.is_none()) {
        e.art = ["jpg", "png", "webp"]
            .iter()
            .map(|x| dir.join(format!("{}.{x}", e.id)))
            .find(|p| p.is_file());
    }
    for e in scan.episodes.iter_mut().filter(|e| e.art.is_none()) {
        e.art = ["jpg", "png", "webp"]
            .iter()
            .map(|x| dir.join(format!("{}.{x}", e.id)))
            .find(|p| p.is_file());
    }
}

/// What a scan found, per kind, and how much of it has artwork: logged after every rescan so a
/// library that "scanned fine" but shows nothing (or only placeholders) is visible at a glance.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LibraryCounts {
    pub movies: usize,
    pub movies_with_art: usize,
    pub shows: usize,
    pub shows_with_art: usize,
    pub show_episodes: usize,
    pub channels: usize,
    pub channels_with_art: usize,
    pub youtube_videos: usize,
    pub games: usize,
    pub games_with_art: usize,
}

pub fn library_counts(scan: &scanner::Scan) -> LibraryCounts {
    use catalog::Kind;
    let mut c = LibraryCounts::default();
    let mut channel_ids = std::collections::HashSet::new();
    for e in &scan.entries {
        let art = usize::from(e.art.is_some());
        match e.kind {
            Kind::Movie => { c.movies += 1; c.movies_with_art += art; }
            Kind::Show => { c.shows += 1; c.shows_with_art += art; }
            Kind::Channel => { c.channels += 1; c.channels_with_art += art; channel_ids.insert(e.id.as_str()); }
            Kind::Game => { c.games += 1; c.games_with_art += art; }
        }
    }
    for ep in &scan.episodes {
        if channel_ids.contains(ep.item_id.as_str()) {
            c.youtube_videos += 1;
        } else {
            c.show_episodes += 1;
        }
    }
    c
}

struct ApiError(StatusCode, String);

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(_) => ApiError(StatusCode::NOT_FOUND, e.to_string()),
            StoreError::EmptyScan { .. } => ApiError(StatusCode::CONFLICT, e.to_string()),
            StoreError::Db(_) => {
                tracing::error!(error = %e, "database error");
                ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn not_found(what: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("no such {what}"))
}

/// One log line per request: 5xx is an error, 4xx a warning (a missing item or art is often
/// expected, but a play that 404s is not), everything else debug.
async fn log_requests(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let started = std::time::Instant::now();
    let resp = next.run(req).await;
    let status = resp.status();
    let ms = started.elapsed().as_millis() as u64;
    if status.is_server_error() {
        tracing::error!(%method, %path, status = status.as_u16(), ms, "request failed");
    } else if status.is_client_error() {
        tracing::warn!(%method, %path, status = status.as_u16(), ms, "request rejected");
    } else {
        applog::debug!(%method, %path, status = status.as_u16(), ms, "request");
    }
    resp
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/rows", get(rows))
        .route("/rows/{id}", get(row))
        .route("/items/{id}", get(item))
        .route("/items/{id}/play", get(play))
        .route("/items/{id}/progress", post(progress))
        .route("/items/{id}/played", post(played))
        .route("/items/{id}/art", get(art))
        .route("/episodes/{id}/art", get(episode_art))
        .route("/rescan", post(rescan))
        .layer(axum::middleware::from_fn(log_requests))
        .with_state(state)
}

async fn health(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let entries = s.store.lock().expect("store lock poisoned").count()?;
    Ok(Json(json!({ "status": "ok", "entries": entries })))
}

async fn rows(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let items = s.store.lock().expect("store lock poisoned").items()?;
    Ok(Json(json!({ "rows": catalog::list_rows(&items) })))
}

async fn row(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let items = s.store.lock().expect("store lock poisoned").items()?;
    let items = catalog::build_row(&id, &items).ok_or_else(|| not_found("row"))?;
    Ok(Json(json!({ "id": id, "items": items })))
}

async fn item(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let store = s.store.lock().expect("store lock poisoned");
    let item = store.item(&id)?.ok_or_else(|| not_found("item"))?;
    let episodes: Vec<Value> = store
        .episodes_of(&id)?
        .into_iter()
        .map(|e| {
            json!({
                "id": e.id, "season": e.season, "episode": e.episode, "title": e.title,
                "aired": e.aired, "watched": e.watched, "position": e.position, "duration": e.duration, "has_art": e.art.is_some(),
            })
        })
        .collect();
    Ok(Json(json!({ "item": item, "episodes": episodes })))
}

async fn play(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let p = s
        .store
        .lock()
        .expect("store lock poisoned")
        .play_info(&id)?
        .ok_or_else(|| not_found("item"))?;
    tracing::info!(id = %p.id, asked_for = %id, kind = p.kind, resume = p.resume_position, "play requested");
    applog::debug!(path = %p.path.display(), title = %p.title, "play resolved");
    Ok(Json(json!({
        "id": p.id, "kind": p.kind, "title": p.title, "path": p.path,
        "resume_position": p.resume_position, "system": p.system,
    })))
}

#[derive(Deserialize)]
struct Progress {
    position: f64,
    duration: f64,
}

async fn progress(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(p): Json<Progress>,
) -> ApiResult<StatusCode> {
    s.store
        .lock()
        .expect("store lock poisoned")
        .save_progress(&id, p.position, p.duration)?;
    applog::debug!(id = %id, position = p.position, duration = p.duration, "progress saved");
    Ok(StatusCode::NO_CONTENT)
}

async fn played(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.store
        .lock()
        .expect("store lock poisoned")
        .mark_played(&id)?;
    tracing::info!(id = %id, "marked played");
    Ok(StatusCode::NO_CONTENT)
}

async fn art(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let item = s
        .store
        .lock()
        .expect("store lock poisoned")
        .item(&id)?
        .ok_or_else(|| not_found("item"))?;
    serve_image(PathBuf::from(item.art.ok_or_else(|| not_found("artwork"))?)).await
}

async fn episode_art(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let art = s.store.lock().expect("store lock poisoned").episode_art(&id)?;
    serve_image(PathBuf::from(art.ok_or_else(|| not_found("artwork"))?)).await
}

async fn serve_image(path: PathBuf) -> ApiResult<Response> {
    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        tracing::warn!(path = %path.display(), error = %e, "artwork unreadable");
        not_found("artwork")
    })?;
    // By content, not file name: yt-dlp avatars are PNGs saved as poster.jpg.
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"RIFF") {
        "image/webp"
    } else {
        "image/jpeg"
    };
    Ok(([(header::CONTENT_TYPE, mime)], bytes).into_response())
}

async fn rescan(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let stats = tokio::task::spawn_blocking(move || s.rescan())
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!({
        "added": stats.added, "removed": stats.removed, "entries": stats.entries, "episodes": stats.episodes,
    })))
}
