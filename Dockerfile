# syntax=docker/dockerfile:1.7
# dailybrief image (SPEC.md §9, spec/r0.md). Targets:
#   dev      rust toolchain + the quality tools; source is bind-mounted at /app (profiles dev, test)
#   runtime  debian slim + the release binary + Claude Code at an exact version (profile app)
# trixie, not bookworm: the prebuilt ONNX Runtime that fastembed links (ort-download-binaries)
# needs glibc >= 2.38 and a matching libstdc++; bookworm has 2.36 and fails at link time.
ARG RUST_IMAGE=rust:1.98.1-trixie
ARG CLAUDE_CODE_VERSION=2.1.274
ARG GITLEAKS_VERSION=8.30.1

# ---- builder: the release binary ----
FROM ${RUST_IMAGE} AS builder
WORKDIR /app
# The image already pins the toolchain; rust-toolchain.toml is deliberately not copied, or
# rustup would sync the channel manifest (a network round-trip) on every build.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked \
 && cp target/release/dailybrief /usr/local/bin/dailybrief

# ---- dev / test: bind-mounted source, cached target and registry volumes ----
FROM ${RUST_IMAGE} AS dev
ARG GITLEAKS_VERSION
RUN apt-get update \
 && apt-get install -y --no-install-recommends bash ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add clippy rustfmt llvm-tools-preview \
 && cargo install --locked cargo-llvm-cov cargo-deny cargo-watch \
 && curl -fsSL -o /tmp/gl.tgz "https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/gitleaks_${GITLEAKS_VERSION}_linux_x64.tar.gz" \
 && tar -xzf /tmp/gl.tgz -C /usr/local/bin gitleaks && rm /tmp/gl.tgz \
 && useradd -m -u 1000 -s /bin/bash app \
 && mkdir -p /app /data /home/app/.claude \
 && chown -R app:app /app /data /home/app /usr/local/cargo
ENV DAILYBRIEF_IN_CONTAINER=1 \
    CLAUDE_CONFIG_DIR=/home/app/.claude \
    DISABLE_AUTOUPDATER=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    CARGO_TARGET_DIR=/app/target
# The compose volumes mount here; a named volume copies the mount point's owner on first use,
# so both must already belong to app or cargo cannot write as uid 1000.
RUN mkdir -p /app/target /usr/local/cargo/registry \
 && chown app:app /app/target /usr/local/cargo/registry
USER app
WORKDIR /app

# ---- runtime: the service ----
FROM debian:trixie-slim AS runtime
ARG CLAUDE_CODE_VERSION
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl zstd libgomp1 libstdc++6 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m -u 1000 -s /bin/bash app \
 && mkdir -p /app /data /home/app/.claude \
 && chown -R app:app /app /data /home/app
USER app
ENV HOME=/home/app \
    PATH=/home/app/.local/bin:/usr/local/bin:/usr/bin:/bin
# Claude Code at an exact version, installed for the non-root user by the native installer
# (checksum-verified). It is the subscription-billed harness; its only credential is the
# CLAUDE_CODE_OAUTH_TOKEN from .env at run time. Never an ANTHROPIC_API_KEY.
RUN curl -fsSL https://claude.ai/install.sh | bash -s -- "${CLAUDE_CODE_VERSION}" \
 && claude --version
ENV DAILYBRIEF_IN_CONTAINER=1 \
    DAILYBRIEF_CONFIG=/app/config/config.toml \
    DAILYBRIEF_DATA_DIR=/data \
    DAILYBRIEF_BIND=0.0.0.0 \
    CLAUDE_CONFIG_DIR=/home/app/.claude \
    DISABLE_AUTOUPDATER=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
WORKDIR /app
COPY --chown=app:app config ./config
COPY --chown=app:app prompts ./prompts
COPY --chown=app:app schemas ./schemas
COPY --from=builder /usr/local/bin/dailybrief /usr/local/bin/dailybrief
EXPOSE 8788
CMD ["dailybrief", "serve"]
