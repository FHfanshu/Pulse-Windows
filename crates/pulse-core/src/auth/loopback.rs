// Ported from upstream Auth/LoopbackCallback.swift (NWListener -> tokio TcpListener on 127.0.0.1).
//! The other half of a browser sign-in: a listener on this PC that the provider redirects back to,
//! carrying the authorization code.
//!
//! Loopback only, for one request, and gone the moment it has what it came for. **Binding is a
//! separate step from waiting**: the port is only known once the listener is bound, and the
//! redirect address, which goes into the authorize request before the browser opens, is built
//! from it. Cancelling a sign-in drops the future, which drops the listener.
//!
//! Unlike the macOS listener this one binds `127.0.0.1` only; `localhost` in the redirect address
//! resolves there (browsers fall back from `::1`).

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::util::query_decode;
use super::Failure;
use crate::i18n;

pub struct LoopbackCallback {
    listener: TcpListener,
    port: u16,
    path: String,
}

/// What the browser is told, and whether this request settled the sign-in.
#[derive(Debug, PartialEq)]
pub struct Handled {
    /// An upstream English string, localized when the page is drawn.
    pub message: &'static str,
    pub outcome: Option<Result<String, Failure>>,
}

impl LoopbackCallback {
    /// Binds the listener. A provider whose client is registered for one specific loopback
    /// address gets that or nothing (`PortBusy`); the rest take whatever is free.
    pub async fn bind(fixed: Option<u16>, path: &str) -> Result<Self, Failure> {
        let wanted = fixed.unwrap_or(0);
        let listener = TcpListener::bind(("127.0.0.1", wanted)).await.map_err(|_| Failure::PortBusy(wanted))?;
        let port = listener.local_addr().map(|a| a.port()).map_err(|_| Failure::PortBusy(wanted))?;
        Ok(Self { listener, port, path: path.to_string() })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Resolves once the browser comes back with the code.
    ///
    /// The `state` is checked here rather than by the caller: a redirect that does not carry the
    /// value this sign-in generated did not come from this sign-in, and the code in it is not
    /// ours to use. The wait is bounded because nothing here can tell a sign-in still being typed
    /// from one that ended on the provider's own error page, which never reaches this listener.
    pub async fn await_code(self, state: &str, patience: Duration, lang: &str) -> Result<String, Failure> {
        let (settled, mut results) = mpsc::unbounded_channel();
        let deadline = tokio::time::sleep(patience);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let (path, state, lang, settled) = (self.path.clone(), state.to_string(), lang.to_string(), settled.clone());
                    tokio::spawn(async move { serve(stream, &path, &state, &lang, settled).await });
                }
                Some(result) = results.recv() => return result,
                () = &mut deadline => return Err(Failure::TimedOut),
            }
        }
    }
}

/// Reads one request, answers it, and reports a result if it settled the sign-in.
async fn serve(
    mut stream: TcpStream,
    path: &str,
    expected_state: &str,
    lang: &str,
    settled: mpsc::UnboundedSender<Result<String, Failure>>,
) {
    let Some(line) = tokio::time::timeout(Duration::from_secs(10), request_line(&mut stream)).await.ok().flatten() else {
        return;
    };
    let handled = handle_request_line(&line, path, expected_state);
    let _ = stream.write_all(response(i18n::t(lang, handled.message, &[]).as_str()).as_bytes()).await;
    let _ = stream.shutdown().await;
    // After the answer is written, so the browser has its page before the listener goes.
    if let Some(outcome) = handled.outcome {
        let _ = settled.send(outcome);
    }
}

/// The request line is all this needs and it arrives in the first packet; reading stops at the end
/// of the headers (or at 8 KiB) so the browser is not left writing into a socket nobody reads.
async fn request_line(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        buffer.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&buffer);
        if read == 0 || text.contains("\r\n\r\n") || buffer.len() >= 8 * 1024 {
            return text.split("\r\n").next().filter(|l| !l.is_empty()).map(String::from);
        }
    }
}

/// Returns what to tell the browser, and settles the sign-in.
pub fn handle_request_line(line: &str, path: &str, expected_state: &str) -> Handled {
    let mut parts = line.split(' ').filter(|p| !p.is_empty());
    let (_method, Some(target)) = (parts.next(), parts.next()) else {
        return Handled { message: "This page can be closed.", outcome: None };
    };
    let (target_path, query) = target.split_once('?').unwrap_or((target, ""));
    if target_path != path {
        return Handled { message: "This page can be closed.", outcome: None };
    }

    let value = |name: &str| -> Option<String> {
        query.split('&').find_map(|pair| {
            let (key, raw) = pair.split_once('=').unwrap_or((pair, ""));
            (key == name).then(|| query_decode(raw))
        })
    };

    let failed = "Sign-in failed. You can close this page.";
    if let Some(error) = value("error_description").or_else(|| value("error")) {
        return Handled { message: failed, outcome: Some(Err(Failure::Refused(error))) };
    }
    match value("code") {
        Some(code) if value("state").as_deref() == Some(expected_state) => Handled {
            message: "Signed in. You can close this page and go back to Pulse.",
            outcome: Some(Ok(code)),
        },
        _ => Handled { message: failed, outcome: Some(Err(Failure::Cancelled)) },
    }
}

fn response(message: &str) -> String {
    let message = message.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Pulse</title>\n\
         <body style=\"font:16px system-ui,'Segoe UI',sans-serif;display:grid;place-items:center;height:90vh;margin:0\">\n\
         <p>{message}</p>\n"
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_state_and_code_settles_the_sign_in() {
        let handled = handle_request_line("GET /callback?code=abc%2B123&state=xyz HTTP/1.1", "/callback", "xyz");
        assert_eq!(handled.outcome, Some(Ok("abc+123".into())));
        assert_eq!(handled.message, "Signed in. You can close this page and go back to Pulse.");
    }

    #[test]
    fn a_wrong_or_missing_state_is_not_ours_to_use() {
        for line in ["GET /callback?code=abc&state=other HTTP/1.1", "GET /callback?code=abc HTTP/1.1", "GET /callback?state=xyz HTTP/1.1"] {
            let handled = handle_request_line(line, "/callback", "xyz");
            assert_eq!(handled.outcome, Some(Err(Failure::Cancelled)), "{line}");
        }
    }

    #[test]
    fn the_providers_own_words_are_decoded_from_the_query() {
        let handled = handle_request_line("GET /callback?error=access_denied&error_description=User+declined HTTP/1.1", "/callback", "xyz");
        assert_eq!(handled.outcome, Some(Err(Failure::Refused("User declined".into()))));
        let bare = handle_request_line("GET /callback?error=access_denied&state=xyz HTTP/1.1", "/callback", "xyz");
        assert_eq!(bare.outcome, Some(Err(Failure::Refused("access_denied".into()))));
    }

    #[test]
    fn other_paths_settle_nothing() {
        let handled = handle_request_line("GET /favicon.ico HTTP/1.1", "/callback", "xyz");
        assert_eq!(handled, Handled { message: "This page can be closed.", outcome: None });
        assert!(handle_request_line("", "/callback", "xyz").outcome.is_none());
    }

    async fn browse(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes()).await.unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        reply
    }

    #[tokio::test]
    async fn the_listener_answers_the_browser_and_returns_the_code() {
        let listener = LoopbackCallback::bind(None, "/callback").await.unwrap();
        let port = listener.port();
        assert_ne!(port, 0);
        let waiting = tokio::spawn(listener.await_code("S", Duration::from_secs(10), "en"));

        // A favicon request first: answered, but it does not settle anything.
        assert!(browse(port, "/favicon.ico").await.contains("This page can be closed."));
        let page = browse(port, "/callback?code=THE-CODE&state=S").await;
        assert!(page.starts_with("HTTP/1.1 200 OK"));
        assert!(page.contains("Signed in. You can close this page"));
        assert_eq!(waiting.await.unwrap(), Ok("THE-CODE".into()));
    }

    #[tokio::test]
    async fn waiting_is_bounded_and_a_busy_port_is_reported() {
        let first = LoopbackCallback::bind(None, "/callback").await.unwrap();
        let busy = LoopbackCallback::bind(Some(first.port()), "/callback").await;
        assert!(matches!(busy, Err(Failure::PortBusy(p)) if p == first.port()));
        assert_eq!(first.await_code("S", Duration::from_millis(50), "en").await, Err(Failure::TimedOut));
    }
}
