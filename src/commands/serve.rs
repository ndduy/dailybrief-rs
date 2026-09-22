//! `dailybrief serve`: the web server, the scheduler and the manual-run starter in one process.
//! Binds `service.bind:service.port` after the loopback guard; stops on SIGTERM/Ctrl-C.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{Env, load_all};
use crate::core::embed::embedder_for;
use crate::core::retention::prune;
use crate::core::time::parse_tz;
use crate::db::Db;
use crate::db::repo::{self, RunKind};
use crate::harness::claude_code::EnvError;
use crate::harness::scheduler::{ScheduledJob, Scheduler, run_jobs};
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
    // A run left `running` by a crash or a restart would hold the day's state forever (and
    // the retention job skips running runs): anything older than the lock window is closed.
    {
        let stale_minutes = 2 * config.harness.claude_code.wall_clock_minutes + 5;
        let now = chrono::Utc::now();
        let before =
            crate::core::time::to_iso(now - chrono::Duration::minutes(i64::from(stale_minutes)));
        let ended_at = crate::core::time::to_iso(now);
        let n = db
            .call(move |conn| {
                repo::mark_orphaned_runs(conn, &before, &ended_at, "orphaned at restart")
            })
            .await?;
        if n > 0 {
            tracing::warn!(
                runs = n,
                "running rows older than the lock window marked killed"
            );
        }
    }
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
    // The Curator's runner shares the database and the lock; only the role differs.
    let curator = match runner.as_ref() {
        Some(_) => match ServiceRunner::new(
            config.clone(),
            db.clone(),
            Vec::new(),
            embedder_for(&config),
            process_env,
            ServiceRunnerOptions {
                role: crate::db::repo::Role::Curator,
                ..Default::default()
            },
        ) {
            Ok(r) => Some(Arc::new(r)),
            Err(e) => return Err(CommandError::Usage(e.to_string())),
        },
        None => None,
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
    // One loop drives every job (ADR 0013): the 06:30 run, the 07:00 prune and the Sunday
    // 07:30 Curator never overlap because the loop awaits whichever is due, in that order on
    // a tie. Without a runner only the prune runs.
    let run_scheduler = Scheduler::new(&config.schedule.cron, tz)
        .map_err(|e| CommandError::Usage(e.to_string()))?;
    let prune_scheduler = Scheduler::new(&config.retention.cron, tz)
        .map_err(|e| CommandError::Usage(e.to_string()))?;
    let curate_scheduler =
        Scheduler::new(&config.curator.cron, tz).map_err(|e| CommandError::Usage(e.to_string()))?;
    let scheduler_task = {
        let mut stop_rx = stop_rx.clone();
        let stop = async move {
            let _ = stop_rx.wait_for(|stopped| *stopped).await;
        };
        let db = db.clone();
        let data_dir = config.paths.data_dir.clone();
        let days = config.retention.days;
        let runner = runner.clone();
        let curator = curator.clone();
        tokio::spawn(async move {
            let mut jobs: Vec<ScheduledJob<'_>> = Vec::new();
            if let Some(runner) = runner.as_ref() {
                let runner = Arc::clone(runner);
                jobs.push(ScheduledJob {
                    name: "run",
                    scheduler: &run_scheduler,
                    job: Box::new(move || {
                        let runner = Arc::clone(&runner);
                        Box::pin(async move {
                            runner
                                .run(RunKind::Scheduled)
                                .await
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        })
                    }),
                });
            }
            jobs.push(ScheduledJob {
                name: "prune",
                scheduler: &prune_scheduler,
                job: Box::new(move || {
                    let db = db.clone();
                    let data_dir = data_dir.clone();
                    Box::pin(async move {
                        prune(&db, &data_dir, chrono::Utc::now(), days, false)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    })
                }),
            });
            if let Some(curator) = curator.as_ref() {
                let curator = Arc::clone(curator);
                jobs.push(ScheduledJob {
                    name: "curate",
                    scheduler: &curate_scheduler,
                    job: Box::new(move || {
                        let curator = Arc::clone(&curator);
                        Box::pin(async move {
                            curator
                                .run(RunKind::Scheduled)
                                .await
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        })
                    }),
                });
            }
            let result = run_jobs(&mut jobs, chrono::Utc::now, tokio::time::sleep, stop).await;
            if let Err(e) = result {
                tracing::error!(error = %e, "scheduler stopped");
            }
        })
    };

    let state = AppState::new(db, config, tz, chrono::Utc::now, runner).with_access(access);
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    let _ = stop_tx.send(true);
    let _ = scheduler_task.await;
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
