//! `GET /rate/reasons` reveals the reasons for a sign; `POST /rate` stores, replaces or deletes
//! the rating of one item (`spec/m3.md` rating-ui, ADR 0016). A rating writes the `ratings`
//! table and nothing else. Same-origin check as `/run`: the Access cookie is SameSite=None.

use axum::extract::rejection::{FormRejection, QueryRejection};
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::internal;
use super::run::{header, is_same_origin};
use crate::core::feedback::{Reason, Sign};
use crate::core::time::to_iso;
use crate::db::repo;
use crate::web::app::AppState;
use crate::web::views::{layout, rate};

#[derive(Debug, Deserialize)]
pub struct ReasonsQuery {
    pub item: String,
    pub sign: String,
}

#[derive(Debug, Deserialize)]
pub struct RateForm {
    pub item: String,
    pub sign: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `up` / `down`, or `none` (un-rate, or back to the two buttons).
fn parse_sign(s: &str) -> Result<Option<Sign>, Box<Response>> {
    match s {
        "none" => Ok(None),
        other => Sign::parse(other)
            .map(Some)
            .ok_or_else(|| Box::new(bad_sign())),
    }
}

fn bad_sign() -> Response {
    (StatusCode::BAD_REQUEST, "sign must be up, down or none").into_response()
}

fn is_htmx(headers: &HeaderMap) -> bool {
    header(headers, "hx-request").is_some_and(|v| v == "true")
}

/// The newest digest the item was in (its id and date) when the item exists, else 404. The
/// card carries only the item id (page size), so the digest is resolved here.
async fn target(state: &AppState, item: &str) -> Result<Option<(String, String)>, Box<Response>> {
    let item = item.to_string();
    let found = state
        .db
        .call(move |conn| {
            if repo::get_item(conn, &item)?.is_none() {
                return Ok(None);
            }
            let digest = match repo::latest_digest_for_item(conn, &item)? {
                Some(id) => repo::get_digest(conn, &id)?.map(|d| (d.id, d.date)),
                None => None,
            };
            Ok(Some(digest))
        })
        .await;
    match found {
        Err(e) => Err(Box::new(internal(e))),
        Ok(Some(digest)) => Ok(digest),
        Ok(None) => Err(Box::new(
            (StatusCode::NOT_FOUND, "unknown item").into_response(),
        )),
    }
}

pub async fn reasons(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<ReasonsQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(q)) = query else {
        return (StatusCode::BAD_REQUEST, "item and sign are required").into_response();
    };
    let sign = match parse_sign(&q.sign) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    if let Err(r) = target(&state, &q.item).await {
        return *r;
    }
    let partial = match sign {
        Some(sign) => rate::reasons(&q.item, sign),
        None => rate::slot(&q.item, None),
    };
    if is_htmx(&headers) {
        partial.into_response()
    } else {
        layout::page_with_css("Rate", rate::css(), partial).into_response()
    }
}

pub async fn rate(
    State(state): State<AppState>,
    headers: HeaderMap,
    form: Result<Form<RateForm>, FormRejection>,
) -> Response {
    if !is_same_origin(
        header(&headers, "sec-fetch-site"),
        header(&headers, "origin"),
        header(&headers, "host"),
    ) {
        tracing::warn!("rating refused: cross-origin request");
        return (StatusCode::FORBIDDEN, "cross-origin requests cannot rate").into_response();
    }
    let Ok(Form(f)) = form else {
        return (StatusCode::BAD_REQUEST, "item and sign are required").into_response();
    };
    let sign = match parse_sign(&f.sign) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let reason = match sign {
        None => None,
        Some(sign) => match f.reason.as_deref().and_then(Reason::parse) {
            Some(r) if r.sign() == sign => Some(r),
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    "reason must be one of the four for that sign",
                )
                    .into_response();
            }
        },
    };
    let digest = match target(&state, &f.item).await {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let at = to_iso((state.now)());
    let item = f.item.clone();
    let digest_id = digest.as_ref().map(|(id, _)| id.clone());
    let stored = state
        .db
        .call(move |conn| {
            match (sign, reason) {
                (Some(sign), Some(reason)) => repo::upsert_rating(
                    conn,
                    &item,
                    digest_id.as_deref(),
                    sign.as_str(),
                    reason.as_str(),
                    &at,
                )?,
                _ => {
                    repo::delete_rating(conn, &item)?;
                }
            }
            repo::get_rating(conn, &item)
        })
        .await;
    let rating = match stored {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    if is_htmx(&headers) {
        rate::slot(&f.item, rating.as_ref()).into_response()
    } else {
        let back = match digest {
            Some((_, date)) => format!("/d/{date}"),
            None => "/".to_string(),
        };
        (StatusCode::SEE_OTHER, [(header::LOCATION, back)]).into_response()
    }
}
