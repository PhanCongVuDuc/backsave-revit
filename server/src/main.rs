use backsave_server::cleanup::{spawn_cleanup, wipe_jobs_dir};
use backsave_server::clock::SystemClock;
use backsave_server::config::Config;
use backsave_server::engine::Engine;
use backsave_server::jobs::JobStore;
use backsave_server::reports::Reports;
use backsave_server::worker::spawn_workers;
use backsave_server::{AppState, app};
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_target(false).init();
    if let Err(e) = run().await {
        eprintln!("backsave-server: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    use anyhow::Context;
    let config = Config::from_env()?;

    // Jobs never survive a restart: start from an empty jobs directory.
    wipe_jobs_dir(&config.data).context("could not reset the jobs directory")?;
    std::fs::create_dir_all(config.data.join("reports"))
        .context("could not create the reports directory")?;

    let engine = Arc::new(Engine::new(config.engine.clone()));
    let pairs = engine.capabilities().await.map_err(|e| {
        anyhow::anyhow!("the conversion engine did not report its capabilities: {e:?}")
    })?;
    tracing::info!(pairs = pairs.len(), "engine ready");

    let clock = Arc::new(SystemClock);
    let store = Arc::new(JobStore::new(config.data.clone(), clock.clone()));
    spawn_workers(config.workers, store.clone(), engine.clone());
    let reports = Arc::new(Reports::new(config.data.join("reports")));
    spawn_cleanup(store.clone(), reports.clone(), clock.clone());
    let engine_version = Arc::new(engine.version().await);

    let listen = config.listen;
    let state = AppState {
        store,
        engine,
        config: Arc::new(config),
        pairs: Arc::new(pairs),
        clock,
        reports,
        engine_version,
    };
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("could not listen on {listen}"))?;
    tracing::info!(%listen, "listening");
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
