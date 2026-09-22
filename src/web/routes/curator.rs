//! `/curator`: the approval queue. `POST /curator/{id}/approve` is the only path that changes
//! the profile (`core::feedback::apply`, one transaction); `/reject` marks the row and nothing
//! else. Same-origin check as `/run`: the Access cookie is SameSite=None.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::internal;
use super::run::{header, is_same_origin};
use crate::core::feedback::{self, FeedbackError, Proposal, ProposalStatus};
use crate::db::repo;
use crate::web::app::AppState;
use crate::web::views;

/// Decided proposals shown under the pending ones.
pub const DECIDED_LIMIT: i64 = 30;

pub async fn index(State(state): State<AppState>) -> Response {
    let loaded = state
        .db
        .call(|conn| {
            let pending = repo::list_proposals(conn, ProposalStatus::Pending, 200)?
                .into_iter()
                .map(Proposal::from_row)
                .collect::<Result<Vec<_>, _>>()?;
            let mut decided = Vec::new();
            for status in [ProposalStatus::Approved, ProposalStatus::Rejected] {
                for row in repo::list_proposals(conn, status, DECIDED_LIMIT)? {
                    decided.push(Proposal::from_row(row)?);
                }
            }
            decided.sort_by(|a, b| b.decided_at.cmp(&a.decided_at).then(b.id.cmp(&a.id)));
            decided.truncate(DECIDED_LIMIT as usize);
            Ok((pending, decided))
        })
        .await;
    match loaded {
        Ok((pending, decided)) => views::curator::render(&pending, &decided).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Clone, Copy)]
enum Verdict {
    Approve,
    Reject,
}

async fn decide(state: &AppState, headers: &HeaderMap, id: String, verdict: Verdict) -> Response {
    if !is_same_origin(
        header(headers, "sec-fetch-site"),
        header(headers, "origin"),
        header(headers, "host"),
    ) {
        tracing::warn!("proposal decision refused: cross-origin request");
        return (StatusCode::FORBIDDEN, "cross-origin requests cannot decide").into_response();
    }
    let now = (state.now)();
    let decided = state
        .db
        .call(move |conn| {
            Ok(match verdict {
                Verdict::Approve => feedback::apply(conn, &id, now),
                Verdict::Reject => feedback::reject(conn, &id, now),
            })
        })
        .await;
    let outcome = match decided {
        Ok(o) => o,
        Err(e) => return internal(e),
    };
    match outcome {
        Ok(p) => {
            tracing::info!(proposal = %p.id, status = %p.status, kind = p.change.kind(), "proposal decided");
            if header(headers, "hx-request").is_some_and(|v| v == "true") {
                (StatusCode::OK, [("hx-refresh", "true")]).into_response()
            } else {
                (StatusCode::SEE_OTHER, [(header::LOCATION, "/curator")]).into_response()
            }
        }
        Err(FeedbackError::UnknownProposal(_)) => {
            (StatusCode::NOT_FOUND, "no such proposal").into_response()
        }
        Err(e @ FeedbackError::NotPending { .. }) => {
            (StatusCode::CONFLICT, e.to_string()).into_response()
        }
        Err(e @ FeedbackError::MissingTarget(_)) => {
            tracing::warn!(error = %e, "proposal cannot apply");
            (StatusCode::CONFLICT, format!("cannot apply: {e}")).into_response()
        }
        Err(FeedbackError::Db(e)) => internal(e),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

pub async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    decide(&state, &headers, id, Verdict::Approve).await
}

pub async fn reject(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    decide(&state, &headers, id, Verdict::Reject).await
}
