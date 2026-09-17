//! `dailybrief serve`: the web server (and, from Task 23, the scheduler and the manual-run
//! starter). Binds `service.bind:service.port` after the loopback guard; stops on SIGTERM/Ctrl-C.

use crate::config::{Env, load_all};
use crate::core::time::parse_tz;
use crate::db::Db;
use crate::web::app::{AppState, assert_bind_allowed, router};

use super::{CommandError, db_path};

pub async fn run(env: &Env) -> Result<(), CommandError> {
    let loaded = load_all(env)?;
    let config = loaded.config;
    assert_bind_allowed(&config.service.bind, env.in_container)?;
    let tz = parse_tz(&config.service.timezone)
        .map_err(|e| CommandError::Usage(format!("service.timezone: {e}")))?;
    let db = Db::open(&db_path(&config))?;
    let addr = format!("{}:{}", config.service.bind, config.service.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|source| CommandError::Listen {
            addr: addr.clone(),
            source,
        })?;
    tracing::info!(%addr, "serving");
    let state = AppState {
        db,
        config,
        tz,
        now: chrono::Utc::now,
    };
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
    tracing::info!("shutdown signal received");
}
