//! Article HTML → clean text (`dom_smoothie`, a Readability port; `readability` as fallback).
//! A page with no extractable body yields `None`, and ingest skips it.

use dom_smoothie::{Config, Readability, TextMode};

/// Fewer words than this and the page is not an article (a link directory, an error page).
pub const MIN_WORDS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub title: String,
    pub byline: Option<String>,
    pub text: String,
    pub word_count: usize,
}

/// Words are whitespace-separated tokens; the same rule caps summaries (`SPEC.md` §4).
pub fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Trims every line, drops trailing spaces, and collapses runs of blank lines to one.
pub fn normalize_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank_run = 0;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            blank_run += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank_run > 0 {
                out.push('\n');
            }
        }
        blank_run = 0;
        out.push_str(&line.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    out
}

/// Extracts the article; `None` when neither extractor finds at least `MIN_WORDS` words.
pub fn extract(html: &str, url: Option<&str>) -> Option<Extracted> {
    primary(html, url)
        .or_else(|| fallback(html, url))
        .filter(|e| e.word_count >= MIN_WORDS)
}

fn primary(html: &str, url: Option<&str>) -> Option<Extracted> {
    let cfg = Config {
        text_mode: TextMode::Formatted,
        ..Config::default()
    };
    let mut r = Readability::new(html, url, Some(cfg)).ok()?;
    let article = r.parse().ok()?;
    let text = normalize_text(&article.text_content);
    Some(Extracted {
        title: article.title.trim().to_string(),
        byline: article
            .byline
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty()),
        word_count: count_words(&text),
        text,
    })
}

fn fallback(html: &str, url: Option<&str>) -> Option<Extracted> {
    let base = url::Url::parse(url.unwrap_or("https://localhost/")).ok()?;
    let mut cursor = std::io::Cursor::new(html.as_bytes());
    let product = readability::extractor::extract(&mut cursor, &base).ok()?;
    let text = normalize_text(&product.text);
    Some(Extracted {
        title: product.title.trim().to_string(),
        byline: None,
        word_count: count_words(&text),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: &str = include_str!("../../tests/fixtures/html/long.html");
    const SHORT: &str = include_str!("../../tests/fixtures/html/short.html");
    const NOBODY: &str = include_str!("../../tests/fixtures/html/nobody.html");

    #[test]
    fn extracts_long_article_title_and_body() {
        let e = extract(LONG, Some("https://blog.example/long")).unwrap();
        assert_eq!(e.title, "Long article about a small service");
        assert!(e.word_count >= 800, "{}", e.word_count);
        assert!(e.text.contains("The borrow checker"));
        for token in ["NAVTOKEN", "SIDEBARTOKEN", "FOOTERTOKEN"] {
            assert!(!e.text.contains(token), "{token} leaked into the body");
        }
    }

    #[test]
    fn extracts_short_article() {
        let e = extract(SHORT, Some("https://blog.example/short")).unwrap();
        assert_eq!(e.title, "Short note");
        assert!((100..=200).contains(&e.word_count), "{}", e.word_count);
        assert!(!e.text.contains("FOOTERTOKEN"));
    }

    #[test]
    fn returns_none_for_no_body() {
        assert_eq!(extract(NOBODY, Some("https://blog.example/links")), None);
        assert_eq!(extract("", None), None);
        assert_eq!(extract("<html><body></body></html>", None), None);
    }

    #[test]
    fn word_count_counts_unicode_words() {
        assert_eq!(count_words("Xin chào thế giới hôm nay"), 6);
        assert_eq!(count_words("  a\tb\n\nc  "), 3);
        assert_eq!(count_words(""), 0);
    }

    #[test]
    fn text_is_whitespace_normalised() {
        let n = normalize_text("  a   b \n\n\n\n c  \n   \n d ");
        assert_eq!(n, "a b\n\nc\n\nd");
        let e = extract(LONG, None).unwrap();
        assert!(!e.text.contains("\n\n\n"));
        assert!(!e.text.lines().any(|l| l.ends_with(' ')));
    }
}
