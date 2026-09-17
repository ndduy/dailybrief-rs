//! `dailybrief serve`: the web server, the scheduler and the manual-run starter in one process.
//! Binds `service.bind:service.port` after the loopback guard; stops on SIGTERM/Ctrl-C.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{Env, load_all};
use crate::core::embed::embedder_for;
use crate::core::time::parse_tz;
use crate::db::Db;
use crate::db::repo::RunKind;
use crate::harness::claude_code::EnvError;
use crate::harness::scheduler::Scheduler;
use crate::harness::service_runner::{ServiceRunner, ServiceRunnerError, ServiceRunnerOptions};
use crate::web::app::{AppState, assert_bind_allowed, router};
use crate::web::auth::{AccessSettings, AccessVerifier};

use super::{CommandError, db_path};

pub async fn run(env: &Env, process_env: &HashMap<String, String>) -> Result<(), CommandError> {
    let loaded = load_all(env)?;
    let config = loaded.config;
    assert_bind_allowed(&config.service.bind, env.in_container)?;
    let tz = parse_tz(&config.service.timezone)
        .map_err(|e| CommandError::Usage(format!("service.timezone: {e}")))?;
    let access = AccessSettings::from_env(process_env, &config.service.bind)
        .map_err(|e| CommandError::Usage(e.to_string()))?
        .map(|settings| {
            tracing::info!(issuer = %settings.issuer(), "Cloudflare Access verification on");
            Arc::new(AccessVerifier::new(settings, None))
        });
    if access.is_none() {
        tracing::warn!("Cloudflare Access verification bypassed: CF_ACCESS_AUD unset on loopback");
    }
    let db = Db::open(&db_path(&config))?;
    // A forbidden variable is a hard error (it would change billing); a missing token only
    // disables runs, so the pages still serve on a box without credentials.
    let runner = match ServiceRunner::new(
        config.clone(),
        db.clone(),
        loaded.topics,
        embedder_for(&config),
        process_env,
        ServiceRunnerOptions::default(),
    ) {
        Ok(r) => Some(Arc::new(r)),
        Err(ServiceRunnerError::Env(EnvError::MissingToken)) => {
            tracing::warn!(
                "CLAUDE_CODE_OAUTH_TOKEN is missing: runs are disabled on this instance"
            );
            None
        }
        Err(ServiceRunnerError::Env(e)) => return Err(CommandError::Env(e)),
        Err(e) => return Err(CommandError::Usage(e.to_string())),
    };
    let addr = format!("{}:{}", config.service.bind, config.service.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|source| CommandError::Listen {
            addr: addr.clone(),
            source,
        })?;
    tracing::info!(%addr, runs_enabled = runner.is_some(), "serving");

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let scheduler_task = runner.as_ref().map(|runner| {
        let scheduler = Scheduler::new(&config.schedule.cron, tz)
            .map_err(|e| CommandError::Usage(e.to_string()));
        let runner = Arc::clone(runner);
        let mut stop_rx = stop_rx.clone();
        scheduler.map(|scheduler| {
            tokio::spawn(async move {
                let result = scheduler
                    .run_loop(
                        chrono::Utc::now,
                        tokio::time::sleep,
                        move || {
                            let runner = Arc::clone(&runner);
                            async move { runner.run(RunKind::Scheduled).await }
                        },
                        async move {
                            let _ = stop_rx.wait_for(|stopped| *stopped).await;
                        },
                    )
                    .await;
                if let Err(e) = result {
                    tracing::error!(error = %e, "scheduler stopped");
                }
            })
        })
    });
    let scheduler_task = match scheduler_task {
        Some(Ok(task)) => Some(task),
        Some(Err(e)) => return Err(e),
        None => None,
    };

    let state = AppState::new(db, config, tz, chrono::Utc::now, runner).with_access(access);
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    let _ = stop_tx.send(true);
    if let Some(task) = scheduler_task {
        let _ = task.await;
    }
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
