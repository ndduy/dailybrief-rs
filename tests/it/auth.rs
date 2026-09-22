//! The Access JWT middleware against a wiremock JWKS with a key generated for the test run.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, TimeZone, Utc};
use dailybrief::config::{Env, load_config};
use dailybrief::core::time::parse_tz;
use dailybrief::db::Db;
use dailybrief::web::app::{AppState, router};
use dailybrief::web::auth::{AccessSettings, AccessVerifier};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const AUD: &str = "aud-for-tests";
const TEAM: &str = "hundredclouds";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 16, 23, 45, 0).unwrap()
}

struct Key {
    kid: &'static str,
    pem: String,
    jwk: Value,
}

fn key(kid: &'static str) -> Key {
    let mut rng = rand::thread_rng();
    let private = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pem = private.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
    let n = URL_SAFE_NO_PAD.encode(private.n().to_bytes_be());
    let e = URL_SAFE_NO_PAD.encode(private.e().to_bytes_be());
    let jwk = json!({ "kty": "RSA", "kid": kid, "use": "sig", "alg": "RS256", "n": n, "e": e });
    Key { kid, pem, jwk }
}

fn jwks_body(keys: &[&Key]) -> Value {
    // Cloudflare's endpoint carries extra fields next to `keys`; they must not break parsing.
    json!({
        "keys": keys.iter().map(|k| k.jwk.clone()).collect::<Vec<_>>(),
        "public_cert": { "kid": "cert-1", "cert": "-----BEGIN CERTIFICATE-----" },
        "public_certs": [],
    })
}

fn token(key: &Key, aud: &str, iss: &str, exp_offset: i64) -> String {
    let exp = (chrono::Utc::now().timestamp() + exp_offset) as u64;
    let claims = json!({
        "aud": [aud], "iss": iss, "exp": exp, "iat": exp - 600,
        "email": "duy@example", "sub": "user-1", "type": "app",
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(key.kid.to_string());
    encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(key.pem.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn issuer() -> String {
    format!("https://{TEAM}.cloudflareaccess.com")
}

fn state_with_access(jwks_url: &str) -> AppState {
    state_with_access_interval(jwks_url, None)
}

/// `interval` overrides the forced-refetch throttle (the production value is 60 s).
fn state_with_access_interval(jwks_url: &str, interval: Option<Duration>) -> AppState {
    let config = load_config(&Env::from_lookup(|_| None).unwrap()).unwrap();
    let tz = parse_tz(&config.service.timezone).unwrap();
    let mut verifier = AccessVerifier::new(
        AccessSettings {
            aud: AUD.into(),
            team: TEAM.into(),
        },
        Some(jwks_url.to_string()),
    );
    if let Some(i) = interval {
        verifier = verifier.with_forced_min_interval(i);
    }
    AppState::new(Db::open_in_memory().unwrap(), config, tz, now, None)
        .with_access(Some(Arc::new(verifier)))
}

async fn get_with(state: AppState, path_: &str, assertion: Option<&str>) -> StatusCode {
    let mut b = Request::builder().uri(path_);
    if let Some(a) = assertion {
        b = b.header("cf-access-jwt-assertion", a);
    }
    let res = router(state)
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let _ = res.into_body().collect().await;
    status
}

async fn mount_jwks(server: &MockServer, keys: &[&Key]) {
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/cdn-cgi/access/certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jwks_body(keys)))
        .mount(server)
        .await;
}

#[tokio::test]
async fn auth_accepts_valid_and_rejects_bad_signature_wrong_audience_expired_and_missing() {
    let k1 = key("k1");
    let other = key("k1"); // same kid, different key: a forged signature
    let server = MockServer::start().await;
    mount_jwks(&server, &[&k1]).await;
    let url = format!("{}/cdn-cgi/access/certs", server.uri());

    let ok = token(&k1, AUD, &issuer(), 600);
    assert_eq!(
        get_with(state_with_access(&url), "/", Some(&ok)).await,
        StatusCode::OK
    );
    assert_eq!(
        get_with(state_with_access(&url), "/", None).await,
        StatusCode::UNAUTHORIZED
    );
    let forged = token(&other, AUD, &issuer(), 600);
    assert_eq!(
        get_with(state_with_access(&url), "/", Some(&forged)).await,
        StatusCode::UNAUTHORIZED
    );
    let wrong_aud = token(&k1, "another-app", &issuer(), 600);
    assert_eq!(
        get_with(state_with_access(&url), "/", Some(&wrong_aud)).await,
        StatusCode::UNAUTHORIZED
    );
    let wrong_iss = token(&k1, AUD, "https://evil.cloudflareaccess.com", 600);
    assert_eq!(
        get_with(state_with_access(&url), "/", Some(&wrong_iss)).await,
        StatusCode::UNAUTHORIZED
    );
    let expired = token(&k1, AUD, &issuer(), -600);
    assert_eq!(
        get_with(state_with_access(&url), "/", Some(&expired)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get_with(state_with_access(&url), "/", Some("not.a.jwt")).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn auth_refetches_on_unknown_kid_and_caches_otherwise() {
    let k1 = key("k1");
    let k2 = key("k2");
    let server = MockServer::start().await;
    mount_jwks(&server, &[&k1]).await;
    let url = format!("{}/cdn-cgi/access/certs", server.uri());
    let state = state_with_access(&url);
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k1, AUD, &issuer(), 600))).await,
        StatusCode::OK
    );
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k1, AUD, &issuer(), 600))).await,
        StatusCode::OK
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "second request used the cache"
    );
    // Rotation: the mock now serves k2; a k2 token triggers one refetch.
    mount_jwks(&server, &[&k1, &k2]).await;
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k2, AUD, &issuer(), 600))).await,
        StatusCode::OK
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "one refetch after the reset"
    );
    // A kid that never appears, right after the forced refetch above: the refetch is
    // throttled (one per minute), so the answer is a 401 from the cache with no request.
    let k3 = key("k3");
    assert_eq!(
        get_with(state, "/", Some(&token(&k3, AUD, &issuer(), 600))).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "the throttled unknown kid made no request"
    );
}

#[tokio::test]
async fn auth_503_when_jwks_unreachable_and_cache_empty_and_post_run_is_401() {
    let k1 = key("k1");
    let server = MockServer::start().await;
    let url = format!("{}/cdn-cgi/access/certs", server.uri());
    let state = state_with_access(&url);
    // No mock mounted: the endpoint answers 404 → unavailable, cache empty → 503.
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k1, AUD, &issuer(), 600))).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/run")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a billed action without an assertion is 401"
    );
}

/// Algorithm confusion: an HS256 token "signed" with the RSA public modulus as the HMAC secret
/// must be refused before any key lookup (and without a JWKS refetch).
#[tokio::test]
async fn auth_rejects_hs256_token_with_rs_key_material() {
    let k1 = key("k1");
    let server = MockServer::start().await;
    mount_jwks(&server, &[&k1]).await;
    let url = format!("{}/cdn-cgi/access/certs", server.uri());
    let state = state_with_access(&url);
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k1, AUD, &issuer(), 600))).await,
        StatusCode::OK
    );
    let n_bytes = URL_SAFE_NO_PAD
        .decode(k1.jwk["n"].as_str().unwrap())
        .unwrap();
    let exp = (chrono::Utc::now().timestamp() + 600) as u64;
    let claims =
        json!({ "aud": [AUD], "iss": issuer(), "exp": exp, "iat": exp - 600, "sub": "user-1" });
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("k1".into());
    let forged = encode(&header, &claims, &EncodingKey::from_secret(&n_bytes)).unwrap();
    assert_eq!(
        get_with(state, "/", Some(&forged)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "no refetch for a bad alg"
    );
}

/// Two unknown-kid tokens in quick succession cause one forced refetch, not two.
#[tokio::test]
async fn jwks_forced_refetch_is_throttled() {
    let k1 = key("k1");
    let k2 = key("k2");
    let k3 = key("k3");
    let server = MockServer::start().await;
    mount_jwks(&server, &[&k1]).await;
    let url = format!("{}/cdn-cgi/access/certs", server.uri());
    let state = state_with_access(&url);
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k1, AUD, &issuer(), 600))).await,
        StatusCode::OK
    );
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k2, AUD, &issuer(), 600))).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k3, AUD, &issuer(), 600))).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "the initial fetch plus one forced refetch; the second unknown kid was throttled"
    );
    // A known key still verifies from the cache meanwhile.
    assert_eq!(
        get_with(state, "/", Some(&token(&k1, AUD, &issuer(), 600))).await,
        StatusCode::OK
    );
}

/// After the throttle interval passes, an unknown kid forces a refetch again (and finds the
/// rotated key).
#[tokio::test]
async fn jwks_forced_refetch_resumes_after_the_interval() {
    let k1 = key("k1");
    let k2 = key("k2");
    let k3 = key("k3");
    let server = MockServer::start().await;
    mount_jwks(&server, &[&k1]).await;
    let url = format!("{}/cdn-cgi/access/certs", server.uri());
    let state = state_with_access_interval(&url, Some(Duration::from_millis(80)));
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k1, AUD, &issuer(), 600))).await,
        StatusCode::OK
    );
    // Unknown kid: one forced refetch (still k1 only) → 401.
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k2, AUD, &issuer(), 600))).await,
        StatusCode::UNAUTHORIZED
    );
    // Straight away another unknown kid: throttled, no request.
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k3, AUD, &issuer(), 600))).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    // The key rotates and the interval passes: the next unknown kid refetches and verifies.
    mount_jwks(&server, &[&k1, &k2]).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        get_with(state.clone(), "/", Some(&token(&k2, AUD, &issuer(), 600))).await,
        StatusCode::OK
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "one refetch after the interval (the mock was reset by mount_jwks)"
    );
}
