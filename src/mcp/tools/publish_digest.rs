//! `publish_digest()` → `{ ok: true, digestId, date, forYou, beyondRadar }` or
//! `{ ok: false, violations: [...], fatal? }` with `isError: true`. The third rejection in this
//! process adds `fatal: true` and every later call is refused outright; the runner treats the
//! run as failed when no digest follows.

use std::sync::atomic::Ordering;

use rmcp::model::CallToolResult;
use serde_json::json;

use crate::core::digest::{PublishError, publish};
use crate::core::time::parse_tz;
use crate::mcp::error::{ToolError, ok, rejected};
use crate::mcp::server::DailyBriefServer;

pub const FATAL_AFTER_REJECTIONS: u8 = 3;

pub async fn run(s: &DailyBriefServer) -> Result<CallToolResult, ToolError> {
    // `SPEC.md` §3: three rejections end the run as failed. A later publish, even a valid
    // one, is refused without touching the database (`spec/m2.md` §11 #7).
    if s.publish_rejections.load(Ordering::SeqCst) >= FATAL_AFTER_REJECTIONS {
        return Ok(rejected(json!({
            "ok": false,
            "fatal": true,
            "violations": [format!(
                "publish_digest was rejected {FATAL_AFTER_REJECTIONS} times; this run is fatal and cannot publish. End the run."
            )],
        })));
    }
    let caps = s.config.caps;
    let tz =
        parse_tz(&s.config.service.timezone).map_err(|e| ToolError::Internal(e.to_string()))?;
    let run_id = s.run_id.clone();
    let now = (s.now)();
    let outcome =
        s.db.call(move |conn| Ok(publish(conn, &caps, tz, &run_id, now)))
            .await?;
    match outcome {
        Ok(p) => Ok(ok(json!({
            "ok": true,
            "digestId": p.digest_id,
            "date": p.date,
            "forYou": p.for_you,
            "beyondRadar": p.beyond_radar,
        }))),
        Err(PublishError::Violations(violations)) => Ok(reject(s, violations)),
        Err(PublishError::AlreadyPublished(id)) => Ok(reject(
            s,
            vec![format!("This run already published digest '{id}'.")],
        )),
        Err(PublishError::Db(e)) => Err(ToolError::Internal(e.to_string())),
    }
}

fn reject(s: &DailyBriefServer, violations: Vec<String>) -> CallToolResult {
    let count = s.publish_rejections.fetch_add(1, Ordering::SeqCst) + 1;
    let mut body = json!({ "ok": false, "violations": violations });
    if count >= FATAL_AFTER_REJECTIONS {
        body["fatal"] = json!(true);
    }
    rejected(body)
}
