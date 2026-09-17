//! `report_feed_issue({ feedId, kind, note? })` → `{ ok: true }`. Records a `feed_issues` row for
//! the Curator and stamps `sources.last_error`. Never changes weights or enablement (`SPEC.md` §3).

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::time::to_iso;
use crate::db::repo;
use crate::mcp::error::{ToolError, ok};
use crate::mcp::server::DailyBriefServer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    Dead,
    Paywalled,
    Junk,
    Duplicate,
}

impl IssueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dead => "dead",
            Self::Paywalled => "paywalled",
            Self::Junk => "junk",
            Self::Duplicate => "duplicate",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportFeedIssueInput {
    /// The feed id from get_briefing's feedHealth list.
    pub feed_id: String,
    /// dead | paywalled | junk | duplicate
    pub kind: IssueKind,
    /// One line on what you saw (optional).
    #[serde(default)]
    pub note: Option<String>,
}

pub async fn run(
    s: &DailyBriefServer,
    input: ReportFeedIssueInput,
) -> Result<CallToolResult, ToolError> {
    let at = to_iso((s.now)());
    let run_id = s.run_id.clone();
    let note = input
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    let kind = input.kind;
    let feed_id = input.feed_id.trim().to_string();
    s.db.call(move |conn| {
        if repo::get_source(conn, &feed_id)?.is_none() {
            return Err(crate::db::DbError::Corrupt(format!(
                "unknown feed {feed_id}"
            )));
        }
        repo::insert_feed_issue(conn, &feed_id, &run_id, kind.as_str(), note.as_deref(), &at)?;
        let error = match &note {
            Some(n) => format!("{}: {n}", kind.as_str()),
            None => kind.as_str().to_string(),
        };
        repo::set_source_last_error(conn, &feed_id, &error)?;
        Ok(())
    })
    .await
    .map_err(|e| match e {
        crate::db::DbError::Corrupt(m) if m.starts_with("unknown feed ") => {
            ToolError::UnknownFeed(m.trim_start_matches("unknown feed ").to_string())
        }
        other => ToolError::Internal(other.to_string()),
    })?;
    Ok(ok(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use crate::core::testutil::source;
    use crate::db::{Db, repo};
    use crate::mcp::server::testkit::*;
    use serde_json::json;

    #[tokio::test]
    async fn report_feed_issue_unknown_feed_is_error() {
        let h = Harness::start(server_with(Db::open_in_memory().unwrap())).await;
        let r = h
            .call(
                "report_feed_issue",
                json!({ "feedId": "ghost", "kind": "dead" }),
            )
            .await;
        assert_eq!(r.is_error, Some(true));
        assert_eq!(
            structured(&r)["error"],
            "No feed with id 'ghost'. Use the feedHealth list from get_briefing."
        );
    }

    #[tokio::test]
    async fn report_feed_issue_writes_row_and_last_error() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| source(c, "s")).unwrap();
        let h = Harness::start(server_with(db.clone())).await;
        let r = h
            .call(
                "report_feed_issue",
                json!({ "feedId": "s", "kind": "paywalled", "note": "every link asks for login" }),
            )
            .await;
        assert_eq!(r.is_error, Some(false));
        assert_eq!(structured(&r), json!({ "ok": true }));
        let issues = db.with(|c| repo::list_feed_issues(c, "s")).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, "paywalled");
        assert_eq!(issues[0].note.as_deref(), Some("every link asks for login"));
        assert_eq!(issues[0].run_id.as_deref(), Some(RUN));
        let src = db.with(|c| repo::get_source(c, "s")).unwrap().unwrap();
        assert_eq!(
            src.last_error.as_deref(),
            Some("paywalled: every link asks for login")
        );
        assert!(src.enabled, "reporting never disables a source");
        let bad = h
            .call(
                "report_feed_issue",
                json!({ "feedId": "s", "kind": "boring" }),
            )
            .await;
        assert_eq!(
            bad.is_error,
            Some(true),
            "schema rejects an unknown kind: {bad:?}"
        );
    }
}
