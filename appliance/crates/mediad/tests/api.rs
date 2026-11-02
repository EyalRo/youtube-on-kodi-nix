use std::fs;
use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mediad::{AppState, router};
use scanner::ReleaseDb;
use serde_json::{Value, json};
use store::Store;
use tower::ServiceExt;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn media() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(r, "movies/Alien (1979)/Alien (1979).mkv", "x");
    write(r, "movies/Alien (1979)/poster.jpg", "jpgdata");
    write(r, "movies/Heat (1995)/Heat (1995).mkv", "x");
    write(r, "tv/Show/Show S01E01.mkv", "x");
    write(r, "tv/Show/Show S01E02.mkv", "x");
    write(
        r,
        "tv/Show/Show S01E01.nfo",
        "<episodedetails><aired>2024-01-01</aired></episodedetails>",
    );
    write(
        r,
        "tv/Show/Show S01E02.nfo",
        "<episodedetails><aired>2024-02-01</aired></episodedetails>",
    );
    dir
}

fn app(root: &Path) -> axum::Router {
    let state = AppState::new(
        Store::open_in_memory().unwrap(),
        root.to_path_buf(),
        ReleaseDb::empty(),
    );
    state.rescan().unwrap();
    router(state)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

async fn json_of(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (s, b) = call(app, method, uri, body).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

fn ids(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn health_and_rows() {
    let dir = media();
    let app = app(dir.path());
    let (s, v) = json_of(&app, "GET", "/health", None).await;
    assert_eq!(
        (s, v["status"].as_str(), v["entries"].as_u64()),
        (StatusCode::OK, Some("ok"), Some(3))
    );
    let (_, v) = json_of(&app, "GET", "/rows", None).await;
    assert_eq!(v["rows"][0]["id"], "everything");
}

#[tokio::test]
async fn everything_is_newest_first() {
    let dir = media();
    let app = app(dir.path());
    let (s, v) = json_of(&app, "GET", "/rows/everything", None).await;
    assert_eq!(s, StatusCode::OK);
    let titles: Vec<_> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Show", "Heat", "Alien"]);
    assert_eq!(
        json_of(&app, "GET", "/rows/nope", None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn play_progress_and_unwatched_row() {
    let dir = media();
    let app = app(dir.path());
    let (_, rows) = json_of(&app, "GET", "/rows/tvshows", None).await;
    let show = ids(&rows).remove(0);

    let (_, p) = json_of(&app, "GET", &format!("/items/{show}/play"), None).await;
    assert_eq!(p["kind"], "episode");
    assert!(p["path"].as_str().unwrap().ends_with("Show S01E01.mkv"));
    let ep = p["id"].as_str().unwrap().to_string();

    let (s, _) = call(
        &app,
        "POST",
        &format!("/items/{ep}/progress"),
        Some(json!({"position": 100.0, "duration": 1000.0})),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, p) = json_of(&app, "GET", &format!("/items/{show}/play"), None).await;
    assert_eq!(
        (p["id"].as_str(), p["resume_position"].as_f64()),
        (Some(ep.as_str()), Some(100.0))
    );

    call(
        &app,
        "POST",
        &format!("/items/{ep}/progress"),
        Some(json!({"position": 990.0, "duration": 1000.0})),
    )
    .await;
    let (_, p) = json_of(&app, "GET", &format!("/items/{show}/play"), None).await;
    assert!(p["path"].as_str().unwrap().ends_with("Show S01E02.mkv"));

    let (_, item) = json_of(&app, "GET", &format!("/items/{show}"), None).await;
    assert_eq!(item["item"]["watched"], 1);
    assert_eq!(item["episodes"].as_array().unwrap().len(), 2);
    let (_, unwatched) = json_of(&app, "GET", "/rows/unwatched", None).await;
    assert_eq!(ids(&unwatched), [show]);
}

#[tokio::test]
async fn watched_endpoint_sets_and_unwatches_movie_episode_show_and_season() {
    let dir = media();
    let app = app(dir.path());
    let (_, rows) = json_of(&app, "GET", "/rows/tvshows", None).await;
    let show = ids(&rows).remove(0);

    // A movie: watch it, then unwatch it.
    let (_, movies) = json_of(&app, "GET", "/rows/movies", None).await;
    let movie = ids(&movies).remove(0);
    assert_eq!(
        json_of(&app, "GET", &format!("/items/{movie}/watched"), None)
            .await
            .1["watched"],
        false
    );
    assert_eq!(
        call(&app, "POST", &format!("/items/{movie}/watched"), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        json_of(&app, "GET", &format!("/items/{movie}/watched"), None)
            .await
            .1["watched"],
        true
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/items/{movie}/watched?watched=false"),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        json_of(&app, "GET", &format!("/items/{movie}/watched"), None)
            .await
            .1["watched"],
        false
    );

    // A whole show: watch all episodes.
    assert_eq!(
        call(&app, "POST", &format!("/items/{show}/watched"), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let (_, item) = json_of(&app, "GET", &format!("/items/{show}"), None).await;
    assert_eq!(item["item"]["watched"], 2);
    assert!(item["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["watched"] == true));
    // Unwatch one season (both episodes are season 1).
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/items/{show}/watched?watched=false&season=1"),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, item) = json_of(&app, "GET", &format!("/items/{show}"), None).await;
    assert_eq!(item["item"]["watched"], 0);
    assert!(item["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["watched"] == false));

    // Unknown id 404s.
    assert_eq!(
        call(&app, "POST", "/items/zzz/watched", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn unknown_items_are_404() {
    let dir = media();
    let app = app(dir.path());
    assert_eq!(
        json_of(&app, "GET", "/items/zzz", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        json_of(&app, "GET", "/items/zzz/play", None).await.0,
        StatusCode::NOT_FOUND
    );
    let (s, _) = call(
        &app,
        "POST",
        "/items/zzz/progress",
        Some(json!({"position": 1.0, "duration": 2.0})),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artwork_is_served() {
    let dir = media();
    let app = app(dir.path());
    let (_, rows) = json_of(&app, "GET", "/rows/movies", None).await;
    let with_art = rows["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Alien")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, bytes) = call(&app, "GET", &format!("/items/{with_art}/art"), None).await;
    assert_eq!(
        (s, bytes.as_slice()),
        (StatusCode::OK, b"jpgdata".as_slice())
    );
    let heat = rows["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Heat")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        call(&app, "GET", &format!("/items/{heat}/art"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn rescan_picks_up_new_files_and_refuses_an_unmounted_share() {
    let dir = media();
    let app = app(dir.path());
    write(dir.path(), "movies/Ronin (1998)/Ronin (1998).mkv", "x");
    let (s, v) = json_of(&app, "POST", "/rescan", None).await;
    assert_eq!((s, v["added"].as_u64()), (StatusCode::OK, Some(1)));

    fs::remove_dir_all(dir.path().join("movies")).unwrap();
    fs::remove_dir_all(dir.path().join("tv")).unwrap();
    let (s, _) = json_of(&app, "POST", "/rescan", None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (_, v) = json_of(&app, "GET", "/health", None).await;
    assert_eq!(v["entries"], 4);
}

fn app_with_channels(root: &Path, file: &Path) -> axum::Router {
    let state = AppState::new(Store::open_in_memory().unwrap(), root.to_path_buf(), ReleaseDb::empty())
        .with_channels_file(file.to_path_buf());
    router(state)
}

#[tokio::test]
async fn channels_can_be_listed_followed_and_unfollowed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("channels.conf");
    fs::write(&file, "# mine\n@idlecub | Idle cub\n@Lunduke\n").unwrap();
    let app = app_with_channels(dir.path(), &file);

    let (st, body) = call(&app, "GET", "/youtube/channels", None).await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["channels"], json!([{"entry": "@idlecub", "name": "Idle cub"}, {"entry": "@Lunduke", "name": "@Lunduke"}]));

    let (st, body) = call(&app, "POST", "/youtube/channels", Some(json!({"entry": "UCVcCi1Vkcg9dtcSxR0dAZZw", "name": "John Wolfe Gaming"}))).await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert!(fs::read_to_string(&file).unwrap().contains("UCVcCi1Vkcg9dtcSxR0dAZZw | John Wolfe Gaming"));
    assert!(fs::read_to_string(&file).unwrap().starts_with("# mine"), "comments are kept");

    let (_, body) = call(&app, "POST", "/youtube/channels", Some(json!({"entry": "@lunduke"}))).await;
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["followed"], json!(false), "already followed");

    let (st, _) = call(&app, "POST", "/youtube/channels", Some(json!({"entry": "not a channel"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _) = call(&app, "POST", "/youtube/channels/remove", Some(json!({"entry": "@idlecub"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!fs::read_to_string(&file).unwrap().contains("idlecub"));
    let (st, _) = call(&app, "POST", "/youtube/channels/remove", Some(json!({"entry": "@idlecub"}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn channels_need_a_configured_file() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = call(&app(dir.path()), "GET", "/youtube/channels", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn game_lists_are_rows_and_a_user_file_can_extend_them() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(r, "roms/nes/Super Mario Bros. (World).nes", "x");
    write(r, "roms/nes/Legend of Zelda, The (USA).nes", "x");
    write(r, "roms/nes/Castlevania (USA).nes", "x");
    write(r, "roms/nes/Tetris (USA).nes", "x");
    let state = AppState::new(Store::open_in_memory().unwrap(), r.to_path_buf(), ReleaseDb::empty());
    state.rescan().unwrap();
    let app = router(state.clone());

    let (_, rows) = json_of(&app, "GET", "/rows", None).await;
    let list = |id: &str| rows["rows"].as_array().unwrap().iter().find(|x| x["id"] == id).cloned();
    assert_eq!(list("list:canon-nes").unwrap()["count"], 3);
    assert!(list("list:canon-snes").is_none(), "no SNES games, no row");
    let (s, v) = json_of(&app, "GET", "/rows/list:canon-nes", None).await;
    assert_eq!(s, StatusCode::OK);
    let titles: Vec<_> = v["items"].as_array().unwrap().iter().map(|i| i["title"].as_str().unwrap().to_string()).collect();
    assert_eq!(titles, ["Super Mario Bros.", "Legend of Zelda, The", "Castlevania"]);

    let user = catalog::GameLists::from_json(r#"[{"id":"puzzle","label":"Puzzlers","titles":["Tetris"]}]"#).unwrap();
    let app = router(state.with_game_lists(catalog::GameLists::builtin().merged_with(user)));
    let (_, v) = json_of(&app, "GET", "/rows/list:puzzle", None).await;
    assert_eq!(v["items"][0]["title"], "Tetris");
    assert_eq!(json_of(&app, "GET", "/rows/list:nope", None).await.0, StatusCode::NOT_FOUND);
}
