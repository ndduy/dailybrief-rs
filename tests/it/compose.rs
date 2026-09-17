//! The container files are text; these checks pin the invariants CONSTRAINTS.md names:
//! loopback-only publishing, no API key anywhere, Access always on for the service, a pinned
//! non-root Claude Code, and the secrets/data directories excluded from the build context.

use std::path::PathBuf;

fn read(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn compose_publishes_loopback_only() {
    let compose = read("compose.yaml");
    let published: Vec<&str> = compose
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("- \"") && l.contains(":8788"))
        .collect();
    assert!(!published.is_empty(), "the app publishes 8788");
    for p in published {
        assert!(
            p.starts_with("- \"127.0.0.1:8788:8788\""),
            "non-loopback publish: {p}"
        );
    }
    assert!(
        !compose.contains("0.0.0.0:8788"),
        "never published on all interfaces"
    );
}

#[test]
fn compose_has_no_anthropic_keys_anywhere_and_no_bare() {
    for f in ["compose.yaml", "Dockerfile", ".env.example", "bin/dc"] {
        let text = read(f);
        for needle in [
            "ANTHROPIC_API_KEY=",
            "ANTHROPIC_AUTH_TOKEN=",
            "sk-ant-",
            "--bare",
        ] {
            assert!(!text.contains(needle), "{f} contains {needle}");
        }
    }
    let env_example = read(".env.example");
    assert!(env_example.contains("CLAUDE_CODE_OAUTH_TOKEN="));
    assert!(env_example.contains("CF_ACCESS_AUD=") && env_example.contains("CF_ACCESS_TEAM="));
}

#[test]
fn compose_app_requires_cf_access_and_mounts_only_named_volumes() {
    let compose = read("compose.yaml");
    let app = compose.split("\n  dev:").next().unwrap_or_default();
    assert!(app.contains("required: true"), "app requires .env");
    // Access cannot be bypassed in the container: the runtime image binds 0.0.0.0, and
    // web::auth refuses to serve off loopback without CF_ACCESS_AUD (tested in web::auth).
    let dockerfile = read("Dockerfile");
    assert!(dockerfile.contains("DAILYBRIEF_BIND=0.0.0.0"));
    assert!(!app.contains("DAILYBRIEF_BIND: 127.0.0.1"));
    assert!(app.contains("dailybrief-data:/data"));
    assert!(app.contains("dailybrief-claude:/home/app/.claude"));
    assert!(
        !app.contains("- .:/app"),
        "the service never bind-mounts the repo"
    );
    assert!(app.contains("init: true"));
}

#[test]
fn dockerfile_pins_claude_code_runs_non_root_and_keeps_secrets_out() {
    let dockerfile = read("Dockerfile");
    let pinned = dockerfile
        .lines()
        .find(|l| l.starts_with("ARG CLAUDE_CODE_VERSION="))
        .expect("CLAUDE_CODE_VERSION is an ARG");
    let version = pinned.trim_start_matches("ARG CLAUDE_CODE_VERSION=");
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "exact x.y.z version, got {version}");
    assert!(
        parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())),
        "{version}"
    );
    assert!(
        dockerfile.contains("USER app"),
        "the service runs as a non-root user"
    );
    assert!(
        dockerfile.contains("useradd -m -u 1000"),
        "uid 1000 matches the host user"
    );
    assert!(dockerfile.contains("CMD [\"dailybrief\", \"serve\"]"));
    assert!(dockerfile.contains("DISABLE_AUTOUPDATER=1"));
    assert!(dockerfile.contains("DAILYBRIEF_IN_CONTAINER=1"));
    assert!(
        !dockerfile.contains("alpine"),
        "ort needs glibc: no Alpine/musl"
    );
    for needle in ["COPY .env", "COPY data", "OAUTH_TOKEN="] {
        assert!(!dockerfile.contains(needle), "Dockerfile contains {needle}");
    }
}

#[test]
fn dockerignore_excludes_env_data_target_and_git() {
    let ignore = read(".dockerignore");
    let lines: Vec<&str> = ignore.lines().map(str::trim).collect();
    for required in [".git", "target", "data", ".env", ".env.*", "*.profraw"] {
        assert!(lines.contains(&required), ".dockerignore lacks {required}");
    }
}

#[test]
fn bin_dc_runs_the_test_profile() {
    let dc = read("bin/dc");
    assert!(dc.contains("docker compose --profile test run --rm test"));
    let meta = std::fs::metadata(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin/dc")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert!(
        meta.permissions().mode() & 0o111 != 0,
        "bin/dc is executable"
    );
}
