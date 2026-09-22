//! Ratings and proposals (`spec/m3.md` §10): only `core::feedback::apply` writes topic and
//! source state, and a rating changes nothing but the `ratings` table.

use std::path::PathBuf;

use dailybrief::db::Db;
use dailybrief::db::repo;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every caller of the five writers, outside the repo itself and `core::feedback`, is a
/// boundary violation (`SPEC.md` §1 #11: nothing changes the profile without approval).
#[test]
fn only_feedback_apply_writes_topics_and_sources() {
    let writers = [
        "set_topic_weight(",
        "set_topic_origin(",
        "insert_topic_from_proposal(",
        "set_source_enabled(",
        "insert_source_from_proposal(",
    ];
    let mut offenders = Vec::new();
    for entry in walkdir(&root().join("src")) {
        let rel = entry
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if rel == "src/db/repo.rs" || rel == "src/core/feedback.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&entry).unwrap();
        for w in writers {
            if text.contains(w) {
                offenders.push(format!("{rel}: {w}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "writers called outside core::feedback: {offenders:?}"
    );
}

fn walkdir(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out
}

#[test]
fn ratings_upsert_replace_and_delete() {
    let db = Db::open_in_memory().unwrap();
    db.with(|c| {
        repo::upsert_source(
            c,
            &dailybrief::config::Feed {
                id: "s".into(),
                url: "https://s.example/rss".into(),
                title: "S".into(),
                weight: 1.0,
                enabled: true,
            },
        )?;
        repo::insert_item(
            c,
            &repo::NewItem {
                id: "i1".into(),
                source_id: "s".into(),
                url: "https://s.example/1".into(),
                canonical_url: "https://s.example/1".into(),
                title: "One".into(),
                author: None,
                published_at: None,
                fetched_at: "2026-09-22T00:00:00.000Z".into(),
                text: "body".into(),
                word_count: 1,
                content_hash: "c1".into(),
                title_hash: "t1".into(),
                vector: None,
            },
        )?;
        repo::upsert_rating(c, "i1", None, "up", "new_to_me", "2026-09-22T09:00:00.000Z")?;
        repo::upsert_rating(
            c,
            "i1",
            None,
            "down",
            "too_shallow",
            "2026-09-22T09:01:00.000Z",
        )?;
        let r = repo::get_rating(c, "i1")?.unwrap();
        assert_eq!(
            (r.sign.as_str(), r.reason.as_str()),
            ("down", "too_shallow")
        );
        let all = repo::list_ratings_since(c, "2026-09-22T00:00:00.000Z")?;
        assert_eq!(all.len(), 1, "one rating per item");
        assert_eq!(repo::delete_rating(c, "i1")?, 1);
        assert!(repo::get_rating(c, "i1")?.is_none());
        let counts = repo::row_counts(c)?;
        assert!(
            counts.contains(&("topics", 0)),
            "a rating touches nothing else: {counts:?}"
        );
        Ok(())
    })
    .unwrap();
}
