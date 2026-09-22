//! The approval queue (`spec/m3.md` approval-web): pending proposals first, each with its
//! typed change, its evidence as a definition list and its run, then the recent decisions.
//! Approve and reject are plain forms (htmx reloads the page on `HX-Refresh`, no script).

use maud::{Markup, html};

use super::layout::{page_with_css, runs_link};
use crate::core::feedback::{Proposal, ProposalChange};

pub const CSS: &str = "\
.proposal{padding:.75rem 0;border-bottom:1px solid var(--line)}\
.proposal h3{margin:0 0 .25rem;font-size:1.05rem}\
.proposal dl{display:grid;grid-template-columns:max-content 1fr;gap:.15rem .75rem;margin:.5rem 0;font-size:.9rem}\
.proposal dt{color:var(--muted)}.proposal dd{margin:0}\
.decide{display:flex;gap:.5rem;margin-top:.5rem}.decide form{display:contents}\
.approve{border-color:#2a7}.reject{border-color:#c44}\
.decided{color:var(--muted)}";

fn describe(change: &ProposalChange) -> String {
    match change {
        ProposalChange::TopicWeight { topic_id, weight } => {
            format!("Set topic '{topic_id}' weight to {weight}")
        }
        ProposalChange::AddTopic { name, weight, .. } => {
            format!("Add topic '{name}' at weight {weight}")
        }
        ProposalChange::DisableSource { source_id } => format!("Disable source '{source_id}'"),
        ProposalChange::AddSource { url, title } => format!("Add source '{title}' ({url})"),
        ProposalChange::PromoteExploreTopic { topic_id } => {
            format!("Promote explore topic '{topic_id}'")
        }
    }
}

fn evidence(p: &Proposal) -> Markup {
    let e = &p.evidence;
    let ids = |v: &[i64]| {
        v.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    html! {
        dl {
            dt { "Summary" } dd { (e.summary) }
            @if !e.rating_ids.is_empty() { dt { "Ratings" } dd { (ids(&e.rating_ids)) } }
            dt { "Reads" } dd { (e.read_count) }
            @if !e.feed_issue_ids.is_empty() { dt { "Feed issues" } dd { (ids(&e.feed_issue_ids)) } }
            @if !e.notes.is_empty() { dt { "Notes" } dd { (e.notes) } }
            @if let ProposalChange::AddTopic { description, .. } = &p.change {
                @if !description.is_empty() { dt { "Description" } dd { (description) } }
            }
            dt { "Proposed" } dd {
                (p.created_at)
                @if let Some(run) = &p.run_id { " · " a href={ "/runs/" (run) } { "run" } }
            }
            @if let Some(at) = &p.decided_at { dt { (p.status) } dd { (at) } }
            @if let Some(applied) = &p.applied { dt { "Applied" } dd { code { (applied) } } }
        }
    }
}

fn pending(p: &Proposal) -> Markup {
    html! {
        article.proposal id={ "p-" (p.id) } {
            h3 { (describe(&p.change)) " " span.badge { (p.change.kind()) } }
            (evidence(p))
            div.decide {
                form method="post" action={ "/curator/" (p.id) "/approve" } hx-post={ "/curator/" (p.id) "/approve" } hx-swap="none" {
                    button.approve type="submit" { "Approve" }
                }
                form method="post" action={ "/curator/" (p.id) "/reject" } hx-post={ "/curator/" (p.id) "/reject" } hx-swap="none" {
                    button.reject type="submit" { "Reject" }
                }
            }
        }
    }
}

fn decided(p: &Proposal) -> Markup {
    html! {
        article.proposal.decided id={ "p-" (p.id) } {
            h3 { (describe(&p.change)) " " span.badge { (p.status) } }
            (evidence(p))
        }
    }
}

pub fn render(pending_list: &[Proposal], decided_list: &[Proposal]) -> Markup {
    page_with_css(
        "Curator",
        CSS,
        html! {
            header { h1 { "Curator" } (runs_link()) }
            main {
                h2 { "Pending" }
                @if pending_list.is_empty() { p.state { "Nothing to approve." } }
                @for p in pending_list { (pending(p)) }
                h2 { "Decided" }
                @if decided_list.is_empty() { p.state { "No decisions yet." } }
                @for p in decided_list { (decided(p)) }
            }
        },
    )
}
