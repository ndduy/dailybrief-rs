# Runbook — dailybrief-rs

The service runs from `dailybrief-rs:runtime` under `docker compose --profile app` on the box,
behind Cloudflare Access at `https://dailybrief.hundredclouds.com/`. Everything below is a
command you can paste from the repo root.

## Dates that must not slip

| What | When | Action |
|---|---|---|
| `CLAUDE_CODE_OAUTH_TOKEN` renewal | the token was created on or before **2026-09-11** (the date of the first `.env`); Anthropic setup tokens last one year | before **2027-09-01**: `claude setup-token` on the host, paste into `.env`, `chmod 600 .env`, `docker compose --profile app up -d` |
| `deny.toml` advisory exceptions X3 (`rsa` Marvin) and X4 (`paste` unmaintained) | expire **2026-12-17** (`CONSTRAINTS.md`) | re-check `cargo deny check advisories`; extend with a reason or drop the dependency |
| Claude Code version in the image | pinned `2.1.274` in the Dockerfile | bump deliberately: re-vendor `ops/claude-install.sh` (`curl -fsSL https://claude.ai/install.sh -o ops/claude-install.sh && (cd ops && sha256sum claude-install.sh > claude-install.sh.sha256)`), re-run the smoke run |

## Every morning (30 seconds)

1. Open the home page. A digest with 24 + 6 items means the 06:30 run succeeded.
2. Otherwise the page shows the failed state with the caps bar; follow the run link.
   `/runs/<id>` shows the turns, `/runs/<id>/log` every event, the transcript link the raw file.
3. `/runs` lists every run with status, turns and wall clock.

## Deploy (only between mornings: after 07:00, before 23:00 local)

```sh
docker tag dailybrief-rs:runtime dailybrief-rs:$(git describe --always)   # keep what runs now
docker compose --profile app build
docker run --rm -v dailybrief-rs_dailybrief-data:/data alpine sh -c \
  "apk add --no-cache sqlite >/dev/null; sqlite3 /data/brief.db \"select count(*) from runs where status='running'\""   # must be 0
docker compose --profile app up -d
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8788/            # 401 (Access on)
docker compose --profile app logs --no-log-prefix --since 1m app | grep -E 'serving|scheduler'
```

Tag **before** you build: a rebuild moves the `runtime` tag, and an untagged image that a
running container uses can be dropped by the image store (that is how the R0 image was lost
and had to be rebuilt from its commit, ADR 0015).

## Rollback (one command, under two minutes)

```sh
docker compose --profile app down && DAILYBRIEF_IMAGE=dailybrief-rs:r0 docker compose --profile app up -d
```

Never run `docker compose ... build` or `up --build` with `DAILYBRIEF_IMAGE` set: the variable
is also the build tag, and a build would overwrite the rollback image. `unset DAILYBRIEF_IMAGE`
once the rollback is over, or the next plain `up -d` silently keeps running the old image.

`dailybrief-rs:r0` is the R0 ship (commit a5f985c); `dailybrief-rs:m2` the M2 ship. Roll back when a 06:30 run fails and the
run page does not explain it, on a 5xx from the pages, or on any sign of a token in a page or
transcript. The database is shared and forward-compatible within a milestone (no migration
since `0001_init`); if a milestone adds a migration, its ADR says how to roll that back.

## Manual run and its cap

`POST /run` from the Refresh button, or `docker compose --profile app exec app dailybrief run`.
Three manual runs per 24 h (retries do not count). `--max-turns 5 --attempts 1` is the forced
failure used at the M2 gate; never schedule it.

## Retention

`[retention]` in `config/config.toml`: 60 days, pruned daily at 07:00 local (after the run
window). By hand: `docker compose --profile app exec app dailybrief prune --dry-run` shows the
plan; without `--dry-run` it deletes run directories and `run_events` older than the window.
`runs` rows are never deleted. `/data/runs` should hold about 60 days.

## Egress gate before every ship

```sh
docker compose --profile app exec app dailybrief scan-transcript /data/runs/<id>/transcript.jsonl
```

Clean output ends with `clean: <path>`; any finding is exit 1 and needs a human look ("Obsidian"
and "/vault" are deliberately broad).

## Where things live

- Data: the `dailybrief-rs_dailybrief-data` volume (`/data`: `brief.db`, `runs/`, `models/`).
- Claude Code state: `dailybrief-rs_dailybrief-claude` (`/home/app/.claude`).
- Secrets: `.env` only (`CLAUDE_CODE_OAUTH_TOKEN`, `CF_ACCESS_AUD`, `CF_ACCESS_TEAM`), mode 600.
- Logs: `docker compose --profile app logs app` (JSON lines on stderr).
- The old TypeScript stack: `docker compose --project-directory ../dailybrief` (stopped; removed
  at the M2 ship).
