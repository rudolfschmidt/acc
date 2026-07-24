//! Shared JSON-RPC client for the crypto-wallet import backends. One HTTP round
//! trip with pluggable auth — none, HTTP Basic (Bitcoin Core), or Digest (Monero
//! wallet-rpc under `--rpc-login`, including Haveno's internal one). The daemon's
//! error is surfaced whether it comes back with a 2xx or a non-2xx JSON body.

use std::time::Duration;

use serde_json::Value;

use crate::error::Error;

/// How to authenticate the request. Borrowed, so no per-call allocation.
pub(super) enum Auth<'a> {
    None,
    /// A ready-made `Authorization` header value (Bitcoin Core: `Basic <b64>`).
    Basic(&'a str),
    /// A `user:pass` login, negotiated with the server's `401` digest challenge.
    Digest(&'a str),
}

/// One JSON-RPC call. `version` is the protocol tag (`"2.0"` monero, `"1.0"`
/// Bitcoin Core). Returns the `result` object, or a named error for transport,
/// bad JSON, or an RPC-level `error`.
pub(super) fn call(
    url: &str,
    method: &str,
    params: Value,
    auth: &Auth,
    version: &str,
    timeout: Duration,
) -> Result<Value, Error> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let body = serde_json::json!({
        "jsonrpc": version, "id": "acc", "method": method, "params": params
    })
    .to_string();
    let post = |header: Option<&str>| {
        let mut req = agent.post(url).set("Content-Type", "application/json");
        if let Some(h) = header {
            req = req.set("Authorization", h);
        }
        req.send_string(&body)
    };

    let basic = match auth {
        Auth::Basic(h) => Some(*h),
        _ => None,
    };
    let text = match post(basic) {
        Ok(r) => r.into_string(),
        // Digest wallets challenge with 401 — answer it and resend.
        Err(ureq::Error::Status(401, r)) if matches!(auth, Auth::Digest(_)) => {
            let Auth::Digest(login) = auth else { unreachable!() };
            let header = digest_header(login, path_of(url), r.header("www-authenticate").unwrap_or(""))
                .ok_or_else(|| Error::from("import: rpc: unparsable digest challenge"))?;
            post(Some(&header))
                .map_err(|e| Error::from(format!("import: rpc {}: {}", url, e)))?
                .into_string()
        }
        // Bitcoin Core returns RPC errors as a non-2xx status with a JSON body.
        Err(ureq::Error::Status(_, r)) => r.into_string(),
        Err(e) => return Err(Error::from(format!("import: rpc {}: {}", url, e))),
    }
    .map_err(|e| Error::from(format!("import: rpc read {}: {}", url, e)))?;

    let resp: Value =
        serde_json::from_str(&text).map_err(|e| Error::from(format!("import: rpc bad JSON: {}", e)))?;
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return Err(Error::from(format!("import: rpc error: {}", err)));
    }
    resp.get("result")
        .cloned()
        .ok_or_else(|| Error::from("import: rpc: response has no result"))
}

/// The request path of a URL, for the digest `uri` field
/// (`http://h:p/json_rpc` → `/json_rpc`, `http://h:p` → `/`).
fn path_of(url: &str) -> &str {
    url.split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
        .unwrap_or("/")
}

/// The `Authorization: Digest …` value for a `user:pass` login answering a
/// `WWW-Authenticate: Digest …` challenge (MD5, qop=auth) — what monero-wallet-rpc
/// expects under `--rpc-login`.
fn digest_header(login: &str, uri: &str, challenge: &str) -> Option<String> {
    let (user, pass) = login.split_once(':')?;
    let p = parse_challenge(challenge);
    let realm = p.get("realm")?;
    let nonce = p.get("nonce")?;
    let nc = "00000001";
    let md5hex = |s: String| format!("{:x}", md5::compute(s));
    let cnonce = md5hex(format!("{}:acc", nonce))[..16].to_string();
    let ha1 = md5hex(format!("{}:{}:{}", user, realm, pass));
    let ha2 = md5hex(format!("POST:{}", uri));
    let has_qop = p.contains_key("qop");
    let response = if has_qop {
        md5hex(format!("{}:{}:{}:{}:auth:{}", ha1, nonce, nc, cnonce, ha2))
    } else {
        md5hex(format!("{}:{}:{}", ha1, nonce, ha2))
    };
    let mut h = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
        user, realm, nonce, uri, response
    );
    if has_qop {
        h.push_str(&format!(", qop=auth, nc={}, cnonce=\"{}\"", nc, cnonce));
    }
    if let Some(op) = p.get("opaque") {
        h.push_str(&format!(", opaque=\"{}\"", op));
    }
    Some(h)
}

/// Parse a `Digest k=v, k="v"` challenge into a lowercased-key map (quotes
/// stripped). monero's challenge has no commas inside its values.
fn parse_challenge(challenge: &str) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    let body = challenge.trim().strip_prefix("Digest ").unwrap_or(challenge);
    for part in body.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            m.insert(k.trim().to_ascii_lowercase(), v.trim().trim_matches('"').to_string());
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_of_extracts_request_path() {
        assert_eq!(path_of("http://127.0.0.1:18100/json_rpc"), "/json_rpc");
        assert_eq!(path_of("http://127.0.0.1:8332/wallet/main"), "/wallet/main");
        assert_eq!(path_of("http://127.0.0.1:8332"), "/");
    }

    #[test]
    fn digest_header_carries_the_expected_fields() {
        let challenge = "Digest realm=\"monero-wallet-rpc\", nonce=\"abc\", qop=\"auth\"";
        let h = digest_header("haveno_user:password", "/json_rpc", challenge).unwrap();
        assert!(h.contains("username=\"haveno_user\""));
        assert!(h.contains("realm=\"monero-wallet-rpc\""));
        assert!(h.contains("nonce=\"abc\""));
        assert!(h.contains("uri=\"/json_rpc\""));
        assert!(h.contains("qop=auth"));
        assert!(h.contains("response=\""));
    }
}
