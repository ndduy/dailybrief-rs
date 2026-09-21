//! Duplicate detection (`SPEC.md` §4 `fetch_sources`): canonical URL, normalised title hash, and
//! cosine against recent items. Also the item id, derived from the canonical URL so re-ingesting
//! the same article is idempotent.

use sha2::{Digest, Sha256};

use super::vector::cosine;
use crate::db::{Connection, DbError, repo};

/// Query keys that only track the click, never identify the page.
fn is_tracking_param(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.starts_with("utm_")
        || k.starts_with("_hs")
        || matches!(
            k.as_str(),
            "fbclid" | "gclid" | "mc_cid" | "mc_eid" | "ref" | "source" | "igshid"
        )
}

/// Drop the fragment, lowercase the host, strip tracking params, sort the remaining query keys,
/// and strip one trailing slash from a non-root path. Unparsable input comes back trimmed.
pub fn canonical_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let Ok(mut url) = url::Url::parse(trimmed) else {
        return trimmed.to_string();
    };
    url.set_fragment(None);
    if let Some(host) = url.host_str().map(str::to_ascii_lowercase) {
        // `set_host` fails only for cannot-be-a-base URLs, which keep their host as parsed.
        let _ = url.set_host(Some(&host));
    }
    let mut kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    kept.sort_by(|a, b| a.0.cmp(&b.0));
    if kept.is_empty() {
        url.set_query(None);
    } else {
        let mut q = url.query_pairs_mut();
        q.clear();
        for (k, v) in &kept {
            q.append_pair(k, v);
        }
        drop(q);
    }
    let path = url.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        url.set_path(path.trim_end_matches('/'));
    }
    url.to_string()
}

fn sha256_hex(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lowercase, runs of non-alphanumeric characters (Unicode-aware) collapsed to one space, trimmed.
pub fn title_hash(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut in_gap = false;
    for c in title.to_lowercase().chars() {
        if c.is_alphanumeric() {
            if in_gap && !out.is_empty() {
                out.push(' ');
            }
            in_gap = false;
            out.push(c);
        } else {
            in_gap = true;
        }
    }
    sha256_hex(out.trim())
}

/// Whitespace runs collapsed to one space, trimmed.
pub fn content_hash(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    sha256_hex(&collapsed)
}

/// The first 16 hex characters of SHA-256 of the canonical URL.
pub fn item_id(canonical: &str) -> String {
    sha256_hex(canonical).chars().take(16).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Duplicate {
    Url,
    Title,
    Near { of: String, score: f32 },
}

/// URL first, then title (within the window), then the nearest vector above `threshold` in
/// `recent`, the `(id, vector)` projection of items fetched since `since` that the caller loads
/// once per batch (`repo::list_item_vectors_since`). `None` when the candidate is new (or has
/// no vector for the cosine step).
pub fn find_duplicate_against(
    conn: &Connection,
    canonical: &str,
    title_hash: &str,
    vector: Option<&[f32]>,
    threshold: f32,
    since: &str,
    recent: &[(String, Vec<f32>)],
) -> Result<Option<Duplicate>, DbError> {
    if repo::has_canonical_url(conn, canonical)? {
        return Ok(Some(Duplicate::Url));
    }
    if repo::has_title_hash_since(conn, title_hash, since)? {
        return Ok(Some(Duplicate::Title));
    }
    let Some(v) = vector else {
        return Ok(None);
    };
    let mut best: Option<(&str, f32)> = None;
    for (id, stored) in recent {
        let score = cosine(v, stored);
        if score > threshold && best.is_none_or(|(_, s)| score > s) {
            best = Some((id.as_str(), score));
        }
    }
    Ok(best.map(|(of, score)| Duplicate::Near {
        of: of.to_string(),
        score,
    }))
}

/// One candidate against the database: loads the projection itself. Batches use
/// [`find_duplicate_against`] with one load.
pub fn find_duplicate(
    conn: &Connection,
    canonical: &str,
    title_hash: &str,
    vector: Option<&[f32]>,
    threshold: f32,
    since: &str,
) -> Result<Option<Duplicate>, DbError> {
    let recent = if vector.is_some() {
        repo::list_item_vectors_since(conn, since)?
    } else {
        Vec::new()
    };
    find_duplicate_against(
        conn, canonical, title_hash, vector, threshold, since, &recent,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Feed;
    use crate::core::vector::DIMENSIONS;
    use crate::db::Db;
    use crate::db::repo::NewItem;

    #[test]
    fn canonical_url_table() {
        let cases = [
            ("https://Example.COM/a", "https://example.com/a"),
            ("https://x.example/a/#section", "https://x.example/a"),
            ("https://x.example/a/", "https://x.example/a"),
            ("https://x.example/", "https://x.example/"),
            ("https://x.example", "https://x.example/"),
            (
                "https://x.example/a?utm_source=t&utm_medium=m",
                "https://x.example/a",
            ),
            (
                "https://x.example/a?fbclid=1&id=7",
                "https://x.example/a?id=7",
            ),
            ("https://x.example/a?z=1&a=2", "https://x.example/a?a=2&z=1"),
            (
                "https://x.example/a?ref=hn&_hsenc=x&page=2",
                "https://x.example/a?page=2",
            ),
            (
                "https://x.example/a?UTM_CAMPAIGN=c&b=1",
                "https://x.example/a?b=1",
            ),
            ("  https://x.example/a  ", "https://x.example/a"),
            ("not a url at all", "not a url at all"),
            (
                "http://x.example:8080/p/?q=1#f",
                "http://x.example:8080/p?q=1",
            ),
            (
                "https://x.example/a?igshid=1&source=s&gclid=g",
                "https://x.example/a",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(canonical_url(input), want, "input {input:?}");
        }
    }

    #[test]
    fn title_hash_ignores_case_and_punctuation() {
        assert_eq!(title_hash("Hello, World!"), title_hash("hello world"));
        assert_eq!(title_hash("  Hello   world  "), title_hash("hello world"));
        assert_ne!(title_hash("hello world"), title_hash("hello there"));
    }

    #[test]
    fn title_hash_handles_vietnamese_diacritics() {
        assert_eq!(
            title_hash("Xin chào, thế giới!"),
            title_hash("xin chào thế giới")
        );
        assert_ne!(
            title_hash("xin chào thế giới"),
            title_hash("xin chao the gioi")
        );
    }

    #[test]
    fn content_hash_ignores_whitespace_runs() {
        assert_eq!(content_hash("a  b\n\n c"), content_hash("a b c"));
        assert_ne!(content_hash("a b c"), content_hash("a b d"));
        assert_eq!(content_hash("x").len(), 64);
    }

    #[test]
    fn item_id_is_16_hex_of_canonical_url() {
        let id = item_id("https://x.example/a");
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(id, item_id("https://x.example/a"));
        assert_ne!(id, item_id("https://x.example/b"));
    }

    fn seed(db: &Db, vector: Option<Vec<f32>>, fetched_at: &str) {
        db.with(|c| {
            repo::upsert_source(
                c,
                &Feed {
                    id: "s".into(),
                    url: "https://s.example/rss".into(),
                    title: "S".into(),
                    weight: 1.0,
                    enabled: true,
                },
            )?;
            repo::insert_item(
                c,
                &NewItem {
                    id: "existing".into(),
                    source_id: "s".into(),
                    url: "https://x.example/a".into(),
                    canonical_url: "https://x.example/a".into(),
                    title: "Existing".into(),
                    author: None,
                    published_at: None,
                    fetched_at: fetched_at.into(),
                    text: "t".into(),
                    word_count: 1,
                    content_hash: "c".into(),
                    title_hash: title_hash("Existing"),
                    vector,
                },
            )
        })
        .unwrap();
    }

    fn v(x: f32, y: f32) -> Vec<f32> {
        let mut out = vec![0.0; DIMENSIONS];
        out[0] = x;
        out[1] = y;
        out
    }

    const SINCE: &str = "2026-09-03T00:00:00.000Z";
    const RECENT: &str = "2026-09-16T00:00:00.000Z";

    #[test]
    fn find_duplicate_by_url() {
        let db = Db::open_in_memory().unwrap();
        seed(&db, None, RECENT);
        let d = db
            .with(|c| find_duplicate(c, "https://x.example/a", "other", None, 0.92, SINCE))
            .unwrap();
        assert_eq!(d, Some(Duplicate::Url));
    }

    #[test]
    fn find_duplicate_by_title() {
        let db = Db::open_in_memory().unwrap();
        seed(&db, None, RECENT);
        let d = db
            .with(|c| {
                find_duplicate(
                    c,
                    "https://x.example/new",
                    &title_hash("existing!"),
                    None,
                    0.92,
                    SINCE,
                )
            })
            .unwrap();
        assert_eq!(d, Some(Duplicate::Title));
    }

    #[test]
    fn find_duplicate_by_cosine_over_threshold() {
        let db = Db::open_in_memory().unwrap();
        seed(&db, Some(v(1.0, 0.0)), RECENT);
        let near = v(0.99, 0.14);
        let d = db
            .with(|c| {
                find_duplicate(
                    c,
                    "https://x.example/new",
                    "other",
                    Some(&near),
                    0.92,
                    SINCE,
                )
            })
            .unwrap();
        match d {
            Some(Duplicate::Near { of, score }) => {
                assert_eq!(of, "existing");
                assert!(score > 0.92);
            }
            other => panic!("expected Near, got {other:?}"),
        }
        let far = v(0.0, 1.0);
        let d = db
            .with(|c| find_duplicate(c, "https://x.example/new", "other", Some(&far), 0.92, SINCE))
            .unwrap();
        assert_eq!(d, None);
    }

    #[test]
    fn find_duplicate_none_when_vector_missing() {
        let db = Db::open_in_memory().unwrap();
        seed(&db, Some(v(1.0, 0.0)), RECENT);
        let d = db
            .with(|c| find_duplicate(c, "https://x.example/new", "other", None, 0.92, SINCE))
            .unwrap();
        assert_eq!(d, None);
    }

    #[test]
    fn find_duplicate_ignores_items_older_than_window() {
        let db = Db::open_in_memory().unwrap();
        seed(&db, Some(v(1.0, 0.0)), "2026-08-01T00:00:00.000Z");
        let same = v(1.0, 0.0);
        let d = db
            .with(|c| {
                find_duplicate(
                    c,
                    "https://x.example/new",
                    "other",
                    Some(&same),
                    0.92,
                    SINCE,
                )
            })
            .unwrap();
        assert_eq!(d, None);
    }

    /// The batch variant compares only against the projection it is handed, so one load per
    /// batch is the whole cost; the per-candidate wrapper loads for itself.
    #[test]
    fn find_duplicate_loads_vectors_once_per_batch() {
        let db = Db::open_in_memory().unwrap();
        seed(&db, Some(v(1.0, 0.0)), RECENT);
        let same = v(1.0, 0.0);
        let (without, with) = db
            .with(|c| {
                let recent = repo::list_item_vectors_since(c, SINCE)?;
                assert_eq!(recent.len(), 1);
                let without = find_duplicate_against(
                    c,
                    "https://x.example/new",
                    "other",
                    Some(&same),
                    0.92,
                    SINCE,
                    &[],
                )?;
                let with = find_duplicate_against(
                    c,
                    "https://x.example/new",
                    "other",
                    Some(&same),
                    0.92,
                    SINCE,
                    &recent,
                )?;
                Ok((without, with))
            })
            .unwrap();
        assert_eq!(without, None, "no query behind the caller's back");
        assert!(matches!(with, Some(Duplicate::Near { ref of, .. }) if of == "existing"));
    }
}
