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

/// The first smoke run failed with "Claude configuration file not found … a backup exists":
/// the build-time `claude --version` ran before `CLAUDE_CONFIG_DIR` was set, so the config
/// landed in $HOME while its backups landed in the config dir the volume adopts. The image must
/// set the env first, verify with a throwaway dir, and ship the config dir empty.
#[test]
fn dockerfile_ships_an_empty_claude_config_dir() {
    let dockerfile = read("Dockerfile");
    let install_at = dockerfile
        .find("bash /tmp/ops/claude-install.sh")
        .expect("the vendored installer line");
    let verify_at = dockerfile
        .find("CLAUDE_CONFIG_DIR=/tmp/claude-verify")
        .expect("the version check uses a throwaway config dir");
    assert!(
        verify_at < install_at,
        "the throwaway config dir is exported before Claude Code runs"
    );
    assert!(dockerfile.contains("claude --version"));
    assert!(
        dockerfile.contains("find /home/app/.claude -mindepth 1 -delete"),
        "the config dir the volume adopts is shipped empty"
    );
}

#[test]
fn dockerfile_uses_the_vendored_installer_and_purges_download_tools() {
    let dockerfile = read("Dockerfile");
    assert!(
        !dockerfile.contains("claude.ai/install.sh"),
        "no installer fetched at build time"
    );
    assert!(dockerfile.contains("COPY ops/claude-install.sh ops/claude-install.sh.sha256"));
    assert!(dockerfile.contains("sha256sum -c claude-install.sh.sha256"));
    assert!(dockerfile.contains("bash /tmp/ops/claude-install.sh ${CLAUDE_CODE_VERSION}"));
    assert!(dockerfile.contains("apt-get purge -y curl zstd"));
    // The recorded hash matches the vendored script.
    use sha2::{Digest, Sha256};
    let script =
        std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ops/claude-install.sh"))
            .unwrap();
    let actual: String = Sha256::digest(&script)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let recorded = read("ops/claude-install.sh.sha256");
    assert_eq!(recorded.split_whitespace().next().unwrap(), actual);
    assert_eq!(
        recorded.split_whitespace().nth(1).unwrap(),
        "claude-install.sh"
    );
}

/// `deny.toml` and CONSTRAINTS.md name the same licence set, so neither can drift alone.
#[test]
fn deny_licences_match_constraints() {
    let deny = read("deny.toml");
    let allow = deny
        .split("allow = [")
        .nth(1)
        .and_then(|s| s.split(']').next())
        .expect("deny.toml [licenses].allow");
    let from_deny: std::collections::BTreeSet<String> = allow
        .lines()
        .filter_map(|l| {
            l.trim()
                .trim_end_matches(',')
                .trim_matches('"')
                .to_string()
                .into()
        })
        .filter(|l: &String| !l.is_empty())
        .collect();
    let constraints = read("CONSTRAINTS.md");
    let listed = constraints
        .split("licences only ")
        .nth(1)
        .and_then(|s| s.split(" (").next())
        .expect("CONSTRAINTS.md licence list");
    let from_constraints: std::collections::BTreeSet<String> =
        listed.split(", ").map(|l| l.trim().to_string()).collect();
    assert_eq!(from_deny, from_constraints);
}

/// The app image is a variable with the runtime tag as default: rollback is one env var.
#[test]
fn compose_app_image_is_overridable_for_rollback() {
    let compose = read("compose.yaml");
    let app = compose.split("\n  dev:").next().unwrap_or_default();
    assert!(
        app.contains("image: ${DAILYBRIEF_IMAGE:-dailybrief-rs:runtime}"),
        "{app}"
    );
}
