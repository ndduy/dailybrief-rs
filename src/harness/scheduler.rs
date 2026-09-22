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

/// Two schedules in one loop (the morning run and the daily prune, ADR 0013): whichever is
/// due first runs, and the other waits for it to finish, so the prune can never overlap a
/// run. An occurrence of the other schedule that passed while a job ran fires right after it
/// (a morning that overruns 07:00 still gets its prune). A failing job is logged and the loop
/// goes on. Ends when `stop` resolves.
#[allow(clippy::too_many_arguments)] // two schedules, two jobs, clock, sleep and stop are the whole interface
pub async fn run_two_loops<C, S, SF, J1, JF1, T1, E1, J2, JF2, T2, E2>(
    first: &Scheduler,
    second: &Scheduler,
    clock: C,
    sleep: S,
    mut job1: J1,
    mut job2: J2,
    stop: impl Future<Output = ()>,
) -> Result<(), SchedulerError>
where
    C: Fn() -> DateTime<Utc>,
    S: Fn(Duration) -> SF,
    SF: Future<Output = ()>,
    J1: FnMut() -> JF1,
    JF1: Future<Output = Result<T1, E1>>,
    E1: std::fmt::Display,
    J2: FnMut() -> JF2,
    JF2: Future<Output = Result<T2, E2>>,
    E2: std::fmt::Display,
{
    tokio::pin!(stop);
    loop {
        let now = clock();
        let n1 = first.next_after(now)?;
        let n2 = second.next_after(now)?;
        let first_due = n1 <= n2;
        let (next, expr) = if first_due {
            (n1, &first.expr)
        } else {
            (n2, &second.expr)
        };
        let wait = (next - now).to_std().unwrap_or(Duration::ZERO);
        tracing::info!(cron = %expr, next = %next, "scheduler waiting");
        tokio::select! {
            biased;
            () = &mut stop => return Ok(()),
            () = sleep(wait) => {}
        }
        if first_due {
            log_job(&first.expr, job1().await);
            if n2 <= clock() {
                log_job(&second.expr, job2().await);
            }
        } else {
            log_job(&second.expr, job2().await);
            if n1 <= clock() {
                log_job(&first.expr, job1().await);
            }
        }
    }
}

fn log_job<T, E: std::fmt::Display>(expr: &str, result: Result<T, E>) {
    match result {
        Ok(_) => tracing::info!(cron = %expr, "scheduled job finished"),
        Err(e) => tracing::warn!(cron = %expr, error = %e, "scheduled job skipped or failed"),
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

    /// One clock, two crons: the 06:30 run fires, then the 07:00 prune, and the next day again.
    /// A prune that fails does not stop either schedule.
    type Log = Rc<RefCell<Vec<(&'static str, DateTime<Utc>)>>>;

    async fn two_loops_log(prune_fails_first: bool) -> Vec<(&'static str, DateTime<Utc>)> {
        two_loops_log_with(prune_fails_first, 0).await
    }

    /// `overrun_minutes` is how long the run job takes on the injected clock.
    async fn two_loops_log_with(
        prune_fails_first: bool,
        overrun_minutes: i64,
    ) -> Vec<(&'static str, DateTime<Utc>)> {
        let run = Scheduler::new("30 6 * * *", chrono_tz::UTC).unwrap();
        let prune = Scheduler::new("0 7 * * *", chrono_tz::UTC).unwrap();
        let clock = Rc::new(Cell::new(
            Utc.with_ymd_and_hms(2026, 9, 17, 0, 0, 0).unwrap(),
        ));
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let stop_tx = Rc::new(RefCell::new(Some(stop_tx)));
        let (c1, c2, c3, c4) = (
            Rc::clone(&clock),
            Rc::clone(&clock),
            Rc::clone(&clock),
            Rc::clone(&clock),
        );
        let (l1, l2, l3) = (Rc::clone(&log), Rc::clone(&log), Rc::clone(&log));
        let stop2 = Rc::clone(&stop_tx);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let result = run_two_loops(
                    &run,
                    &prune,
                    move || c1.get(),
                    move |d| {
                        c2.set(c2.get() + chrono::Duration::from_std(d).unwrap());
                        async {}
                    },
                    move || {
                        l1.borrow_mut().push(("run", c3.get()));
                        c3.set(c3.get() + chrono::Duration::minutes(overrun_minutes));
                        async { Ok::<(), String>(()) }
                    },
                    move || {
                        l2.borrow_mut().push(("prune", c4.get()));
                        let n = l3.borrow().iter().filter(|(k, _)| *k == "prune").count();
                        if n == 2
                            && let Some(tx) = stop2.borrow_mut().take()
                        {
                            let _ = tx.send(());
                        }
                        let fail = prune_fails_first && n == 1;
                        async move {
                            if fail {
                                Err::<(), _>("cannot remove /data/runs/x: busy".to_string())
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
        log.borrow().clone()
    }

    #[tokio::test]
    async fn daily_prune_fires_after_the_morning_run() {
        let log = two_loops_log(false).await;
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
        let log = two_loops_log(true).await;
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
        let log = two_loops_log_with(false, 40).await;
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
}
