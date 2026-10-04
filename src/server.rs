use crate::config::Config;
use crate::storage::{sanitize_ipxe_key, StorageManager};
use tokio_util::io::ReaderStream;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, Method, Response, StatusCode};
use axum::routing::any;
use axum::Router;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

pub struct AppState {
    pub config: Config,
    pub storage: StorageManager,
    pub start: Instant,
}

fn http_ts() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    crate::timefmt::format_datetime(secs)
}

fn req_log(client: &SocketAddr, line: &str) {
    println!("[{}] {} - {}", http_ts(), client.ip(), line);
}

async fn health(State(state): State<Arc<AppState>>) -> Response<Body> {
    #[derive(Serialize)]
    struct TargetOut {
        file: String,
        iqn: String,
        nbd_port: u16,
        root_dev: String,
        format: String,
        is_windows: bool,
        has_partitions: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        alive: Option<bool>,
    }
    #[derive(Serialize)]
    struct HealthOut {
        status: &'static str,
        version: &'static str,
        pid: u32,
        uptime_seconds: f64,
        mode: String,
        standalone: bool,
        targets: std::collections::BTreeMap<String, TargetOut>,
    }
    let mut targets = std::collections::BTreeMap::new();
    for t in state.storage.health_targets().await {
        targets.insert(
            sanitize_ipxe_key(&t.id),
            TargetOut {
                file: t.file.clone(),
                iqn: t.iqn.clone(),
                nbd_port: t.nbd_port,
                root_dev: t.root_dev.clone(),
                format: t.format.clone(),
                is_windows: t.is_windows,
                has_partitions: t.has_partitions,
                alive: t.alive,
            },
        );
    }
    let uptime = state.start.elapsed().as_secs_f64();
    let body = serde_json::to_string_pretty(&HealthOut {
        status: "ok",
        version: "2.5.0-standalone",
        pid: std::process::id(),
        uptime_seconds: uptime,
        mode: format!("{:?}", state.config.boot_mode).to_lowercase(),
        standalone: true,
        targets,
    })
    .unwrap_or_else(|_| "{}".into());

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .unwrap()
}

async fn serve_static(path: &str, is_head: bool) -> Response<Body> {
    match tokio::fs::File::open(path).await {
        Ok(file) => {
            let meta = match file.metadata().await {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[ERR] Failed to stat static file {}: {}", path, e);
                    return Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from(format!("Failed to stat file: {}", e)))
                        .unwrap();
                }
            };
            let len = meta.len();
            if is_head {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header(header::CONTENT_LENGTH, len)
                    .header(header::ACCEPT_RANGES, "bytes")
                    .body(Body::empty())
                    .unwrap()
            } else {
                let stream = ReaderStream::new(file);
                let body = Body::from_stream(stream);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header(header::CONTENT_LENGTH, len)
                    .header(header::ACCEPT_RANGES, "bytes")
                    .body(body)
                    .unwrap()
            }
        }
        Err(e) => {
            eprintln!("[ERR] Failed to open static file {}: {}", path, e);
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(format!("File not found: {}", e)))
                .unwrap()
        }
    }
}

async fn standalone_ipxe_menu(State(state): State<Arc<AppState>>) -> Response<Body> {
    state.storage.scan_and_export().await;
    let script = state.storage.build_standalone_ipxe_menu().await;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CONTENT_LENGTH, script.len())
        .body(Body::from(script))
        .unwrap()
}

async fn handle_route(
    State(state): State<Arc<AppState>>,
    ConnectInfo(client): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response<Body> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let pure_path = uri.path();
    req_log(&client, &format!("{} {}", method, uri));

    let is_head = method == Method::HEAD;
    if (method == Method::GET || is_head) && pure_path == "/vtoy/vmlinuz" {
        return serve_static(state.config.kernel_path.as_str(), is_head).await;
    }
    if (method == Method::GET || is_head) && pure_path == "/vtoy/initramfs.img" {
        return serve_static(state.config.initrd_path.as_str(), is_head).await;
    }

    // Serve ISO or raw disk files directly for HTTP sanboot
    if (method == Method::GET || is_head) && pure_path.starts_with("/iso/") {
        let filename = pure_path.trim_start_matches("/iso/");
        let filepath = format!("{}/{}", state.config.vtoy_dir, filename);
        return serve_static(&filepath, is_head).await;
    }

    if matches!(pure_path, "/boot.ipxe" | "/ipxe" | "/") {
        if method == Method::GET {
            return standalone_ipxe_menu(State(state)).await;
        }
        if method == Method::HEAD {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain")
                .header(header::CONTENT_LENGTH, 0)
                .body(Body::empty())
                .unwrap();
        }
    }

    if pure_path == "/api/health" || pure_path == "/health" {
        return health(State(state)).await;
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(format!("404 Not Found: {}", pure_path)))
        .unwrap()
}

pub fn make_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", any(handle_route))
        .route("/{*path}", any(handle_route))
        .with_state(state)
}

pub fn build_router(state: Arc<AppState>) -> Router {
    make_app(state)
}
