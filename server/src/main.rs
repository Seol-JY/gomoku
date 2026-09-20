//! `gomoku-server`: HTTP POST + SSE game server backed by PostgreSQL

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use server::{AppState, Config, build_app, db};
use tracing_subscriber::EnvFilter;

/// Matches `compose.yaml`
const DEFAULT_DATABASE_URL: &str = "postgres://gomoku:gomoku@localhost:5432/gomoku";

#[derive(Parser, Debug)]
#[command(
    name = "gomoku-server",
    version,
    about = "gomoku game server (HTTP + SSE, PostgreSQL)"
)]
struct Args {
    /// Address to listen on
    #[arg(long, env = "GOMOKU_BIND", default_value = "127.0.0.1:3000")]
    bind: SocketAddr,

    /// PostgreSQL connection URL. Migrations run automatically at start-up
    #[arg(long, env = "GOMOKU_DATABASE_URL", default_value = DEFAULT_DATABASE_URL)]
    database_url: String,

    /// Oldest client version allowed to connect
    #[arg(
        long,
        env = "GOMOKU_MIN_CLIENT_VERSION",
        default_value = server::state::DEFAULT_MIN_CLIENT_VERSION
    )]
    min_client_version: String,

    /// Newest released client version, advertised for the update hint
    #[arg(long, env = "GOMOKU_LATEST_CLIENT_VERSION")]
    latest_client_version: Option<String>,

    /// SSE keepalive interval in seconds
    #[arg(long, env = "GOMOKU_KEEPALIVE_SECS", default_value_t = 15)]
    keepalive_secs: u64,

    /// Seconds to wait for in-flight requests after a shutdown signal
    #[arg(long, env = "GOMOKU_SHUTDOWN_TIMEOUT_SECS", default_value_t = 10)]
    shutdown_timeout_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // .env optional; real env wins
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,sqlx=warn")),
        )
        .init();

    proto::parse_semver(&args.min_client_version)
        .with_context(|| format!("invalid --min-client-version {:?}", args.min_client_version))?;

    let pool = db::connect(&args.database_url).await?;
    if let Some(v) = &args.latest_client_version {
        proto::parse_semver(v).with_context(|| format!("invalid --latest-client-version {v:?}"))?;
    }
    let config = Config {
        min_client_version: args.min_client_version,
        latest_client_version: args.latest_client_version,
        keepalive: Duration::from_secs(args.keepalive_secs.max(1)),
        ..Config::default()
    };
    let state = AppState::new(pool, config);
    let app = build_app(state.clone());

    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("binding {}", args.bind))?;
    tracing::info!(
        addr = %listener.local_addr()?,
        db = %db::redact(&args.database_url),
        "gomoku-server listening"
    );

    let mut stopping = state.shutdown_signal();
    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
        shutdown_signal().await;
        state.shutdown();
    });
    let force = async {
        let _ = stopping.changed().await;
        tokio::time::sleep(Duration::from_secs(args.shutdown_timeout_secs)).await;
    };
    tokio::select! {
        r = serve => r?,
        () = force => tracing::warn!("shutdown timeout reached; exiting with open connections"),
    }
    tracing::info!("shutdown complete");
    Ok(())
}

/// SIGTERM too on Unix (containers, systemd)
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
