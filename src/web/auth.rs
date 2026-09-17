//! Cloudflare Access in front, verified here as defence in depth (`SPEC.md` §7 Auth, §7b):
//! `Cf-Access-Jwt-Assertion` is an RS256 JWT signed by the team's JWKS, with the Access app's
//! AUD as audience. Bypassed only when `CF_ACCESS_AUD` is unset **and** the bind is loopback.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::RwLock;

use super::app::AppState;

pub const ASSERTION_HEADER: &str = "cf-access-jwt-assertion";
pub const JWKS_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthConfigError {
    #[error("CF_ACCESS_AUD is set but CF_ACCESS_TEAM is missing")]
    MissingTeam,
    #[error(
        "CF_ACCESS_AUD is unset and the bind is {0}: refusing to serve unauthenticated off loopback"
    )]
    UnauthenticatedBind(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessSettings {
    pub aud: String,
    pub team: String,
}

impl AccessSettings {
    /// `Ok(None)` is the loopback bypass; anything else needs both variables.
    pub fn from_env(
        env: &HashMap<String, String>,
        bind: &str,
    ) -> Result<Option<Self>, AuthConfigError> {
        let aud = env
            .get("CF_ACCESS_AUD")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let team = env
            .get("CF_ACCESS_TEAM")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        match (aud, team) {
            (Some(aud), Some(team)) => Ok(Some(Self {
                aud: aud.to_string(),
                team: team.to_string(),
            })),
            (Some(_), None) => Err(AuthConfigError::MissingTeam),
            (None, _) if matches!(bind, "127.0.0.1" | "localhost" | "::1") => Ok(None),
            (None, _) => Err(AuthConfigError::UnauthenticatedBind(bind.to_string())),
        }
    }

    pub fn issuer(&self) -> String {
        format!("https://{}.cloudflareaccess.com", self.team)
    }

    pub fn jwks_url(&self) -> String {
        format!("{}/cdn-cgi/access/certs", self.issuer())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing {ASSERTION_HEADER}")]
    Missing,
    #[error("invalid assertion: {0}")]
    Invalid(String),
    #[error("JWKS unavailable: {0}")]
    JwksUnavailable(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub sub: Option<String>,
    pub exp: u64,
}

pub struct AccessVerifier {
    settings: AccessSettings,
    jwks_url: String,
    client: reqwest::Client,
    cache: RwLock<Option<(JwkSet, Instant)>>,
    ttl: Duration,
}

impl AccessVerifier {
    /// `jwks_url` defaults to the team's certs endpoint; tests point it at a mock.
    pub fn new(settings: AccessSettings, jwks_url: Option<String>) -> Self {
        let jwks_url = jwks_url.unwrap_or_else(|| settings.jwks_url());
        Self {
            settings,
            jwks_url,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            cache: RwLock::new(None),
            ttl: JWKS_TTL,
        }
    }

    async fn fetch_jwks(&self) -> Result<JwkSet, AuthError> {
        let resp = self
            .client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| AuthError::JwksUnavailable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AuthError::JwksUnavailable(format!(
                "HTTP {}",
                resp.status().as_u16()
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AuthError::JwksUnavailable(e.to_string()))?;
        let set: JwkSet = serde_json::from_slice(&bytes)
            .map_err(|e| AuthError::JwksUnavailable(format!("bad JWKS: {e}")))?;
        *self.cache.write().await = Some((set.clone(), Instant::now()));
        Ok(set)
    }

    /// The cached set when fresh, else a fetch; on fetch failure a stale cache still serves.
    async fn jwks(&self, force: bool) -> Result<JwkSet, AuthError> {
        if !force
            && let Some((set, at)) = self.cache.read().await.as_ref()
            && at.elapsed() < self.ttl
        {
            return Ok(set.clone());
        }
        match self.fetch_jwks().await {
            Ok(set) => Ok(set),
            Err(e) => match self.cache.read().await.as_ref() {
                Some((stale, _)) if !force => {
                    tracing::warn!(error = %e, "JWKS refresh failed; serving the stale cache");
                    Ok(stale.clone())
                }
                _ => Err(e),
            },
        }
    }

    pub async fn verify(&self, token: &str) -> Result<Claims, AuthError> {
        let header = decode_header(token).map_err(|e| AuthError::Invalid(e.to_string()))?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::Invalid(format!(
                "unexpected alg {:?}",
                header.alg
            )));
        }
        let kid = header
            .kid
            .ok_or_else(|| AuthError::Invalid("no kid".into()))?;
        let mut set = self.jwks(false).await?;
        if set.find(&kid).is_none() {
            // Key rotation: one refetch, then give up.
            set = self.jwks(true).await?;
        }
        let jwk = set
            .find(&kid)
            .ok_or_else(|| AuthError::Invalid(format!("unknown kid {kid}")))?;
        let key = DecodingKey::from_jwk(jwk).map_err(|e| AuthError::Invalid(e.to_string()))?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[self.settings.aud.as_str()]);
        validation.set_issuer(&[self.settings.issuer()]);
        validation.set_required_spec_claims(&["exp", "aud", "iss"]);
        let data = decode::<Claims>(token, &key, &validation)
            .map_err(|e| AuthError::Invalid(e.to_string()))?;
        Ok(data.claims)
    }
}

/// Every route sits behind this; `state.access == None` is the loopback bypass.
pub async fn require_access(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(verifier) = state.access.clone() else {
        return next.run(req).await;
    };
    let token = req
        .headers()
        .get(ASSERTION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    };
    match verifier.verify(&token).await {
        Ok(_) => next.run(req).await,
        Err(AuthError::JwksUnavailable(e)) => {
            tracing::error!(error = %e, "cannot verify Access assertion");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "cannot verify access right now",
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "Access assertion rejected");
            (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
        }
    }
}

pub type SharedVerifier = Option<Arc<AccessVerifier>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn auth_bypassed_only_on_loopback_without_aud() {
        assert_eq!(AccessSettings::from_env(&env(&[]), "127.0.0.1"), Ok(None));
        assert_eq!(
            AccessSettings::from_env(&env(&[]), "0.0.0.0"),
            Err(AuthConfigError::UnauthenticatedBind("0.0.0.0".into()))
        );
        assert_eq!(
            AccessSettings::from_env(&env(&[("CF_ACCESS_AUD", "abc")]), "127.0.0.1"),
            Err(AuthConfigError::MissingTeam)
        );
        let s = AccessSettings::from_env(
            &env(&[
                ("CF_ACCESS_AUD", "abc"),
                ("CF_ACCESS_TEAM", "hundredclouds"),
            ]),
            "0.0.0.0",
        )
        .unwrap()
        .unwrap();
        assert_eq!(s.issuer(), "https://hundredclouds.cloudflareaccess.com");
        assert_eq!(
            s.jwks_url(),
            "https://hundredclouds.cloudflareaccess.com/cdn-cgi/access/certs"
        );
    }
}
