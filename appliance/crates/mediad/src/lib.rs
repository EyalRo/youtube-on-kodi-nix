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
}

impl AppState {
    pub fn new(store: Store, media_root: PathBuf, release_db: ReleaseDb) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            media_root: Arc::new(media_root),
            release_db: Arc::new(release_db),
        }
    }

    /// Scan the media root and replace the catalog. Blocking: call from `spawn_blocking`.
    pub fn rescan(&self) -> Result<store::ScanStats, StoreError> {
        let scan = scanner::scan_all(&self.media_root, &self.release_db);
        for w in &scan.warnings {
            tracing::warn!(warning = %w, "scan warning");
        }
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
        Ok(stats)
    }
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
        .route("/rescan", post(rescan))
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
                "aired": e.aired, "watched": e.watched, "position": e.position, "duration": e.duration,
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
    tracing::info!(id = %p.id, kind = p.kind, resume = p.resume_position, "play requested");
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
    tracing::debug!(id = %id, position = p.position, duration = p.duration, "progress saved");
    Ok(StatusCode::NO_CONTENT)
}

async fn played(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.store
        .lock()
        .expect("store lock poisoned")
        .mark_played(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn art(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let item = s
        .store
        .lock()
        .expect("store lock poisoned")
        .item(&id)?
        .ok_or_else(|| not_found("item"))?;
    let path = PathBuf::from(item.art.ok_or_else(|| not_found("artwork"))?);
    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        tracing::warn!(path = %path.display(), error = %e, "artwork unreadable");
        not_found("artwork")
    })?;
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
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
