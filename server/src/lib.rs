pub mod api;
pub mod cleanup;
pub mod clock;
pub mod config;
pub mod engine;
pub mod jobs;
pub mod model;
pub mod reports;
pub mod upload;
pub mod worker;

use axum::Router;
use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderValue, header};
use axum::middleware::{Next, from_fn};
use axum::response::Response;
use std::sync::Arc;
use tower_http::services::{ServeDir, ServeFile};

/// Shared state handed to every route.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<jobs::JobStore>,
    pub engine: Arc<engine::Engine>,
    pub config: Arc<config::Config>,
    pub pairs: Arc<Vec<(u16, u16)>>,
    pub clock: Arc<dyn clock::Clock>,
    pub reports: Arc<reports::Reports>,
    /// First line of the engine's `--version`, read once at startup.
    pub engine_version: Arc<String>,
}

/// The whole HTTP application: API, uploads and (when configured) the static web files.
pub fn app(state: AppState) -> Router {
    let web_dir = state.config.web_dir.clone();
    let mut router = Router::new().merge(api::routes()).merge(upload::routes());
    if let Some(dir) = web_dir {
        let index = dir.join("index.html");
        router = router.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)));
    }
    router
        .with_state(state)
        .layer(from_fn(security_headers))
        .layer(from_fn(log_request))
}

/// Logs method, route template and status. File names, bodies and job ids never appear.
async fn log_request(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "(unmatched)".to_string(), |p| p.as_str().to_string());
    let resp = next.run(req).await;
    tracing::info!(%method, route = %route, status = resp.status().as_u16(), "request");
    resp
}

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'";

/// Browser hardening for every response: same-origin content only, no framing, no referrer.
async fn security_headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}
