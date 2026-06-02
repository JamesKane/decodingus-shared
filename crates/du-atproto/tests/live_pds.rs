//! Live OAuth handshake (discovery + PAR) against a real AT Protocol PDS.
//!
//! Validates the bits that unit tests can't: that a real atproto authorization
//! server accepts our pushed-authorization-request form + DPoP proof and returns
//! a `request_uri`, including the `use_dpop_nonce` single-retry dance. The full
//! browser redirect + token exchange needs HTTPS/identity infra and is out of
//! scope here.
//!
//! Gated on `PDS_TEST_URL` (e.g. a local container: `http://192.168.64.5:3000`);
//! skips/passes when unset.
//!
//!   PDS_TEST_URL=http://<pds-ip>:3000 cargo test -p du-atproto --test live_pds -- --nocapture

use du_atproto::oauth::{dpop_proof, par_form_public, AuthServerMetadata, EcKey, Pkce};
use std::time::{SystemTime, UNIX_EPOCH};

fn pds_url() -> Option<String> {
    std::env::var("PDS_TEST_URL").ok().filter(|s| !s.is_empty())
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_and_par_against_live_pds() {
    let Some(pds) = pds_url() else {
        eprintln!("PDS_TEST_URL unset — skipping live PDS handshake test");
        return;
    };
    let pds = pds.trim_end_matches('/').to_string();
    let http = reqwest::Client::new();

    // 1. Discovery: the PDS serves authorization-server metadata. (We fetch it
    //    from the reachable base rather than following the https issuer, which a
    //    local container doesn't terminate TLS for.)
    let meta: AuthServerMetadata = http
        .get(format!("{pds}/.well-known/oauth-authorization-server"))
        .send()
        .await
        .expect("fetch auth-server metadata")
        .error_for_status()
        .expect("metadata 2xx")
        .json()
        .await
        .expect("parse AuthServerMetadata");
    assert!(meta.issuer.starts_with("https://"), "issuer should be https: {}", meta.issuer);
    assert!(
        meta.pushed_authorization_request_endpoint.is_some(),
        "PDS must advertise a PAR endpoint"
    );
    eprintln!("discovered issuer={} par={:?}", meta.issuer, meta.pushed_authorization_request_endpoint);

    // 2. Build a loopback (public, PKCE-only) client request — the atproto dev
    //    client that needs no hosted client-metadata document.
    let redirect_uri = "http://127.0.0.1:9000/oauth/callback";
    let client_id = format!(
        "http://localhost?redirect_uri={}&scope=atproto",
        urlencoding(redirect_uri)
    );
    let pkce = Pkce::generate();
    let state = du_atproto::oauth::random_token();
    let form = par_form_public(&client_id, redirect_uri, "atproto", &state, &pkce.challenge, None);

    // 3. POST the PAR with a DPoP proof, retrying once on a server nonce.
    //    The DPoP `htu` must be the server's CANONICAL endpoint (from metadata),
    //    not the transport address — a local container is reached over http://ip
    //    but the auth server validates against its https issuer URL.
    let canonical_par = meta.pushed_authorization_request_endpoint.clone().unwrap();
    let post_to = format!("{pds}/oauth/par");
    let key = EcKey::generate();
    let (status, body, nonce) = post_par(&http, &key, &post_to, &canonical_par, &form, None).await;
    let (status, body) = if status == 400 && body.get("error").and_then(|e| e.as_str()) == Some("use_dpop_nonce") {
        let n = nonce.expect("server should supply DPoP-Nonce with use_dpop_nonce");
        eprintln!("retrying PAR with server DPoP-Nonce");
        let (s, b, _) = post_par(&http, &key, &post_to, &canonical_par, &form, Some(&n)).await;
        (s, b)
    } else {
        (status, body)
    };

    eprintln!("PAR status={status} body={body}");
    assert!(status.is_success(), "PAR should succeed, got {status}: {body}");
    let request_uri = body.get("request_uri").and_then(|v| v.as_str());
    assert!(request_uri.is_some(), "PAR response must include request_uri: {body}");
    eprintln!("✓ PAR accepted; request_uri={}", request_uri.unwrap());
}

/// POST the PAR form with a fresh DPoP proof; returns (status, json body, DPoP-Nonce).
async fn post_par(
    http: &reqwest::Client,
    key: &EcKey,
    post_to: &str,
    canonical_htu: &str,
    form: &[(String, String)],
    nonce: Option<&str>,
) -> (reqwest::StatusCode, serde_json::Value, Option<String>) {
    let proof = dpop_proof(key, "POST", canonical_htu, now(), nonce, None);
    let resp = http
        .post(post_to)
        .header("DPoP", proof)
        .form(form)
        .send()
        .await
        .expect("PAR request");
    let status = resp.status();
    let server_nonce = resp
        .headers()
        .get("DPoP-Nonce")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    (status, body, server_nonce)
}

/// Minimal percent-encoding for a redirect_uri embedded in the client_id query.
fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
