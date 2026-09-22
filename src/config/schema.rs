//! TOML shapes (`*File`, exact keys, unknown keys rejected) and the validated types.
//! Every default here is the value in the committed `config/config.toml`; change that file first.

use std::path::PathBuf;
use std::str::FromStr;

use serde::Deserialize;

use super::ConfigError;

// ---------- config.toml ----------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub service: ServiceFile,
    #[serde(default)]
    pub schedule: ScheduleFile,
    #[serde(default)]
    pub caps: CapsFile,
    #[serde(default)]
    pub ingest: IngestFile,
    #[serde(default)]
    pub embeddings: EmbeddingsFile,
    #[serde(default)]
    pub harness: HarnessFile,
    #[serde(default)]
    pub retention: RetentionFile,
}

/// `[retention]`: run directories and `run_events` older than `days` are pruned by the daily
/// job at `cron` (service timezone); `runs` rows are never pruned (ADR 0013).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionFile {
    #[serde(default = "d_retention_days")]
    pub days: u32,
    #[serde(default = "d_retention_cron")]
    pub cron: String,
}
impl Default for RetentionFile {
    fn default() -> Self {
        Self {
            days: d_retention_days(),
            cron: d_retention_cron(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceFile {
    #[serde(default = "d_bind")]
    pub bind: String,
    #[serde(default = "d_port")]
    pub port: u16,
    pub timezone: String,
    #[serde(default = "d_data_dir")]
    pub data_dir: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleFile {
    #[serde(default = "d_cron")]
    pub cron: String,
}
impl Default for ScheduleFile {
    fn default() -> Self {
        Self { cron: d_cron() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapsFile {
    #[serde(default = "d_reads")]
    pub reads: u32,
    #[serde(default = "d_for_you")]
    pub for_you: u32,
    #[serde(default = "d_beyond_radar")]
    pub beyond_radar: u32,
    #[serde(default = "d_per_source")]
    pub per_source: u32,
    #[serde(default = "d_per_topic")]
    pub per_topic: u32,
    #[serde(default = "d_web_search")]
    pub web_search: u32,
    #[serde(default = "d_read_text_chars")]
    pub read_text_chars: u32,
    #[serde(default = "d_shown_days")]
    pub shown_days: u32,
    #[serde(default = "d_candidate_days")]
    pub candidate_days: u32,
}
impl Default for CapsFile {
    fn default() -> Self {
        Self {
            reads: d_reads(),
            for_you: d_for_you(),
            beyond_radar: d_beyond_radar(),
            per_source: d_per_source(),
            per_topic: d_per_topic(),
            web_search: d_web_search(),
            read_text_chars: d_read_text_chars(),
            shown_days: d_shown_days(),
            candidate_days: d_candidate_days(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestFile {
    #[serde(default = "d_concurrency")]
    pub concurrency: u32,
    #[serde(default = "d_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "d_total_budget_ms")]
    pub total_budget_ms: u64,
    #[serde(default = "d_body_max_bytes")]
    pub body_max_bytes: u64,
    #[serde(default = "d_max_redirects")]
    pub max_redirects: u32,
    #[serde(default = "d_dedupe_cosine")]
    pub dedupe_cosine: f32,
    /// Tests only: lets the fetcher reach a mock on 127.0.0.1. Never read from the file
    /// (`deny_unknown_fields` refuses the key), so production cannot switch the guard off.
    #[serde(skip)]
    pub allow_loopback: bool,
}
impl Default for IngestFile {
    fn default() -> Self {
        Self {
            concurrency: d_concurrency(),
            request_timeout_ms: d_request_timeout_ms(),
            total_budget_ms: d_total_budget_ms(),
            body_max_bytes: d_body_max_bytes(),
            max_redirects: d_max_redirects(),
            dedupe_cosine: d_dedupe_cosine(),
            allow_loopback: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingsFile {
    #[serde(default = "d_model")]
    pub model: String,
    #[serde(default = "d_dimensions")]
    pub dimensions: u32,
}
impl Default for EmbeddingsFile {
    fn default() -> Self {
        Self {
            model: d_model(),
            dimensions: d_dimensions(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessFile {
    #[serde(default)]
    pub default: HarnessName,
    /// Arguments written into each run's `mcp.json` after the service's own executable
    /// (`SPEC.md` §3: the harness spawns `current_exe()`, never a configured command).
    #[serde(default = "d_mcp_args")]
    pub mcp_args: Vec<String>,
    #[serde(default, rename = "claude-code")]
    pub claude_code: ClaudeCodeFile,
}
impl Default for HarnessFile {
    fn default() -> Self {
        Self {
            default: HarnessName::default(),
            mcp_args: d_mcp_args(),
            claude_code: ClaudeCodeFile::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeCodeFile {
    #[serde(default = "d_model_alias")]
    pub model: String,
    #[serde(default = "d_max_turns")]
    pub max_turns: u32,
    #[serde(default = "d_wall_clock_minutes")]
    pub wall_clock_minutes: u32,
}
impl Default for ClaudeCodeFile {
    fn default() -> Self {
        Self {
            model: d_model_alias(),
            max_turns: d_max_turns(),
            wall_clock_minutes: d_wall_clock_minutes(),
        }
    }
}

fn d_bind() -> String {
    "127.0.0.1".to_string()
}
fn d_port() -> u16 {
    8788
}
fn d_data_dir() -> String {
    "data".to_string()
}
fn d_cron() -> String {
    "30 6 * * *".to_string()
}
fn d_retention_days() -> u32 {
    60
}
fn d_retention_cron() -> String {
    "0 7 * * *".to_string()
}
/// Below this the window would not even cover a week of failed mornings.
pub const MIN_RETENTION_DAYS: u32 = 7;
fn d_reads() -> u32 {
    45
}
fn d_for_you() -> u32 {
    24
}
fn d_beyond_radar() -> u32 {
    6
}
fn d_per_source() -> u32 {
    4
}
fn d_per_topic() -> u32 {
    8
}
fn d_web_search() -> u32 {
    5
}
fn d_read_text_chars() -> u32 {
    5000
}
fn d_shown_days() -> u32 {
    14
}
fn d_candidate_days() -> u32 {
    7
}
fn d_concurrency() -> u32 {
    6
}
fn d_request_timeout_ms() -> u64 {
    15_000
}
fn d_total_budget_ms() -> u64 {
    120_000
}
fn d_body_max_bytes() -> u64 {
    2_097_152
}
fn d_max_redirects() -> u32 {
    3
}
fn d_dedupe_cosine() -> f32 {
    0.92
}
fn d_model() -> String {
    "Xenova/bge-small-en-v1.5".to_string()
}
fn d_dimensions() -> u32 {
    384
}
fn d_mcp_args() -> Vec<String> {
    vec!["mcp".to_string()]
}
fn d_model_alias() -> String {
    "opus".to_string()
}
fn d_max_turns() -> u32 {
    120
}
fn d_wall_clock_minutes() -> u32 {
    15
}

// ---------- feeds.toml / topics.toml ----------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedsFile {
    #[serde(default)]
    pub feeds: Vec<FeedEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedEntry {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    #[serde(default = "one")]
    pub weight: f64,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicsFile {
    #[serde(default)]
    pub topics: Vec<TopicEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "one")]
    pub weight: f64,
    #[serde(default)]
    pub origin: TopicOrigin,
}

fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}

// ---------- validated types ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessName {
    #[default]
    ClaudeCode,
}

impl HarnessName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TopicOrigin {
    #[default]
    Seed,
    Curator,
    ExplorePromoted,
}

impl TopicOrigin {
    /// The value stored in `topics.origin` (`SPEC.md` §5 CHECK constraint).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::Curator => "curator",
            Self::ExplorePromoted => "explore-promoted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub bind: String,
    pub port: u16,
    pub timezone: String,
    /// Resolved by `load_config`: the env override or the file value, relative to the cwd.
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub cron: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retention {
    pub days: u32,
    pub cron: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    pub reads: u32,
    pub for_you: u32,
    pub beyond_radar: u32,
    pub per_source: u32,
    pub per_topic: u32,
    pub web_search: u32,
    pub read_text_chars: u32,
    pub shown_days: u32,
    pub candidate_days: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ingest {
    pub concurrency: u32,
    pub request_timeout_ms: u64,
    pub total_budget_ms: u64,
    pub body_max_bytes: u64,
    pub max_redirects: u32,
    pub dedupe_cosine: f32,
    /// Only tests set this (see the file field of the same name).
    pub allow_loopback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embeddings {
    pub model: String,
    pub dimensions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessSettings {
    pub default: HarnessName,
    pub mcp_args: Vec<String>,
    pub claude_code: ClaudeCodeSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeSettings {
    pub model: String,
    pub max_turns: u32,
    pub wall_clock_minutes: u32,
}

/// Every file path the service reads, derived from the config file's location (`load::resolve_paths`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Paths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub feeds: PathBuf,
    pub topics: PathBuf,
    pub mcp_template: PathBuf,
    pub editor_prompt: PathBuf,
    pub digest_schema: PathBuf,
    pub curator_prompt: PathBuf,
    pub curator_schema: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub service: Service,
    pub schedule: Schedule,
    pub caps: Caps,
    pub ingest: Ingest,
    pub embeddings: Embeddings,
    pub harness: HarnessSettings,
    pub retention: Retention,
    pub paths: Paths,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Feed {
    pub id: String,
    pub url: String,
    pub title: String,
    pub weight: f64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Topic {
    pub id: String,
    pub name: String,
    pub description: String,
    pub weight: f64,
    pub origin: TopicOrigin,
}

// ---------- validation ----------

fn invalid(file: &'static str, message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        file,
        message: message.into(),
    }
}

fn positive(file: &'static str, field: &str, value: u64) -> Result<(), ConfigError> {
    if value == 0 {
        return Err(invalid(file, format!("{field} must be greater than 0")));
    }
    Ok(())
}

impl TryFrom<ConfigFile> for Config {
    type Error = ConfigError;

    fn try_from(f: ConfigFile) -> Result<Self, Self::Error> {
        const FILE: &str = "config.toml";
        if f.service.bind.trim().is_empty() {
            return Err(invalid(FILE, "service.bind must not be empty"));
        }
        if f.service.port == 0 {
            return Err(invalid(FILE, "service.port must be 1..=65535"));
        }
        if chrono_tz::Tz::from_str(&f.service.timezone).is_err() {
            return Err(invalid(
                FILE,
                format!(
                    "service.timezone '{}' is not an IANA timezone",
                    f.service.timezone
                ),
            ));
        }
        if let Err(e) = croner::Cron::from_str(&f.schedule.cron) {
            return Err(invalid(
                FILE,
                format!("schedule.cron '{}' is invalid: {e}", f.schedule.cron),
            ));
        }
        if let Err(e) = croner::Cron::from_str(&f.retention.cron) {
            return Err(invalid(
                FILE,
                format!("retention.cron '{}' is invalid: {e}", f.retention.cron),
            ));
        }
        if f.retention.days < MIN_RETENTION_DAYS {
            return Err(invalid(
                FILE,
                format!(
                    "retention.days must be at least {MIN_RETENTION_DAYS}, got {}",
                    f.retention.days
                ),
            ));
        }
        let c = &f.caps;
        for (name, value) in [
            ("caps.reads", c.reads),
            ("caps.for_you", c.for_you),
            ("caps.beyond_radar", c.beyond_radar),
            ("caps.per_source", c.per_source),
            ("caps.per_topic", c.per_topic),
            ("caps.read_text_chars", c.read_text_chars),
            ("caps.shown_days", c.shown_days),
            ("caps.candidate_days", c.candidate_days),
            ("ingest.concurrency", f.ingest.concurrency),
            ("embeddings.dimensions", f.embeddings.dimensions),
            (
                "harness.claude-code.max_turns",
                f.harness.claude_code.max_turns,
            ),
            (
                "harness.claude-code.wall_clock_minutes",
                f.harness.claude_code.wall_clock_minutes,
            ),
        ] {
            positive(FILE, name, u64::from(value))?;
        }
        for (name, value) in [
            ("ingest.request_timeout_ms", f.ingest.request_timeout_ms),
            ("ingest.total_budget_ms", f.ingest.total_budget_ms),
            ("ingest.body_max_bytes", f.ingest.body_max_bytes),
        ] {
            positive(FILE, name, value)?;
        }
        if !(0.0..=1.0).contains(&f.ingest.dedupe_cosine) {
            return Err(invalid(FILE, "ingest.dedupe_cosine must be within 0..=1"));
        }
        if f.embeddings.model.trim().is_empty() {
            return Err(invalid(FILE, "embeddings.model must not be empty"));
        }
        if f.harness.claude_code.model.trim().is_empty() {
            return Err(invalid(FILE, "harness.claude-code.model must not be empty"));
        }
        Ok(Self {
            service: Service {
                bind: f.service.bind,
                port: f.service.port,
                timezone: f.service.timezone,
                data_dir: PathBuf::from(f.service.data_dir),
            },
            schedule: Schedule {
                cron: f.schedule.cron,
            },
            retention: Retention {
                days: f.retention.days,
                cron: f.retention.cron,
            },
            caps: Caps {
                reads: c.reads,
                for_you: c.for_you,
                beyond_radar: c.beyond_radar,
                per_source: c.per_source,
                per_topic: c.per_topic,
                web_search: c.web_search,
                read_text_chars: c.read_text_chars,
                shown_days: c.shown_days,
                candidate_days: c.candidate_days,
            },
            ingest: Ingest {
                concurrency: f.ingest.concurrency,
                request_timeout_ms: f.ingest.request_timeout_ms,
                total_budget_ms: f.ingest.total_budget_ms,
                body_max_bytes: f.ingest.body_max_bytes,
                max_redirects: f.ingest.max_redirects,
                dedupe_cosine: f.ingest.dedupe_cosine,
                allow_loopback: f.ingest.allow_loopback,
            },
            embeddings: Embeddings {
                model: f.embeddings.model,
                dimensions: f.embeddings.dimensions,
            },
            harness: HarnessSettings {
                default: f.harness.default,
                mcp_args: f.harness.mcp_args,
                claude_code: ClaudeCodeSettings {
                    model: f.harness.claude_code.model,
                    max_turns: f.harness.claude_code.max_turns,
                    wall_clock_minutes: f.harness.claude_code.wall_clock_minutes,
                },
            },
            paths: Paths::default(),
        })
    }
}

/// Ids are `[a-z0-9][a-z0-9_-]{0,63}`: safe in URLs, file names and SQL text columns.
pub fn is_valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    id.len() <= 64
        && first.is_ascii_lowercase() | first.is_ascii_digit()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn check_ids<'a>(
    file: &'static str,
    ids: impl Iterator<Item = &'a str>,
) -> Result<(), ConfigError> {
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if !is_valid_id(id) {
            return Err(invalid(
                file,
                format!("id '{id}' must match [a-z0-9][a-z0-9_-]{{0,63}}"),
            ));
        }
        if !seen.insert(id) {
            return Err(invalid(file, format!("duplicate id '{id}'")));
        }
    }
    Ok(())
}

impl TryFrom<FeedsFile> for Vec<Feed> {
    type Error = ConfigError;

    fn try_from(f: FeedsFile) -> Result<Self, Self::Error> {
        const FILE: &str = "feeds.toml";
        check_ids(FILE, f.feeds.iter().map(|e| e.id.as_str()))?;
        f.feeds
            .into_iter()
            .map(|e| {
                let parsed = url::Url::parse(&e.url).map_err(|err| {
                    invalid(
                        FILE,
                        format!("feed '{}': url '{}' is invalid: {err}", e.id, e.url),
                    )
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(invalid(
                        FILE,
                        format!("feed '{}': url must be http(s), got '{}'", e.id, e.url),
                    ));
                }
                if e.weight.is_nan() || e.weight < 0.0 {
                    return Err(invalid(
                        FILE,
                        format!("feed '{}': weight must be >= 0", e.id),
                    ));
                }
                let title = match e.title.map(|t| t.trim().to_string()) {
                    Some(t) if !t.is_empty() => t,
                    _ => e.id.clone(),
                };
                Ok(Feed {
                    id: e.id,
                    url: e.url,
                    title,
                    weight: e.weight,
                    enabled: e.enabled,
                })
            })
            .collect()
    }
}

impl TryFrom<TopicsFile> for Vec<Topic> {
    type Error = ConfigError;

    fn try_from(f: TopicsFile) -> Result<Self, Self::Error> {
        const FILE: &str = "topics.toml";
        check_ids(FILE, f.topics.iter().map(|e| e.id.as_str()))?;
        f.topics
            .into_iter()
            .map(|e| {
                if e.name.trim().is_empty() {
                    return Err(invalid(
                        FILE,
                        format!("topic '{}': name must not be empty", e.id),
                    ));
                }
                if e.weight.is_nan() || e.weight < 0.0 {
                    return Err(invalid(
                        FILE,
                        format!("topic '{}': weight must be >= 0", e.id),
                    ));
                }
                Ok(Topic {
                    id: e.id,
                    name: e.name.trim().to_string(),
                    description: e.description.trim().to_string(),
                    weight: e.weight,
                    origin: e.origin,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_rules() {
        assert!(is_valid_id("a"));
        assert!(is_valid_id("hn-front_page9"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("-lead"));
        assert!(!is_valid_id("Caps"));
        assert!(!is_valid_id("has space"));
        assert!(!is_valid_id(&"a".repeat(65)));
    }

    #[test]
    fn origin_strings_match_the_check_constraint() {
        assert_eq!(TopicOrigin::Seed.as_str(), "seed");
        assert_eq!(TopicOrigin::Curator.as_str(), "curator");
        assert_eq!(TopicOrigin::ExplorePromoted.as_str(), "explore-promoted");
        assert_eq!(HarnessName::ClaudeCode.as_str(), "claude-code");
    }

    #[test]
    fn rejects_zero_caps_and_out_of_range_cosine() {
        let zero: ConfigFile =
            toml::from_str("[service]\ntimezone = \"UTC\"\n[caps]\nreads = 0\n").unwrap();
        assert!(
            Config::try_from(zero)
                .unwrap_err()
                .to_string()
                .contains("caps.reads")
        );
        let cos: ConfigFile =
            toml::from_str("[service]\ntimezone = \"UTC\"\n[ingest]\ndedupe_cosine = 1.5\n")
                .unwrap();
        assert!(
            Config::try_from(cos)
                .unwrap_err()
                .to_string()
                .contains("dedupe_cosine")
        );
    }
}
