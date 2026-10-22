//! Owner-local bridge from the native Canvas renderer to its browser mirror.
//!
//! This is intentionally loopback-only. The browser receives the pixels that
//! native Canvas actually rendered; credentials and control sockets never
//! cross into WASM. The direct WASM projection remains a separate low-detail
//! mode, not a second interpretation of the workspace.

use anyhow::{Context, Result, bail};
use axum::{
    Router,
    body::Body,
    extract::Query,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use canvas::control::{self, Command, ControlRequest, Outcome, Receipt};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
};

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[tokio::main]
async fn main() -> Result<()> {
    let listen = std::env::var("CANVAS_WEB_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8090".into())
        .parse::<SocketAddr>()
        .context("CANVAS_WEB_LISTEN must be a socket address")?;
    if !listen.ip().is_loopback() {
        bail!("canvas-web-host refuses non-loopback listen address {listen}");
    }

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/frame", get(frame))
        .route("/v1/key", get(key))
        .route("/v1/pointer", get(pointer));
    let listener = tokio::net::TcpListener::bind(listen).await?;
    eprintln!("canvas-web-host: exact native mirror on http://{listen}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    with_cors(Response::new(Body::from("ok")))
}

async fn frame() -> Response {
    match tokio::task::spawn_blocking(capture).await {
        Ok(Ok(bytes)) => {
            let mut response = Response::new(Body::from(bytes));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store, max-age=0"),
            );
            with_cors(response)
        }
        Ok(Err(error)) => error_response(error),
        Err(error) => error_response(anyhow::anyhow!("capture worker failed: {error}")),
    }
}

#[derive(Deserialize)]
struct BrowserKey {
    key: String,
    #[serde(default)]
    shift: bool,
    #[serde(default)]
    control: bool,
    #[serde(default)]
    alt: bool,
    #[serde(default)]
    meta: bool,
}

async fn key(Query(input): Query<BrowserKey>) -> Response {
    let request = ControlRequest {
        schema_version: control::CONTROL_VERSION,
        request_id: format!(
            "canvas-web-key-{}",
            REQUEST_ID.fetch_add(1, Ordering::Relaxed)
        ),
        expected_revision: None,
        actor: None,
        command: Command::BrowserKey {
            key: input.key,
            shift: input.shift,
            control: input.control,
            alt: input.alt,
            meta: input.meta,
        },
    };
    match tokio::task::spawn_blocking(move || control::send(&request)).await {
        Ok(Ok(reply)) => match reply.outcome {
            Outcome::Ok {
                receipt: Receipt::BrowserKeyAccepted,
            } => with_cors(StatusCode::NO_CONTENT.into_response()),
            Outcome::Error { code, message } => {
                error_response(anyhow::anyhow!("Canvas key {code:?}: {message}"))
            }
            _ => error_response(anyhow::anyhow!("Canvas returned a non-key receipt")),
        },
        Ok(Err(error)) => error_response(anyhow::Error::msg(error)),
        Err(error) => error_response(anyhow::anyhow!("key worker failed: {error}")),
    }
}

#[derive(Deserialize)]
struct BrowserPointer {
    phase: String,
    x: f32,
    y: f32,
    #[serde(default)]
    delta: f32,
}

async fn pointer(Query(input): Query<BrowserPointer>) -> Response {
    let request = ControlRequest {
        schema_version: control::CONTROL_VERSION,
        request_id: format!(
            "canvas-web-pointer-{}",
            REQUEST_ID.fetch_add(1, Ordering::Relaxed)
        ),
        expected_revision: None,
        actor: None,
        command: Command::BrowserPointer {
            phase: input.phase,
            x: input.x,
            y: input.y,
            delta: input.delta,
        },
    };
    match tokio::task::spawn_blocking(move || control::send(&request)).await {
        Ok(Ok(reply)) => match reply.outcome {
            Outcome::Ok {
                receipt: Receipt::BrowserPointerAccepted,
            } => with_cors(StatusCode::NO_CONTENT.into_response()),
            Outcome::Error { code, message } => {
                error_response(anyhow::anyhow!("Canvas pointer {code:?}: {message}"))
            }
            _ => error_response(anyhow::anyhow!("Canvas returned a non-pointer receipt")),
        },
        Ok(Err(error)) => error_response(anyhow::Error::msg(error)),
        Err(error) => error_response(anyhow::anyhow!("pointer worker failed: {error}")),
    }
}

fn capture() -> Result<Vec<u8>> {
    let request = ControlRequest {
        schema_version: control::CONTROL_VERSION,
        request_id: format!("canvas-web-{}", REQUEST_ID.fetch_add(1, Ordering::Relaxed)),
        expected_revision: None,
        actor: None,
        command: Command::Screenshot {
            surface_id: None,
            region: None,
        },
    };
    let reply = control::send(&request).map_err(anyhow::Error::msg)?;
    match reply.outcome {
        Outcome::Ok {
            receipt: Receipt::ScreenshotCaptured { path, .. },
        } => {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("read native Canvas capture {}", path.display()));
            // The control protocol deliberately returns a file for tooling.
            // This bridge consumes it immediately, so retaining every frame
            // would turn a mirror session into unbounded temp storage.
            let _ = std::fs::remove_file(path);
            bytes
        }
        Outcome::Error { code, message } => bail!("Canvas capture {code:?}: {message}"),
        _ => bail!("Canvas returned a non-screenshot receipt"),
    }
}

fn error_response(error: anyhow::Error) -> Response {
    with_cors((StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response())
}

fn with_cors(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("http://127.0.0.1:8088"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_listener_is_rejected_by_policy() {
        let address = "0.0.0.0:8090".parse::<SocketAddr>().unwrap();
        assert!(!address.ip().is_loopback());
    }
}
