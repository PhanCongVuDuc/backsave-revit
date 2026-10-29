use revit_downgrade_server::cleanup::{spawn_cleanup, wipe_jobs_dir};
use revit_downgrade_server::clock::{Clock, SystemClock};
use revit_downgrade_server::config::Config;
use revit_downgrade_server::engine::Engine;
use revit_downgrade_server::jobs::JobStore;
use revit_downgrade_server::reports::Reports;
use revit_downgrade_server::stats::Stats;
use revit_downgrade_server::worker::spawn_workers;
use revit_downgrade_server::{AppState, app};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// After a stop signal, open connections (a slow upload, a download) get this long to finish.
const CONNECTION_GRACE: Duration = Duration::from_secs(10);
/// Cancelled conversions get this long for their engine to exit and their files to go.
const WORKER_GRACE: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_target(false).init();
    if let Err(e) = run().await {
        tracing::error!("revit-downgrade-server: {e:#}");
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
    // Usage statistics live outside the jobs and reports directories, so no cleanup touches them.
    let stats = Arc::new(Stats::open(config.data.join("stats"), clock.now()));
    let store = Arc::new(
        JobStore::new(config.data.clone(), clock.clone()).with_disk_budget(config.disk_budget),
    );
    spawn_workers(config.workers, store.clone(), engine.clone(), stats.clone());
    let reports = Arc::new(Reports::new(config.data.join("reports")));
    spawn_cleanup(store.clone(), reports.clone(), clock.clone());
    let engine_version = Arc::new(engine.version().await);

    let listen = config.listen;
    let state = AppState {
        store: store.clone(),
        engine,
        config: Arc::new(config),
        pairs: Arc::new(pairs),
        clock,
        reports,
        stats,
        engine_version,
    };
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("could not listen on {listen}"))?;
    tracing::info!(%listen, "listening");

    let stopping = CancellationToken::new();
    {
        let (stopping, store) = (stopping.clone(), store.clone());
        tokio::spawn(async move {
            stop_signal().await;
            tracing::info!("stopping: cancelling unfinished jobs");
            // Running engines are killed through their jobs' cancel tokens.
            store.cancel_all();
            stopping.cancel();
        });
    }
    let server = axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(stopping.clone().cancelled_owned());
    tokio::select! {
        r = server => r?,
        () = async {
            stopping.cancelled().await;
            tokio::time::sleep(CONNECTION_GRACE).await;
        } => tracing::warn!("open connections did not finish in time"),
    }

    let deadline = tokio::time::Instant::now() + WORKER_GRACE;
    while store.has_cancelled_running() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tracing::info!("stopped");
    Ok(())
}

/// Resolves on Ctrl-C, or on SIGTERM on Unix (what `docker stop` sends).
async fn stop_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}
