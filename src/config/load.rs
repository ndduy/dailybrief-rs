//! Reading the three TOML files and the environment into validated config.

use std::path::{Path, PathBuf};

use super::ConfigError;
use super::schema::{Config, ConfigFile, Feed, FeedsFile, Paths, Topic, TopicsFile};

/// The `DAILYBRIEF_*` environment, read once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    /// Path to `config.toml`; default `config/config.toml`.
    pub config_path: PathBuf,
    pub data_dir: Option<PathBuf>,
    pub bind: Option<String>,
    pub run_id: Option<String>,
    /// `DAILYBRIEF_RUN_ROLE`: `editor` (default when unset) or `curator`; the `mcp` verb reads it.
    pub run_role: Option<String>,
    pub in_container: bool,
}

impl Env {
    /// Reads `DAILYBRIEF_CONFIG`, `DAILYBRIEF_DATA_DIR`, `DAILYBRIEF_BIND`, `DAILYBRIEF_RUN_ID`,
    /// `DAILYBRIEF_RUN_ROLE`, `DAILYBRIEF_IN_CONTAINER` from the process environment.
    pub fn from_process() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Builds an `Env` from any lookup function (tests pass a map).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let non_empty = |name: &'static str| -> Result<Option<String>, ConfigError> {
            match lookup(name) {
                Some(v) if v.trim().is_empty() => Err(ConfigError::Env {
                    name,
                    message: "is set but empty".to_string(),
                }),
                other => Ok(other),
            }
        };
        let in_container = matches!(
            lookup("DAILYBRIEF_IN_CONTAINER").as_deref().map(str::trim),
            Some("1") | Some("true")
        );
        Ok(Self {
            config_path: non_empty("DAILYBRIEF_CONFIG")?
                .map_or_else(|| PathBuf::from("config/config.toml"), PathBuf::from),
            data_dir: non_empty("DAILYBRIEF_DATA_DIR")?.map(PathBuf::from),
            bind: non_empty("DAILYBRIEF_BIND")?,
            run_id: non_empty("DAILYBRIEF_RUN_ID")?,
            run_role: non_empty("DAILYBRIEF_RUN_ROLE")?,
            in_container,
        })
    }
}

/// Everything a command needs from disk and env, validated.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub feeds: Vec<Feed>,
    pub topics: Vec<Topic>,
}

fn read(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn parse<T: serde::de::DeserializeOwned>(path: &Path, text: &str) -> Result<T, ConfigError> {
    toml::from_str(text).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// Derives every path from the config file's location: `feeds.toml` and `topics.toml` sit next to
/// it, `prompts/` and `schemas/` sit in its parent's parent (the project root), and `data_dir`
/// resolves against the current working directory unless the env overrides it.
pub fn resolve_paths(config_path: &Path, data_dir: &Path, env: &Env) -> Paths {
    let config_dir = config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let root = config_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let data_dir = env
        .data_dir
        .clone()
        .unwrap_or_else(|| data_dir.to_path_buf());
    Paths {
        root: root.clone(),
        config: config_path.to_path_buf(),
        feeds: config_dir.join("feeds.toml"),
        topics: config_dir.join("topics.toml"),
        mcp_template: config_dir.join("mcp.json"),
        editor_prompt: root.join("prompts/editor.md"),
        digest_schema: root.join("schemas/digest.json"),
        data_dir,
    }
}

/// Loads and validates `config.toml`, applying env overrides for `data_dir` and `bind`.
pub fn load_config(env: &Env) -> Result<Config, ConfigError> {
    let path = &env.config_path;
    let file: ConfigFile = parse(path, &read(path)?)?;
    let mut config = Config::try_from(file)?;
    config.paths = resolve_paths(path, &config.service.data_dir, env);
    config.service.data_dir = config.paths.data_dir.clone();
    if let Some(bind) = &env.bind {
        config.service.bind = bind.clone();
    }
    Ok(config)
}

/// Loads and validates `feeds.toml`.
pub fn load_feeds(path: &Path) -> Result<Vec<Feed>, ConfigError> {
    let file: FeedsFile = parse(path, &read(path)?)?;
    Vec::<Feed>::try_from(file)
}

/// Loads and validates `topics.toml`.
pub fn load_topics(path: &Path) -> Result<Vec<Topic>, ConfigError> {
    let file: TopicsFile = parse(path, &read(path)?)?;
    Vec::<Topic>::try_from(file)
}

/// Loads all three files.
pub fn load_all(env: &Env) -> Result<Loaded, ConfigError> {
    let config = load_config(env)?;
    let feeds = load_feeds(&config.paths.feeds)?;
    let topics = load_topics(&config.paths.topics)?;
    Ok(Loaded {
        config,
        feeds,
        topics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_with(vars: &[(&str, &str)]) -> Env {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        Env::from_lookup(|name| map.get(name).cloned()).unwrap()
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    const MINIMAL_CONFIG: &str = "[service]\ntimezone = \"Asia/Ho_Chi_Minh\"\n";

    #[test]
    fn loads_committed_files() {
        let env = env_with(&[]);
        let loaded = load_all(&env).expect("committed config/ must load");
        assert_eq!(loaded.config.service.port, 8788);
        assert_eq!(loaded.config.service.timezone, "Asia/Ho_Chi_Minh");
        assert_eq!(loaded.config.schedule.cron, "30 6 * * *");
        assert!(!loaded.feeds.is_empty(), "feeds.toml has entries");
        assert!(!loaded.topics.is_empty(), "topics.toml has entries");
        assert!(
            loaded
                .config
                .paths
                .editor_prompt
                .ends_with("prompts/editor.md")
        );
    }

    #[test]
    fn defaults_match_committed_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "config/config.toml", MINIMAL_CONFIG);
        let env = env_with(&[("DAILYBRIEF_CONFIG", path.to_str().unwrap())]);
        let from_minimal = load_config(&env).unwrap();
        let committed = load_config(&env_with(&[])).unwrap();
        assert_eq!(from_minimal.caps, committed.caps);
        assert_eq!(from_minimal.ingest, committed.ingest);
        assert_eq!(from_minimal.embeddings, committed.embeddings);
        assert_eq!(from_minimal.harness, committed.harness);
        assert_eq!(from_minimal.schedule, committed.schedule);
        assert_eq!(from_minimal.service.bind, committed.service.bind);
        assert_eq!(from_minimal.service.port, committed.service.port);
    }

    #[test]
    fn rejects_unknown_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "config/config.toml",
            "[service]\ntimezone = \"UTC\"\ncolour = \"blue\"\n",
        );
        let env = env_with(&[("DAILYBRIEF_CONFIG", path.to_str().unwrap())]);
        let err = load_config(&env).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
        assert!(err.to_string().contains("colour"), "{err}");
    }

    #[test]
    fn rejects_bad_cron() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "config/config.toml",
            "[service]\ntimezone = \"UTC\"\n[schedule]\ncron = \"30 6 * *\"\n",
        );
        let env = env_with(&[("DAILYBRIEF_CONFIG", path.to_str().unwrap())]);
        let err = load_config(&env).unwrap_err();
        assert!(err.to_string().contains("cron"), "{err}");
    }

    #[test]
    fn retention_days_below_seven_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "config/config.toml",
            "[service]\ntimezone = \"UTC\"\n[retention]\ndays = 3\n",
        );
        let env = env_with(&[("DAILYBRIEF_CONFIG", path.to_str().unwrap())]);
        let err = load_config(&env).unwrap_err();
        assert!(err.to_string().contains("retention.days"), "{err}");
        let ok = write(
            dir.path(),
            "config/config.toml",
            "[service]\ntimezone = \"UTC\"\n",
        );
        let env = env_with(&[("DAILYBRIEF_CONFIG", ok.to_str().unwrap())]);
        let cfg = load_config(&env).unwrap();
        assert_eq!(
            (cfg.retention.days, cfg.retention.cron.as_str()),
            (60, "0 7 * * *")
        );
    }

    #[test]
    fn allow_loopback_cannot_be_set_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "config/config.toml",
            "[service]\ntimezone = \"UTC\"\n[ingest]\nallow_loopback = true\n",
        );
        let env = env_with(&[("DAILYBRIEF_CONFIG", path.to_str().unwrap())]);
        let err = load_config(&env).unwrap_err();
        assert!(err.to_string().contains("allow_loopback"), "{err}");
        let committed = load_config(&Env::from_lookup(|_| None).unwrap()).unwrap();
        assert!(
            !committed.ingest.allow_loopback,
            "the committed config never allows loopback"
        );
    }

    #[test]
    fn unknown_key_mcp_command_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "config/config.toml",
            "[service]\ntimezone = \"UTC\"\n[harness]\nmcp_command = \"dailybrief\"\n",
        );
        let env = env_with(&[("DAILYBRIEF_CONFIG", path.to_str().unwrap())]);
        let err = load_config(&env).unwrap_err();
        assert!(err.to_string().contains("mcp_command"), "{err}");
    }

    #[test]
    fn rejects_bad_timezone() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "config/config.toml",
            "[service]\ntimezone = \"Mars/Olympus\"\n",
        );
        let env = env_with(&[("DAILYBRIEF_CONFIG", path.to_str().unwrap())]);
        let err = load_config(&env).unwrap_err();
        assert!(err.to_string().contains("timezone"), "{err}");
    }

    #[test]
    fn rejects_duplicate_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "feeds.toml",
            "[[feeds]]\nid = \"a\"\nurl = \"https://x.example/rss\"\n[[feeds]]\nid = \"a\"\nurl = \"https://y.example/rss\"\n",
        );
        let err = load_feeds(&path).unwrap_err();
        assert!(err.to_string().contains("duplicate id 'a'"), "{err}");
    }

    #[test]
    fn rejects_bad_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "topics.toml",
            "[[topics]]\nid = \"Bad Id\"\nname = \"x\"\n",
        );
        let err = load_topics(&path).unwrap_err();
        assert!(err.to_string().contains("id 'Bad Id'"), "{err}");
    }

    #[test]
    fn rejects_non_http_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "feeds.toml",
            "[[feeds]]\nid = \"a\"\nurl = \"ftp://x.example/rss\"\n",
        );
        let err = load_feeds(&path).unwrap_err();
        assert!(err.to_string().contains("http"), "{err}");
    }

    #[test]
    fn feed_title_defaults_to_id_and_flags_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "feeds.toml",
            "[[feeds]]\nid = \"a\"\nurl = \"https://x.example/rss\"\n",
        );
        let feeds = load_feeds(&path).unwrap();
        assert_eq!(feeds[0].title, "a");
        assert!(feeds[0].enabled);
        assert_eq!(feeds[0].weight, 1.0);
    }

    #[test]
    fn env_overrides_data_dir_and_bind() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "config/config.toml", MINIMAL_CONFIG);
        let env = env_with(&[
            ("DAILYBRIEF_CONFIG", path.to_str().unwrap()),
            ("DAILYBRIEF_DATA_DIR", "/data"),
            ("DAILYBRIEF_BIND", "0.0.0.0"),
            ("DAILYBRIEF_IN_CONTAINER", "1"),
        ]);
        assert!(env.in_container);
        let config = load_config(&env).unwrap();
        assert_eq!(config.service.data_dir, PathBuf::from("/data"));
        assert_eq!(config.paths.data_dir, PathBuf::from("/data"));
        assert_eq!(config.service.bind, "0.0.0.0");
    }

    #[test]
    fn paths_resolve_next_to_config_and_from_project_root() {
        let env = env_with(&[("DAILYBRIEF_CONFIG", "/app/config/config.toml")]);
        let paths = resolve_paths(
            Path::new("/app/config/config.toml"),
            Path::new("data"),
            &env,
        );
        assert_eq!(paths.feeds, PathBuf::from("/app/config/feeds.toml"));
        assert_eq!(paths.topics, PathBuf::from("/app/config/topics.toml"));
        assert_eq!(paths.mcp_template, PathBuf::from("/app/config/mcp.json"));
        assert_eq!(paths.editor_prompt, PathBuf::from("/app/prompts/editor.md"));
        assert_eq!(
            paths.digest_schema,
            PathBuf::from("/app/schemas/digest.json")
        );
        assert_eq!(paths.data_dir, PathBuf::from("data"));
    }

    #[test]
    fn empty_env_value_is_an_error() {
        let err =
            Env::from_lookup(|n| (n == "DAILYBRIEF_CONFIG").then(|| "  ".to_string())).unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::Env {
                    name: "DAILYBRIEF_CONFIG",
                    ..
                }
            ),
            "{err}"
        );
    }
}
