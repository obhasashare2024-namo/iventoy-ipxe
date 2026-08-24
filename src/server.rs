use crate::config::Config;
use crate::storage::{sanitize_ipxe_key, StorageManager};
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, Method, Response, StatusCode, Uri};
use axum::routing::{any, get};
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
                has_partitions: t.has_partitions,
                alive: t.alive,
            },
        );
    }
    let mode = match state.config.boot_mode {
        crate::config::BootMode::Iscsi => "iscsi",
        crate::config::BootMode::Nbd => "nbd",
    };
    let out = HealthOut {
        status: "healthy",
        version: env!("CARGO_PKG_VERSION"),
        pid: std::process::id(),
        uptime_seconds: (state.start.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        mode: mode.into(),
        targets,
    };
    let body = serde_json::to_string_pretty(&out).unwrap_or_default();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

async fn serve_static(path: &str) -> Response<Body> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, bytes.len())
            .body(Body::from(bytes))
            .unwrap(),
        Err(_) => {
            let msg = format!("File Not Found: {}\n", path);
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::CONTENT_LENGTH, msg.len())
                .body(Body::from(msg))
                .unwrap()
        }
    }
}

/// Same placement rules as Python:
/// new = orig[:pos] + menu_block + orig[pos:] + handler_block  (pos = start of first "choose ...")
/// or orig + menu_block + handler_block if no choose found.
fn inject_vtoy_menu(original_ipxe: &str, menu_block: &str, handler_block: &str) -> String {
    match find_choose(original_ipxe) {
        Some(pos) => format!(
            "{}{}{}{}",
            &original_ipxe[..pos],
            menu_block,
            &original_ipxe[pos..],
            handler_block
        ),
        None => format!(
            "{}{}{}",
            original_ipxe, menu_block, handler_block
        ),
    }
}

/// Equivalent of Python `re.search(r'(choose\s+.*)', script, re.IGNORECASE)` position.
fn find_choose(s: &str) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find("choose") {
        let abs = from + rel;
        let before_ok = abs == 0
            || !(bytes[abs - 1].is_ascii_alphanumeric() || bytes[abs - 1] == b'_');
        let after = &lower[abs + "choose".len()..];
        // Python `\s+` requires at least one whitespace after keyword
        let ws = after.chars().take_while(|c| c.is_whitespace()).count();
        let label_start = &after[ws..];
        let has_label = label_start
            .chars()
            .next()
            .map(|c| !(c.is_whitespace() || c == '\n' || c == '\r'))
            .unwrap_or(false);
        if before_ok && ws >= 1 && has_label {
            return Some(abs);
        }
        from = abs + "choose".len();
    }
    None
}

async fn ipxe_menu(State(state): State<Arc<AppState>>, uri: Uri) -> Response<Body> {
    state.storage.scan_and_export().await;

    let target_url = format!("http://{}{}", state.config.iventoy_real, uri);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => reqwest::Client::new(),
    };
    let raw_script = match client.get(&target_url).send().await {
        Ok(resp) => resp.text().await.unwrap_or_else(|_| fallback_script()),
        Err(_) => fallback_script(),
    };

    let (menu_block, handler_block) = state.storage.build_vtoy_menu().await;
    let injected = inject_vtoy_menu(&raw_script, &menu_block, &handler_block);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CONTENT_LENGTH, injected.len())
        .body(Body::from(injected))
        .unwrap()
}

fn fallback_script() -> String {
    "#!ipxe\n\n:start\nmenu Network Boot Menu\nitem --gap -- Virtual Disks\nchoose selected\ngoto ${selected}\n".to_string()
}

async fn proxy_pass(
    State(state): State<Arc<AppState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response<Body> {
    let target_url = format!("http://{}{}", state.config.iventoy_real, uri);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(_) => reqwest::Client::new(),
    };
    let mut req = match method {
        Method::POST => client.post(&target_url),
        Method::PUT => client.put(&target_url),
        Method::DELETE => client.delete(&target_url),
        Method::HEAD => client.head(&target_url),
        Method::PATCH => client.patch(&target_url),
        _ => client.get(&target_url),
    };
    for (k, v) in headers.iter() {
        let kl = k.as_str().to_ascii_lowercase();
        if kl == "host" || kl == "connection" || kl == "content-length" {
            continue;
        }
        req = req.header(k, v);
    }
    req = req.header("Host", &state.config.iventoy_real);
    if !body.is_empty() && method != Method::GET && method != Method::HEAD {
        req = req.body(body.to_vec());
    }
    match req.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut rb = Response::builder().status(status);
            for (k, v) in resp.headers().iter() {
                let kl = k.as_str().to_ascii_lowercase();
                if kl == "connection" || kl == "transfer-encoding" || kl == "content-length" {
                    continue;
                }
                rb = rb.header(k, v);
            }
            let bytes = if method == Method::HEAD {
                Vec::new()
            } else {
                resp.bytes().await.unwrap_or_default().to_vec()
            };
            rb.header(header::CONTENT_LENGTH, bytes.len())
                .body(Body::from(bytes))
                .unwrap()
        }
        Err(e) => {
            let msg = format!("Bridge Proxy Error: {}\n", e);
            Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header(header::CONTENT_LENGTH, msg.len())
                .body(Body::from(msg))
                .unwrap()
        }
    }
}

async fn handler_any(
    State(state): State<Arc<AppState>>,
    ConnectInfo(client): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response<Body> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();
    let pure_path = uri.path();
    req_log(&client, &format!("{} {}", method, uri));

    if method == Method::GET && pure_path == "/vtoy/vmlinuz" {
        return serve_static(state.config.kernel_path.clone().as_str()).await;
    }
    if method == Method::GET && pure_path == "/vtoy/initramfs.img" {
        return serve_static(state.config.initrd_path.clone().as_str()).await;
    }
    if matches!(pure_path, "/boot.ipxe" | "/ipxe" | "/") {
        if method == Method::GET {
            return ipxe_menu(State(state), uri).await;
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
    let body = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    proxy_pass(State(state), method, uri, headers, body).await
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .fallback(any(handler_any))
        .with_state(state)
}
