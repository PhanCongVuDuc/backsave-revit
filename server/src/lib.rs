pub mod api;
pub mod clock;
pub mod config;
pub mod engine;
pub mod jobs;
pub mod model;
pub mod upload;
pub mod worker;

use axum::Router;
use axum::extract::{MatchedPath, Request};
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
}

/// The whole HTTP application: API, uploads and (when configured) the static web files.
pub fn app(state: AppState) -> Router {
    let web_dir = state.config.web_dir.clone();
    let mut router = Router::new().merge(api::routes()).merge(upload::routes());
    if let Some(dir) = web_dir {
        let index = dir.join("index.html");
        router = router.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)));
    }
    router.with_state(state).layer(from_fn(log_request))
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
