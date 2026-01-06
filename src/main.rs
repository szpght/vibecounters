use std::{fs, io::Write, path::{Path, PathBuf}, sync::Arc};

use axum::{
    extract::{Path as AxumPath, State},
    http::{header, HeaderValue, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::sync::RwLock;
use tower_http::{compression::CompressionLayer, trace::TraceLayer};
use tracing::info;
use uuid::Uuid;

const DEFAULT_DATA_FILE: &str = "counters.json";
static STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/src/static");

// Re-export OffsetDateTime with custom serde that accepts both RFC3339 strings and the
// legacy array format produced by the default `time` serializer.
mod offset_datetime_rfc3339 {
    use serde::{de, Deserializer, Serializer};
    use time::{format_description::well_known::Rfc3339, Date, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

    pub fn serialize<S>(value: &OffsetDateTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = value
            .format(&Rfc3339)
            .map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<OffsetDateTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = OffsetDateTime;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(
                    "RFC3339 string or [year, ordinal, hour, minute, second, nanosecond, offset_hour, offset_minute, offset_second]",
                )
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                OffsetDateTime::parse(v, &Rfc3339).map_err(E::custom)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let year: i32 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let ordinal: u16 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let hour: u8 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(2, &self))?;
                let minute: u8 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(3, &self))?;
                let second: u8 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(4, &self))?;
                let nanosecond: u32 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(5, &self))?;
                let offset_hour: i8 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(6, &self))?;
                let offset_minute: i8 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(7, &self))?;
                let offset_second: i8 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(8, &self))?;

                let date = Date::from_ordinal_date(year, ordinal).map_err(de::Error::custom)?;
                let time = Time::from_hms_nano(hour.into(), minute.into(), second.into(), nanosecond)
                    .map_err(de::Error::custom)?;
                let offset = UtcOffset::from_hms(
                    offset_hour.into(),
                    offset_minute.into(),
                    offset_second.into(),
                )
                .map_err(de::Error::custom)?;

                Ok(PrimitiveDateTime::new(date, time).assume_offset(offset))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Counter {
    id: Uuid,
    title: String,
    #[serde(with = "offset_datetime_rfc3339")]
    target: OffsetDateTime,
    #[serde(with = "offset_datetime_rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct CreateCounter {
    title: String,
    target: String,
}

#[derive(Debug, Deserialize)]
struct UpdateCounter {
    title: String,
    target: String,
}

#[derive(Clone)]
struct AppState {
    counters: Arc<RwLock<Vec<Counter>>>,
    data_path: Arc<PathBuf>,
}

#[derive(Debug)]
enum AppError {
    NotFound,
    BadRequest(String),
    Internal(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "Not Found").into_response(),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            AppError::Internal(err) => {
                tracing::error!(error = %err, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }
        }
    }
}

type AppResult<T> = Result<T, AppError>;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    init_tracing();

    let data_path = data_file_path();
    info!("COUNTERS_FILE = {}", data_path.display());
    let counters = load_counters(&data_path)?;
    let state = AppState {
        counters: Arc::new(RwLock::new(counters)),
        data_path: Arc::new(data_path),
    };

    let app = Router::new()
        .route("/api/counters", get(list_counters).post(create_counter))
        .route(
            "/api/counters/:id",
            put(update_counter).delete(delete_counter),
        )
        .route("/health", get(health))
        .fallback(get(serve_static))
        .with_state(state)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http());

    // Read bind address and port from environment with sensible defaults.
    // ADDR defaults to 0.0.0.0, PORT defaults to 3000. The values are combined and parsed
    // as a SocketAddr (e.g. "0.0.0.0:3000" or "[::1]:3000").
    let addr = get_listen_addr()?;
    info!("listening on http://{}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;

    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .try_init();
}

fn get_listen_addr() -> Result<std::net::SocketAddr, anyhow::Error> {
    let host = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let candidate = format!("{}:{}", host, port);
    // Log which env values were used for startup to make debugging easier.
    info!("startup: ADDR='{}' PORT='{}' => {}", host, port, candidate);
    candidate
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid listen address '{}': {}", candidate, e))
}

fn data_file_path() -> PathBuf {
    std::env::var("COUNTERS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DATA_FILE))
}

fn load_counters(path: &Path) -> Result<Vec<Counter>, anyhow::Error> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(path)?;
    let counters: Vec<Counter> = serde_json::from_str(&data)?;
    Ok(counters)
}

fn persist_counters(path: &Path, counters: &[Counter]) -> Result<(), anyhow::Error> {
    let json = serde_json::to_vec_pretty(counters)?;
    let dir = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&dir)?;
    let tmp_name = format!(".counters-{}.tmp", Uuid::new_v4());
    let tmp_path = dir.join(tmp_name);
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(&json)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

async fn list_counters(State(state): State<AppState>) -> AppResult<Json<Vec<Counter>>> {
    let counters = state.counters.read().await.clone();
    Ok(Json(counters))
}

async fn create_counter(
    State(state): State<AppState>,
    Json(payload): Json<CreateCounter>,
) -> AppResult<Json<Counter>> {
    let target = parse_time(&payload.target)?;
    let mut counters = state.counters.write().await;
    let counter = Counter {
        id: Uuid::new_v4(),
        title: payload.title,
        target,
        created_at: OffsetDateTime::now_utc(),
    };
    counters.push(counter.clone());
    persist_counters(&state.data_path, &counters).map_err(AppError::Internal)?;
    Ok(Json(counter))
}

async fn update_counter(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Uuid>,
    Json(payload): Json<UpdateCounter>,
) -> AppResult<Json<Counter>> {
    let target = parse_time(&payload.target)?;
    let mut counters = state.counters.write().await;
    let updated = if let Some(counter) = counters.iter_mut().find(|c| c.id == id) {
        counter.title = payload.title;
        counter.target = target;
        Some(counter.clone())
    } else {
        None
    };

    if let Some(counter) = updated {
        let snapshot = counters.clone();
        persist_counters(&state.data_path, &snapshot).map_err(AppError::Internal)?;
        return Ok(Json(counter));
    }

    Err(AppError::NotFound)
}

async fn delete_counter(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Uuid>,
) -> AppResult<StatusCode> {
    let mut counters = state.counters.write().await;
    let len_before = counters.len();
    counters.retain(|c| c.id != id);
    if len_before == counters.len() {
        return Err(AppError::NotFound);
    }
    persist_counters(&state.data_path, &counters).map_err(AppError::Internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn health() -> &'static str {
    "ok"
}

fn parse_time(input: &str) -> AppResult<OffsetDateTime> {
    OffsetDateTime::parse(input, &Rfc3339)
        .map_err(|_| AppError::BadRequest("Invalid datetime (expected RFC3339)".into()))
}

async fn serve_static(uri: Uri) -> impl IntoResponse {
    let path = uri.path();
    if path == "/" || path.is_empty() {
        return Html(get_asset("index.html")).into_response();
    }

    let trimmed = path.trim_start_matches('/');
    if let Some(file) = STATIC_DIR.get_file(trimmed) {
        let body: Vec<u8> = file.contents().into();
        let mime = content_type_for(trimmed);
        let mut resp = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .body(body.into())
            .unwrap();
        if trimmed != "index.html" {
            resp.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=604800"),
            );
        }
        return resp;
    }

    Html(get_asset("index.html")).into_response()
}

fn content_type_for(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

fn get_asset(name: &str) -> String {
    STATIC_DIR
        .get_file(name)
        .and_then(|f| std::str::from_utf8(f.contents()).ok())
        .unwrap_or("missing asset")
        .to_string()
}
