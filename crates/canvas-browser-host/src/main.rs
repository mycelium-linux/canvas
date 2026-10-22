use anyhow::{Context, Result};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use canvas_browser_host::{BrowserPool, CreateSession, HostConfig};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
struct App {
    pool: Arc<BrowserPool>,
    client: reqwest::Client,
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: canvas-browser-host <config.json>")?;
    let config = HostConfig::read(&path)?;
    let listen = config.listen;
    let app = App {
        pool: Arc::new(BrowserPool::new(config)?),
        client: reqwest::Client::new(),
    };
    let router = Router::new()
        .route("/health", get(health))
        .route("/v1/sessions", get(list).post(create))
        .route("/v1/sessions/{id}", delete(remove))
        .route(
            "/v1/sessions/{id}/webdriver",
            post(proxy_root).get(proxy_root).delete(proxy_root),
        )
        .route(
            "/v1/sessions/{id}/webdriver/{*path}",
            post(proxy).get(proxy).delete(proxy).put(proxy).patch(proxy),
        )
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("bind {listen}"))?;
    eprintln!("canvas-browser-host: listening on {listen}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, axum::Json(json!({"ok":true})))
}

fn authorized(app: &App, headers: &HeaderMap) -> bool {
    app.pool.authenticate(
        headers
            .get(axum::http::header::AUTHORIZATION)
            .map(|v| v.as_bytes()),
    )
}

async fn list(State(app): State<App>, headers: HeaderMap) -> Response {
    if !authorized(&app, &headers) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    match app.pool.list() {
        Ok(value) => axum::Json(value).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn create(
    State(app): State<App>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<CreateSession>,
) -> Response {
    if !authorized(&app, &headers) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    match tokio::task::spawn_blocking(move || app.pool.create(request)).await {
        Ok(Ok(value)) => (StatusCode::CREATED, axum::Json(value)).into_response(),
        Ok(Err(e)) => error(StatusCode::BAD_REQUEST, e.to_string()),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn remove(State(app): State<App>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if !authorized(&app, &headers) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    match tokio::task::spawn_blocking(move || app.pool.remove(&id)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => error(StatusCode::NOT_FOUND, e.to_string()),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn proxy_root(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    method: Method,
    body: Bytes,
) -> Response {
    forward(app, headers, id, String::new(), method, body).await
}
async fn proxy(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, path)): Path<(String, String)>,
    method: Method,
    body: Bytes,
) -> Response {
    forward(app, headers, id, path, method, body).await
}

async fn forward(
    app: App,
    headers: HeaderMap,
    id: String,
    path: String,
    method: Method,
    body: Bytes,
) -> Response {
    if !authorized(&app, &headers) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let port = match app.pool.webdriver_port(&id) {
        Ok(p) => p,
        Err(e) => return error(StatusCode::NOT_FOUND, e.to_string()),
    };
    let url = if path.is_empty() {
        format!("http://127.0.0.1:{port}/")
    } else {
        format!("http://127.0.0.1:{port}/{path}")
    };
    let mut request = app.client.request(method.clone(), url).body(body);
    if let Some(content_type) = headers.get(axum::http::header::CONTENT_TYPE) {
        request = request.header(reqwest::header::CONTENT_TYPE, content_type.as_bytes());
    }
    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => return error(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .cloned();
    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    if method == Method::POST
        && path == "session"
        && status.is_success()
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        let webdriver_id = value
            .pointer("/value/sessionId")
            .or_else(|| value.get("sessionId"))
            .and_then(serde_json::Value::as_str);
        if let Some(webdriver_id) = webdriver_id
            && let Err(e) = app.pool.start_stream(&id, webdriver_id)
        {
            return error(
                StatusCode::BAD_GATEWAY,
                format!("browser started but media failed: {e:#}"),
            );
        }
    }
    let mut builder = Response::builder().status(status);
    if let Some(value) = content_type {
        builder = builder.header(axum::http::header::CONTENT_TYPE, value.as_bytes());
    }
    builder
        .body(Body::from(bytes))
        .unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, axum::Json(json!({"error":message.into()}))).into_response()
}
