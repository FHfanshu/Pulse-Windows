// Ported from upstream Auth/OAuthLogin.swift (formEncoded, claims(inJWT:), randomToken,
// described) and the `base64URLEncoded` extension.
//! Encoding and parsing helpers shared by the sign-in flows.

use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Everything but the unreserved set, which is what a form body wants and what the published
/// clients send. Spaces are `%20`, a literal plus is `%2B`.
pub fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `a=1&b=2` with both sides strictly encoded.
pub fn form_body<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode(k.as_ref()), form_encode(v.as_ref())))
        .collect::<Vec<_>>()
        .join("&")
}

/// Reads one query value the way upstream's callback does: a query string spells a space `+`, so
/// that is undone **before** percent-decoding, which keeps a literal plus (`%2B`) a plus.
pub fn query_decode(raw: &str) -> String {
    let spaced = raw.replace('+', "%20");
    let bytes = spaced.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Base64 as OAuth wants it: URL-safe, unpadded.
pub fn base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE's S256 challenge: base64url of the SHA-256 of the verifier **string** (not raw bytes).
pub fn challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// 32 random bytes, base64url: comfortably inside the 43..128 characters PKCE asks of a verifier,
/// and the same generator serves the `state`. Unpredictability is load-bearing, so a failing
/// system source is a panic rather than 32 zero bytes standing in for both.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system random source failed");
    base64url(&bytes)
}

/// The claims of a JWT, unverified (the token came from the provider's own endpoint over TLS).
pub fn jwt_claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (_header, payload, _signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    // Some issuers pad; the url-safe engine without padding refuses that, so strip it.
    let payload = payload.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload).or_else(|_| URL_SAFE.decode(payload)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// A refusal in enough detail to act on: which step, what status, and the provider's own words
/// when it gave any (upstream `described`).
pub fn described(status: u16, body: &[u8], step: &str) -> String {
    let mut said = String::new();
    if let Ok(json) = serde_json::from_slice::<Value>(body) {
        for key in ["error_description", "detail", "message", "error"] {
            if let Some(text) = json.get(key).and_then(Value::as_str).filter(|t| !t.is_empty()) {
                said = text.to_string();
                break;
            }
        }
    }
    if said.is_empty() {
        said = String::from_utf8_lossy(body).trim().chars().take(160).collect();
    }
    if said.is_empty() {
        format!("{step}: HTTP {status}")
    } else {
        format!("{step}: HTTP {status} — {said}")
    }
}

/// `interval` as the services spell it: a number, or a string holding one.
pub fn seconds(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding_is_strict() {
        assert_eq!(form_encode("a b+c:d/e~f-g_h.i"), "a%20b%2Bc%3Ad%2Fe~f-g_h.i");
        assert_eq!(form_body(&[("redirect_uri", "http://localhost:1455/auth/callback")]), "redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback");
    }

    #[test]
    fn query_values_undo_plus_before_percent() {
        assert_eq!(query_decode("User+declined"), "User declined");
        assert_eq!(query_decode("a%2Bb"), "a+b");
        assert_eq!(query_decode("100%"), "100%");
        assert_eq!(query_decode("%E2%9C%93"), "✓");
    }

    #[test]
    fn pkce_challenge_matches_the_rfc_example() {
        // RFC 7636 appendix B.
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let token = random_token();
        assert_eq!(token.len(), 43);
        assert_ne!(token, random_token());
    }

    #[test]
    fn jwt_claims_decode_with_and_without_padding() {
        // {"sub":"auth0|user_abc","exp":1900000000}
        let payload = base64url(br#"{"sub":"auth0|user_abc","exp":1900000000}"#);
        let token = format!("e30.{payload}.sig");
        assert_eq!(jwt_claims(&token).unwrap()["sub"], "auth0|user_abc");
        let padded = format!("e30.{payload}==.sig");
        assert!(jwt_claims(&padded).is_some());
        assert!(jwt_claims("not-a-jwt").is_none());
        assert!(jwt_claims("a.b.c.d").is_none());
    }

    #[test]
    fn refusals_name_the_step_status_and_words() {
        assert_eq!(described(400, br#"{"error_description":"bad code"}"#, "token"), "token: HTTP 400 — bad code");
        assert_eq!(described(502, b"<html>gateway</html>", "token"), "token: HTTP 502 — <html>gateway</html>");
        assert_eq!(described(500, b"", "token"), "token: HTTP 500");
    }
}
