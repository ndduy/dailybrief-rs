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
}
