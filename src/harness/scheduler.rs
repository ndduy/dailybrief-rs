//! The morning run's clock (ADR 0003): a `croner` expression evaluated in the service timezone and
//! a sleep loop. The clock and the sleep are injected so tests drive it without waiting.

use std::future::Future;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("cron '{expr}' is invalid: {message}")]
    BadCron { expr: String, message: String },
    #[error("cron '{expr}' has no next occurrence")]
    NoNext { expr: String },
}

#[derive(Debug, Clone)]
pub struct Scheduler {
    expr: String,
    cron: croner::Cron,
    tz: Tz,
}

impl Scheduler {
    pub fn new(expr: &str, tz: Tz) -> Result<Self, SchedulerError> {
        let cron = croner::Cron::from_str(expr).map_err(|e| SchedulerError::BadCron {
            expr: expr.to_string(),
            message: e.to_string(),
        })?;
        Ok(Self {
            expr: expr.to_string(),
            cron,
            tz,
        })
    }

    /// The first occurrence strictly after `now`, in UTC.
    pub fn next_after(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>, SchedulerError> {
        let local = now.with_timezone(&self.tz);
        let next =
            self.cron
                .find_next_occurrence(&local, false)
                .map_err(|_| SchedulerError::NoNext {
                    expr: self.expr.clone(),
                })?;
        Ok(next.with_timezone(&Utc))
    }

    /// Sleeps until each occurrence and runs `job`; a failing job is logged and the loop goes on.
    /// Ends when `stop` resolves.
    pub async fn run_loop<C, S, SF, J, JF, T, E>(
        &self,
        clock: C,
        sleep: S,
        mut job: J,
        stop: impl Future<Output = ()>,
    ) -> Result<(), SchedulerError>
    where
        C: Fn() -> DateTime<Utc>,
        S: Fn(Duration) -> SF,
        SF: Future<Output = ()>,
        J: FnMut() -> JF,
        JF: Future<Output = Result<T, E>>,
        E: std::fmt::Display,
    {
        tokio::pin!(stop);
        loop {
            let now = clock();
            let next = self.next_after(now)?;
            let wait = (next - now).to_std().unwrap_or(Duration::ZERO);
            tracing::info!(cron = %self.expr, next = %next, "scheduler waiting");
            tokio::select! {
                biased;
                () = &mut stop => return Ok(()),
                () = sleep(wait) => {}
            }
            match job().await {
                Ok(_) => tracing::info!(cron = %self.expr, "scheduled run finished"),
                Err(e) => {
                    tracing::warn!(cron = %self.expr, error = %e, "scheduled run skipped or failed")
                }
            }
        }
    }
}

/// One named schedule with its job, for [`run_jobs`].
pub struct ScheduledJob<'a> {
    pub name: &'static str,
    pub scheduler: &'a Scheduler,
    pub job: Box<dyn FnMut() -> BoxFuture<'static, Result<(), String>> + Send + 'a>,
}

pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Every schedule in one loop (ADR 0013, generalised in M3): whichever job is due first runs
/// and the others wait for it, so no two jobs overlap. An occurrence of another schedule that
/// passed while a job ran fires right after it, in list order (a morning that overruns 07:00
/// still gets its prune). A tie is broken by list order. A failing job is logged and the loop
/// goes on. Ends when `stop` resolves.
pub async fn run_jobs<C, S, SF>(
    jobs: &mut [ScheduledJob<'_>],
    clock: C,
    sleep: S,
    stop: impl Future<Output = ()>,
) -> Result<(), SchedulerError>
where
    C: Fn() -> DateTime<Utc>,
    S: Fn(Duration) -> SF,
    SF: Future<Output = ()>,
{
    tokio::pin!(stop);
    loop {
        let now = clock();
        let mut due: Vec<DateTime<Utc>> = Vec::with_capacity(jobs.len());
        for j in jobs.iter() {
            due.push(j.scheduler.next_after(now)?);
        }
        let Some(first) = (0..jobs.len()).min_by_key(|&i| (due[i], i)) else {
            stop.await;
            return Ok(());
        };
        let next = due[first];
        let wait = (next - now).to_std().unwrap_or(Duration::ZERO);
        tracing::info!(job = jobs[first].name, cron = %jobs[first].scheduler.expr, next = %next, "scheduler waiting");
        tokio::select! {
            biased;
            () = &mut stop => return Ok(()),
            () = sleep(wait) => {}
        }
        log_job(jobs[first].name, (jobs[first].job)().await);
        // Anything else whose occurrence passed while that ran, in list order.
        for i in (0..jobs.len()).filter(|&i| i != first) {
            if due[i] <= clock() {
                log_job(jobs[i].name, (jobs[i].job)().await);
            }
        }
    }
}

fn log_job(name: &str, result: Result<(), String>) {
    match result {
        Ok(()) => tracing::info!(job = name, "scheduled job finished"),
        Err(e) => tracing::warn!(job = name, error = %e, "scheduled job skipped or failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::parse_tz;
    use chrono::TimeZone;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[test]
    fn scheduler_next_occurrence_in_ho_chi_minh() {
        let s = Scheduler::new("30 6 * * *", parse_tz("Asia/Ho_Chi_Minh").unwrap()).unwrap();
        // 06:30 in Ho Chi Minh (UTC+7) is 23:30 UTC the previous day.
        let now = Utc.with_ymd_and_hms(2026, 9, 17, 6, 0, 0).unwrap();
        let next = s.next_after(now).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 9, 17, 23, 30, 0).unwrap());
        // Exactly at the tick, the next one is tomorrow.
        let at_tick = Utc.with_ymd_and_hms(2026, 9, 17, 23, 30, 0).unwrap();
        assert_eq!(
            s.next_after(at_tick).unwrap(),
            Utc.with_ymd_and_hms(2026, 9, 18, 23, 30, 0).unwrap()
        );
        assert!(Scheduler::new("30 6 * *", chrono_tz::UTC).is_err());
    }

    #[tokio::test]
    async fn scheduler_fires_once_per_tick_with_fake_clock_and_survives_a_failing_job() {
        let s = Scheduler::new("30 6 * * *", chrono_tz::UTC).unwrap();
        let clock = Rc::new(Cell::new(
            Utc.with_ymd_and_hms(2026, 9, 17, 0, 0, 0).unwrap(),
        ));
        let slept: Rc<RefCell<Vec<Duration>>> = Rc::new(RefCell::new(Vec::new()));
        let fired: Rc<RefCell<Vec<DateTime<Utc>>>> = Rc::new(RefCell::new(Vec::new()));
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let stop_tx = Rc::new(RefCell::new(Some(stop_tx)));
        let c1 = Rc::clone(&clock);
        let c2 = Rc::clone(&clock);
        let slept2 = Rc::clone(&slept);
        let fired2 = Rc::clone(&fired);
        let stop2 = Rc::clone(&stop_tx);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let result = s
                    .run_loop(
                        move || c1.get(),
                        move |d| {
                            slept2.borrow_mut().push(d);
                            c2.set(c2.get() + chrono::Duration::from_std(d).unwrap());
                            async {}
                        },
                        move || {
                            let now = clock.get();
                            fired2.borrow_mut().push(now);
                            let n = fired2.borrow().len();
                            if n == 3
                                && let Some(tx) = stop2.borrow_mut().take()
                            {
                                let _ = tx.send(());
                            }
                            async move {
                                if n == 1 {
                                    Err::<(), _>("a run is already in progress (lock held)")
                                } else {
                                    Ok(())
                                }
                            }
                        },
                        async {
                            let _ = stop_rx.await;
                        },
                    )
                    .await;
                assert!(result.is_ok());
            })
            .await;
        let fired = fired.borrow();
        assert_eq!(
            fired.len(),
            3,
            "three ticks, the first job failing did not stop the loop"
        );
        assert_eq!(
            fired[0],
            Utc.with_ymd_and_hms(2026, 9, 17, 6, 30, 0).unwrap()
        );
        assert_eq!(
            fired[1],
            Utc.with_ymd_and_hms(2026, 9, 18, 6, 30, 0).unwrap()
        );
        assert_eq!(
            fired[2],
            Utc.with_ymd_and_hms(2026, 9, 19, 6, 30, 0).unwrap()
        );
        let slept = slept.borrow();
        assert_eq!(slept[0], Duration::from_secs(6 * 3600 + 30 * 60));
        assert_eq!(slept[1], Duration::from_secs(24 * 3600));
    }

    /// One clock, several crons, on `run_jobs`: the 06:30 run fires, then the 07:00 prune,
    /// and the next day again; a job that fails does not stop any schedule.
    type Log = std::sync::Arc<std::sync::Mutex<Vec<(&'static str, DateTime<Utc>)>>>;

    struct Sim {
        run: Scheduler,
        prune: Scheduler,
        curate: Option<Scheduler>,
        start: DateTime<Utc>,
        prune_fails_first: bool,
        overrun_minutes: i64,
        stop_after_prunes: usize,
    }

    async fn simulate(sim: Sim) -> Vec<(&'static str, DateTime<Utc>)> {
        use std::sync::{Arc, Mutex};
        let clock = Arc::new(Mutex::new(sim.start));
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let stop_tx = Arc::new(Mutex::new(Some(stop_tx)));
        let now = |c: &Arc<Mutex<DateTime<Utc>>>| *c.lock().unwrap();
        let (c1, c2, c3, c4, c5) = (
            Arc::clone(&clock),
            Arc::clone(&clock),
            Arc::clone(&clock),
            Arc::clone(&clock),
            Arc::clone(&clock),
        );
        let (l1, l2, l3) = (Arc::clone(&log), Arc::clone(&log), Arc::clone(&log));
        let (prune_fails_first, overrun, stop_after) = (
            sim.prune_fails_first,
            sim.overrun_minutes,
            sim.stop_after_prunes,
        );
        let mut jobs: Vec<ScheduledJob<'_>> = vec![
            ScheduledJob {
                name: "run",
                scheduler: &sim.run,
                job: Box::new(move || {
                    l1.lock().unwrap().push(("run", now(&c3)));
                    *c3.lock().unwrap() += chrono::Duration::minutes(overrun);
                    Box::pin(async { Ok(()) })
                }),
            },
            ScheduledJob {
                name: "prune",
                scheduler: &sim.prune,
                job: Box::new(move || {
                    l2.lock().unwrap().push(("prune", now(&c4)));
                    let n = l2
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|(k, _)| *k == "prune")
                        .count();
                    if n == stop_after
                        && let Some(tx) = stop_tx.lock().unwrap().take()
                    {
                        let _ = tx.send(());
                    }
                    let fail = prune_fails_first && n == 1;
                    Box::pin(async move {
                        if fail {
                            Err("cannot remove /data/runs/x: busy".to_string())
                        } else {
                            Ok(())
                        }
                    })
                }),
            },
        ];
        if let Some(curate) = sim.curate.as_ref() {
            jobs.push(ScheduledJob {
                name: "curate",
                scheduler: curate,
                job: Box::new(move || {
                    l3.lock().unwrap().push(("curate", now(&c5)));
                    Box::pin(async { Ok(()) })
                }),
            });
        }
        let result = run_jobs(
            &mut jobs,
            move || now(&c1),
            move |d| {
                *c2.lock().unwrap() += chrono::Duration::from_std(d).unwrap();
                async {}
            },
            async {
                let _ = stop_rx.await;
            },
        )
        .await;
        assert!(result.is_ok());
        log.lock().unwrap().clone()
    }

    fn daily(prune_fails_first: bool, overrun_minutes: i64) -> Sim {
        Sim {
            run: Scheduler::new("30 6 * * *", chrono_tz::UTC).unwrap(),
            prune: Scheduler::new("0 7 * * *", chrono_tz::UTC).unwrap(),
            curate: None,
            start: Utc.with_ymd_and_hms(2026, 9, 17, 0, 0, 0).unwrap(),
            prune_fails_first,
            overrun_minutes,
            stop_after_prunes: 2,
        }
    }

    #[tokio::test]
    async fn daily_prune_fires_after_the_morning_run() {
        let log = simulate(daily(false, 0)).await;
        let expect = |d: u32, h: u32, m: u32| Utc.with_ymd_and_hms(2026, 9, d, h, m, 0).unwrap();
        assert_eq!(
            log,
            vec![
                ("run", expect(17, 6, 30)),
                ("prune", expect(17, 7, 0)),
                ("run", expect(18, 6, 30)),
                ("prune", expect(18, 7, 0)),
            ]
        );
    }

    #[tokio::test]
    async fn prune_job_error_does_not_stop_serve() {
        let log = simulate(daily(true, 0)).await;
        assert_eq!(
            log.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            ["run", "prune", "run", "prune"],
            "the failing first prune stopped neither schedule"
        );
    }

    /// A morning run that ends at 07:10 (two attempts) does not lose that day's prune: the
    /// passed 07:00 occurrence fires right after the run.
    #[tokio::test]
    async fn prune_fires_after_a_run_that_overran_it() {
        let log = simulate(daily(false, 40)).await;
        let expect = |d: u32, h: u32, m: u32| Utc.with_ymd_and_hms(2026, 9, d, h, m, 0).unwrap();
        assert_eq!(
            log,
            vec![
                ("run", expect(17, 6, 30)),
                ("prune", expect(17, 7, 10)),
                ("run", expect(18, 6, 30)),
                ("prune", expect(18, 7, 10)),
            ]
        );
    }

    /// Eight days from a Monday (the stop lands on the second Monday's prune): eight runs,
    /// eight prunes, and the Curator once, on Sunday 2026-09-20 at 07:30, after that
    /// morning's run and prune.
    #[tokio::test]
    async fn curator_fires_on_sundays_only() {
        let mut sim = daily(false, 0);
        sim.curate = Some(Scheduler::new("30 7 * * 0", chrono_tz::UTC).unwrap());
        sim.start = Utc.with_ymd_and_hms(2026, 9, 14, 0, 0, 0).unwrap(); // a Monday
        sim.stop_after_prunes = 8;
        let log = simulate(sim).await;
        let runs = log.iter().filter(|(k, _)| *k == "run").count();
        let prunes = log.iter().filter(|(k, _)| *k == "prune").count();
        let curates: Vec<DateTime<Utc>> = log
            .iter()
            .filter(|(k, _)| *k == "curate")
            .map(|(_, t)| *t)
            .collect();
        assert_eq!((runs, prunes), (8, 8));
        assert_eq!(
            curates,
            vec![Utc.with_ymd_and_hms(2026, 9, 20, 7, 30, 0).unwrap()]
        );
        let sunday: Vec<&str> = log
            .iter()
            .filter(|(_, t)| {
                t.date_naive() == chrono::NaiveDate::from_ymd_opt(2026, 9, 20).unwrap()
            })
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(sunday, ["run", "prune", "curate"]);
    }

    /// Two schedules due at the same instant run in list order, both of them.
    #[tokio::test]
    async fn ties_run_in_list_order() {
        let mut sim = daily(false, 0);
        sim.prune = Scheduler::new("30 6 * * *", chrono_tz::UTC).unwrap();
        sim.stop_after_prunes = 2;
        let log = simulate(sim).await;
        let expect = |d: u32| Utc.with_ymd_and_hms(2026, 9, d, 6, 30, 0).unwrap();
        assert_eq!(
            log,
            vec![
                ("run", expect(17)),
                ("prune", expect(17)),
                ("run", expect(18)),
                ("prune", expect(18)),
            ]
        );
    }
}
