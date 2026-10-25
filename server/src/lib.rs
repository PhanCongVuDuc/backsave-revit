pub mod clock;
pub mod config;
pub mod engine;
pub mod jobs;
pub mod model;
pub mod upload;
pub mod worker;

use std::sync::Arc;

/// Shared state handed to every route.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<jobs::JobStore>,
    pub engine: Arc<engine::Engine>,
    pub config: Arc<config::Config>,
    pub pairs: Arc<Vec<(u16, u16)>>,
    pub clock: Arc<dyn clock::Clock>,
}
