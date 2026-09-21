//! A harness "success" is not a digest (`SPEC.md` §3): the run counts only when the final message
//! parses as `DigestOutput` (the JSON Schema itself is enforced by `--json-schema` in the
//! harness), a digest row exists for this run, and the ids agree.

use super::types::RunOutcome;
use crate::db::{Connection, DbError, repo};
use crate::editor::digest_output::DigestOutput;

/// `Ok(())` when the outcome is a verified digest, else the one-line reason.
pub fn verify_digest_outcome(
    conn: &Connection,
    run_id: &str,
    outcome: &RunOutcome,
) -> Result<Result<(), String>, DbError> {
    let RunOutcome::Success { result, .. } = outcome else {
        return Ok(Err("harness did not report success".into()));
    };
    let parsed = result
        .structured_output
        .clone()
        .and_then(|v| serde_json::from_value::<DigestOutput>(v).ok());
    let Some(output) = parsed else {
        return Ok(Err("final message did not parse as DigestOutput (schemas/digest.json is enforced by the harness)".into()));
    };
    let Some(digest) = repo::get_digest_by_run(conn, run_id)? else {
        return Ok(Err(
            "harness reported success but no digest was published for this run".into(),
        ));
    };
    if digest.id != output.digest_id {
        return Ok(Err(format!(
            "final message names digest '{}' but this run published '{}'",
            output.digest_id, digest.id
        )));
    }
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::db::repo::{DigestInsert, NewRun, RunKind};
    use crate::harness::claude_code::ResultEvent;
    use crate::harness::types::FailReason;
    use serde_json::json;

    const RUN: &str = "2026-09-17-verify";

    fn success(structured: Option<serde_json::Value>) -> RunOutcome {
        RunOutcome::Success {
            result: ResultEvent {
                subtype: "success".into(),
                is_error: false,
                duration_ms: Some(1),
                duration_api_ms: None,
                num_turns: Some(3),
                result: None,
                session_id: Some("s".into()),
                total_cost_usd: None,
                usage: None,
                structured_output: structured,
            },
            init: None,
        }
    }

    fn db_with_digest(digest_id: Option<&str>) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            repo::insert_run(
                c,
                &NewRun {
                    id: RUN.into(),
                    kind: RunKind::Manual,
                    harness: "test".into(),
                    attempt: 1,
                    started_at: "2026-09-17T06:00:00.000Z".into(),
                    transcript_path: None,
                },
            )?;
            if let Some(id) = digest_id {
                repo::insert_digest(
                    c,
                    &DigestInsert {
                        id: id.into(),
                        date: "2026-09-17".into(),
                        run_id: RUN.into(),
                        published_at: "2026-09-17T06:10:00.000Z".into(),
                        for_you_count: 24,
                        beyond_radar_count: 6,
                    },
                    &[],
                )?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    fn output(id: &str) -> serde_json::Value {
        json!({ "digestId": id, "date": "2026-09-17", "forYou": 24, "beyondRadar": 6 })
    }

    #[test]
    fn verify_accepts_a_matching_digest() {
        let db = db_with_digest(Some("2026-09-17-x"));
        let r = db
            .with(|c| verify_digest_outcome(c, RUN, &success(Some(output("2026-09-17-x")))))
            .unwrap();
        assert_eq!(r, Ok(()));
    }

    #[test]
    fn verify_rejects_non_success_bad_schema_missing_row_and_mismatch() {
        let db = db_with_digest(Some("2026-09-17-x"));
        let failed = RunOutcome::Failed {
            reason: FailReason::Error,
            message: "m".into(),
            exit_code: Some(1),
            result: None,
            init: None,
        };
        assert_eq!(
            db.with(|c| verify_digest_outcome(c, RUN, &failed)).unwrap(),
            Err("harness did not report success".to_string())
        );
        assert_eq!(
            db.with(|c| verify_digest_outcome(c, RUN, &success(Some(json!({ "nope": 1 })))))
                .unwrap(),
            Err("final message did not parse as DigestOutput (schemas/digest.json is enforced by the harness)".to_string())
        );
        assert_eq!(
            db.with(|c| verify_digest_outcome(c, RUN, &success(None)))
                .unwrap(),
            Err("final message did not parse as DigestOutput (schemas/digest.json is enforced by the harness)".to_string())
        );
        assert_eq!(
            db.with(|c| verify_digest_outcome(c, RUN, &success(Some(output("2026-09-17-y")))))
                .unwrap(),
            Err(
                "final message names digest '2026-09-17-y' but this run published '2026-09-17-x'"
                    .to_string()
            )
        );
        let empty = db_with_digest(None);
        assert_eq!(
            empty
                .with(|c| verify_digest_outcome(c, RUN, &success(Some(output("2026-09-17-x")))))
                .unwrap(),
            Err("harness reported success but no digest was published for this run".to_string())
        );
    }

    #[test]
    fn verify_reason_names_the_parse() {
        let db = crate::db::Db::open_in_memory().unwrap();
        let outcome = RunOutcome::Success {
            result: ResultEvent {
                subtype: "success".into(),
                is_error: false,
                duration_ms: None,
                duration_api_ms: None,
                num_turns: None,
                result: None,
                session_id: None,
                total_cost_usd: None,
                usage: None,
                structured_output: Some(serde_json::json!({ "not": "a digest" })),
            },
            init: None,
        };
        let reason = db
            .with(|c| verify_digest_outcome(c, "r", &outcome))
            .unwrap()
            .unwrap_err();
        assert!(
            reason.starts_with("final message did not parse as DigestOutput"),
            "{reason}"
        );
    }
}
