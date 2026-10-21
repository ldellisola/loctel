//! The `Origin` check (design section 8).
//!
//! The MCP spec requires a local server to validate `Origin`, so that a web
//! page cannot drive it from the user's browser. loctel has no authentication,
//! so this is what keeps a page on `https://evil.example` out: a request whose
//! `Origin` names any host other than `localhost`, `127.0.0.1` or `[::1]` gets
//! HTTP 403. A request with no `Origin` header (a command line client, Claude
//! Code, an IDE) is allowed: browsers always send one on a cross-site request.
//!
//! (The `Host` header is checked by rmcp itself, against the same three
//! names, which is what stops DNS rebinding.)

use axum::extract::Request;
use axum::http::{StatusCode, header::ORIGIN};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// The hosts that count as "this machine", lowercase, IPv6 in brackets as it
/// appears in an `Origin`.
const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// Axum middleware: passes the request on unless its `Origin` is foreign.
/// Every method and path is checked.
pub(crate) async fn require_local_origin(request: Request, next: Next) -> Response {
    let foreign = request
        .headers()
        .get_all(ORIGIN)
        .iter()
        .any(|value| !value.to_str().is_ok_and(origin_is_local));
    if foreign {
        return (StatusCode::FORBIDDEN, "Forbidden: Origin is not allowed\n").into_response();
    }
    next.run(request).await
}

/// Whether an `Origin` header value names this machine.
///
/// An origin is `scheme://host[:port]`. Anything that is not (`null`, which
/// browsers send from sandboxed pages and `file:` URLs, a bare host, a value
/// with credentials, an empty one) is not local. The host is compared ignoring
/// case, and the port and scheme do not matter: the check is about which
/// machine the page came from.
pub(crate) fn origin_is_local(origin: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return false;
    }
    // An origin has no path, but be exact about where the authority ends.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // `localhost@evil.example` is the host `evil.example`, and no browser
    // sends credentials in an origin, so refuse them outright.
    if authority.contains('@') {
        return false;
    }
    // The host (an IPv6 literal keeps its brackets) and what follows it: empty,
    // or a colon and the port.
    let (host, port) = if authority.starts_with('[') {
        let Some(end) = authority.find(']') else {
            return false;
        };
        authority.split_at(end + 1)
    } else {
        authority
            .find(':')
            .map_or((authority, ""), |colon| authority.split_at(colon))
    };
    let port_ok = match port.strip_prefix(':') {
        None => port.is_empty(),
        Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
    };
    port_ok
        && LOCAL_HOSTS
            .iter()
            .any(|local| host.eq_ignore_ascii_case(local))
}

#[cfg(test)]
mod tests {
    use super::origin_is_local;

    #[test]
    fn this_machine_in_its_three_spellings_is_local() {
        for origin in [
            "http://localhost",
            "http://localhost:3000",
            "https://localhost:8443",
            "http://LOCALHOST:3000",
            "http://LocalHost",
            "http://127.0.0.1",
            "http://127.0.0.1:5173",
            "http://[::1]",
            "http://[::1]:5173",
            "app://localhost",
            "vscode-webview://localhost",
        ] {
            assert!(origin_is_local(origin), "{origin}");
        }
    }

    #[test]
    fn any_other_host_is_foreign() {
        for origin in [
            "https://evil.example",
            "http://evil.example:80",
            "http://localhost.evil.example",
            "http://evil.localhost.example",
            "http://127.0.0.1.evil.example:4319",
            "http://127.0.0.2",
            "http://0.0.0.0:4319",
            "http://[::2]:8080",
            "http://[::]:8080",
            "http://[0:0:0:0:0:0:0:1]",
            "http://10.0.0.5",
            "chrome-extension://abcdefghijklmnop",
            "http://notlocalhost",
            "http://localhost.",
        ] {
            assert!(!origin_is_local(origin), "{origin}");
        }
    }

    #[test]
    fn what_is_not_an_origin_is_refused() {
        for origin in [
            "",
            "null",
            "localhost",
            "localhost:3000",
            "127.0.0.1",
            "//localhost",
            "://localhost",
            "http://",
            "http:///",
            "not an origin",
            "http://local host",
        ] {
            assert!(!origin_is_local(origin), "{origin:?}");
        }
    }

    #[test]
    fn credentials_and_odd_authorities_are_refused() {
        for origin in [
            "http://localhost@evil.example",
            "http://evil.example@localhost",
            "http://user:pass@localhost",
            "http://localhost:@evil.example",
            "http://localhost:abc",
            "http://localhost:",
            "http://localhost:80:80",
            "http://[::1",
            "http://[::1]x",
            "http://[::1]:",
            "http://[::1]:port",
            "http://localhost:-1",
            "http://localhost:+1",
        ] {
            assert!(!origin_is_local(origin), "{origin}");
        }
    }

    #[test]
    fn a_path_after_the_authority_does_not_change_the_host() {
        // Not a real origin, but the host is what counts.
        assert!(origin_is_local("http://localhost/x"));
        assert!(!origin_is_local("http://evil.example/localhost"));
        assert!(!origin_is_local("http://evil.example?localhost"));
        assert!(!origin_is_local("http://evil.example#localhost"));
    }

    #[test]
    fn a_scheme_must_be_well_formed() {
        assert!(!origin_is_local("ht tp://localhost"));
        assert!(!origin_is_local("http_x://localhost"));
        assert!(!origin_is_local("😀://localhost"));
    }
}
