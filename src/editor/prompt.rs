//! The prompts are byte-stable across days (`SPEC.md` §1 #16): nothing run-specific goes into
//! them, so the harness can cache the system prompt and the trajectory is reproducible.

/// `true` when `text` contains a `YYYY-MM-DD` date.
pub fn contains_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.windows(10).any(|w| {
        w[..4].iter().all(u8::is_ascii_digit)
            && w[4] == b'-'
            && w[5..7].iter().all(u8::is_ascii_digit)
            && w[7] == b'-'
            && w[8..10].iter().all(u8::is_ascii_digit)
    })
}

/// `true` when `text` contains an `8-4-4-4-12` hex UUID.
pub fn contains_uuid(text: &str) -> bool {
    let b = text.as_bytes();
    let hex = |s: &[u8]| s.iter().all(u8::is_ascii_hexdigit);
    b.windows(36).any(|w| {
        hex(&w[0..8])
            && w[8] == b'-'
            && hex(&w[9..13])
            && w[13] == b'-'
            && hex(&w[14..18])
            && w[18] == b'-'
            && hex(&w[19..23])
            && w[23] == b'-'
            && hex(&w[24..36])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDITOR: &str = include_str!("../../prompts/editor.md");
    const SMOKE: &str = include_str!("../../prompts/smoke.md");
    const CURATOR: &str = include_str!("../../prompts/curator.md");

    #[test]
    fn curator_prompt_is_byte_stable_and_names_no_date() {
        assert!(
            !contains_date(CURATOR),
            "prompts/curator.md contains a date"
        );
        assert!(
            !contains_uuid(CURATOR),
            "prompts/curator.md contains a UUID"
        );
        for tool in [
            "get_feedback",
            "get_profile",
            "find_feeds",
            "validate_feed",
            "propose_change",
        ] {
            assert!(
                CURATOR.contains(tool),
                "{tool} missing from the Curator prompt"
            );
        }
        assert!(CURATOR.contains("at most 8 times"), "the web search cap");
        assert!(!CURATOR.contains("get_briefing") && !CURATOR.contains("publish_digest"));
    }

    #[test]
    fn editor_prompt_is_byte_stable() {
        assert!(!contains_date(EDITOR), "prompts/editor.md contains a date");
        assert!(!contains_uuid(EDITOR), "prompts/editor.md contains a UUID");
        assert!(EDITOR.contains("get_briefing"));
    }

    #[test]
    fn smoke_prompt_too() {
        assert!(!contains_date(SMOKE));
        assert!(!contains_uuid(SMOKE));
    }

    #[test]
    fn detectors_catch_dates_and_uuids() {
        assert!(contains_date("published 2026-09-17 morning"));
        assert!(!contains_date("version 12.3-4"));
        assert!(contains_uuid(
            "run 123e4567-e89b-12d3-a456-426614174000 done"
        ));
        assert!(!contains_uuid("2026-09-17-abcd1234"));
    }
}
