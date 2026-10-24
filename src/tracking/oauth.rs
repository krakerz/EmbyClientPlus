//! Loopback redirect target for MAL / AniList logins: a tiny HTTP server on
//! `localhost:PORT` that lives only while a login waits for the browser.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Below every OS's ephemeral port range; one port, a path per service.
pub const PORT: u16 = 29417;
pub const TIMEOUT: Duration = Duration::from_secs(300);

const MAX_REQUEST: usize = 16 * 1024;

pub fn redirect_uri(service: &str) -> String {
    format!("http://localhost:{PORT}/callback/{service}")
}

/// 48 random bytes, base64url without padding: 64 chars of [A-Za-z0-9_-]
/// (a valid PKCE verifier, 43-128) — also used for `state`.
pub fn random_token() -> String {
    let mut bytes = [0u8; 48];
    getrandom::fill(&mut bytes).expect("system random source");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Opens `url` in the default browser, on the GTK thread.
pub fn open_browser(url: String) {
    gtk::glib::MainContext::default().invoke(move || {
        if let Err(e) =
            gtk::gio::AppInfo::launch_default_for_uri(&url, None::<&gtk::gio::AppLaunchContext>)
        {
            tracing::warn!("could not open the browser: {e}");
        }
    });
}

const DONE_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>Emby Client+</title>\
<p>Logged in to Emby Client+. You can close this tab.</p>";

const RELAY_PAGE: &str = r#"<!doctype html><meta charset="utf-8"><title>Emby Client+</title>
<p id="m">Finishing login…</p>
<script>
fetch('/callback/anilist/token', {method: 'POST',
  headers: {'Content-Type': 'application/x-www-form-urlencoded'},
  body: location.hash.slice(1)})
  .then(r => { document.getElementById('m').textContent = r.ok
    ? 'Logged in to Emby Client+. You can close this tab.'
    : 'Login failed; try again from the app.'; });
</script>
"#;

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: HashMap<String, String>,
}

struct Response {
    status: &'static str,
    body: String,
}

impl Response {
    fn new(status: &'static str, body: impl Into<String>) -> Response {
        Response {
            status,
            body: body.into(),
        }
    }

    fn bytes(&self) -> Vec<u8> {
        format!(
            "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
            self.status,
            self.body.len(),
            self.body
        )
        .into_bytes()
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn page(text: &str) -> String {
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Emby Client+</title><p>{}</p>",
        escape(text)
    )
}

fn pairs(query: &str) -> HashMap<String, String> {
    reqwest::Url::parse(&format!("http://localhost/?{query}"))
        .map(|url| url.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

/// What a request does to the login: `None` keeps waiting.
fn route(
    request: &Request,
    service: &str,
    state: &str,
) -> (Response, Option<Result<HashMap<String, String>>>) {
    let callback = format!("/callback/{service}");
    if request.method == "GET" && request.path == callback {
        if request.query.is_empty() && service == "anilist" {
            // The token is in the URL fragment; the page posts it back.
            return (Response::new("200 OK", RELAY_PAGE), None);
        }
        if let Some(error) = request.query.get("error") {
            let detail = request
                .query
                .get("error_description")
                .unwrap_or(error)
                .clone();
            return (
                Response::new("200 OK", page(&format!("Login cancelled: {detail}"))),
                Some(Err(anyhow!("Login cancelled: {detail}"))),
            );
        }
        if request.query.get("state").map(String::as_str) != Some(state) {
            return (
                Response::new(
                    "400 Bad Request",
                    page("This login link is stale; start again from the app"),
                ),
                Some(Err(anyhow!("The login link was stale"))),
            );
        }
        return (
            Response::new("200 OK", DONE_PAGE),
            Some(Ok(request.query.clone())),
        );
    }
    if service == "anilist" && request.method == "POST" && request.path == "/callback/anilist/token"
    {
        if let Some(origin) = request.headers.get("origin")
            && *origin != format!("http://localhost:{PORT}")
        {
            return (Response::new("403 Forbidden", "forbidden"), None);
        }
        if request
            .body
            .get("state")
            .is_some_and(|got| got.as_str() != state)
        {
            return (
                Response::new("400 Bad Request", "stale"),
                Some(Err(anyhow!("The login link was stale"))),
            );
        }
        if !request.body.contains_key("access_token") {
            return (
                Response::new("400 Bad Request", "no token"),
                Some(Err(anyhow!("AniList sent no token"))),
            );
        }
        return (
            Response::new("200 OK", "ok"),
            Some(Ok(request.body.clone())),
        );
    }
    (Response::new("404 Not Found", "not found"), None)
}

pub struct Listener {
    listeners: Vec<TcpListener>,
}

impl Listener {
    /// Binds 127.0.0.1:PORT and [::1]:PORT (either may fail alone, e.g. no
    /// IPv6); none bound is an error.
    pub fn bind() -> Result<Listener> {
        let mut listeners = Vec::new();
        match TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, PORT))) {
            Ok(listener) => listeners.push(listener),
            Err(e) if e.kind() == ErrorKind::AddrInUse => {
                bail!("Port {PORT} is in use by another program; close it and try again")
            }
            Err(e) => tracing::debug!("could not bind 127.0.0.1:{PORT}: {e}"),
        }
        match TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, PORT))) {
            Ok(listener) => listeners.push(listener),
            Err(e) => tracing::debug!("could not bind [::1]:{PORT}: {e}"),
        }
        if listeners.is_empty() {
            bail!("Could not listen on port {PORT} for the login");
        }
        Ok(Listener { listeners })
    }

    /// Blocking (run in `tokio::task::spawn_blocking`): serves requests until
    /// `service`'s callback arrives with `state`, or `timeout` passes. The
    /// sockets are dropped on return (port closed again).
    pub fn wait(
        self,
        service: &str,
        state: &str,
        timeout: Duration,
    ) -> Result<HashMap<String, String>> {
        for listener in &self.listeners {
            listener.set_nonblocking(true)?;
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            for listener in &self.listeners {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => continue,
                    Err(e) => {
                        tracing::debug!("login listener accept: {e}");
                        continue;
                    }
                };
                if let Some(result) = serve(stream, service, state) {
                    return result;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!("No answer from the browser within 5 minutes")
    }
}

fn serve(
    mut stream: TcpStream,
    service: &str,
    state: &str,
) -> Option<Result<HashMap<String, String>>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let request = read_request(&mut stream).ok()?;
    let (response, outcome) = route(&request, service, state);
    let _ = stream.write_all(&response.bytes());
    let _ = stream.flush();
    outcome
}

fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if data.len() > MAX_REQUEST {
            bail!("request too large");
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            bail!("connection closed");
        }
        data.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_owned();
    let target = first.next().unwrap_or("/").to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if length > MAX_REQUEST {
        bail!("body too large");
    }
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(parse_request(
        method,
        &target,
        headers,
        &String::from_utf8_lossy(&body),
    ))
}

fn parse_request(
    method: String,
    target: &str,
    headers: HashMap<String, String>,
    body: &str,
) -> Request {
    let (path, query) = match reqwest::Url::parse(&format!("http://localhost{target}")) {
        Ok(url) => (
            url.path().to_owned(),
            url.query_pairs().into_owned().collect(),
        ),
        Err(_) => (target.to_owned(), HashMap::new()),
    };
    Request {
        method,
        path,
        query,
        headers,
        body: pairs(body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(target: &str) -> Request {
        parse_request("GET".into(), target, HashMap::new(), "")
    }

    fn post(target: &str, origin: Option<&str>, body: &str) -> Request {
        let headers = origin
            .map(|o| HashMap::from([("origin".to_owned(), o.to_owned())]))
            .unwrap_or_default();
        parse_request("POST".into(), target, headers, body)
    }

    #[test]
    fn mal_callback_with_good_state() {
        let (_, outcome) = route(&get("/callback/mal?code=abc&state=s1"), "mal", "s1");
        let query = outcome.unwrap().unwrap();
        assert_eq!(query.get("code").map(String::as_str), Some("abc"));
    }

    #[test]
    fn bad_state_and_denied_are_errors() {
        let (response, outcome) = route(&get("/callback/mal?code=abc&state=nope"), "mal", "s1");
        assert!(outcome.unwrap().is_err());
        assert!(response.status.starts_with("400"));
        let (_, outcome) = route(&get("/callback/mal?error=access_denied"), "mal", "s1");
        assert!(outcome.unwrap().is_err());
    }

    #[test]
    fn anilist_relay_and_token_post() {
        let (response, outcome) = route(&get("/callback/anilist"), "anilist", "s1");
        assert!(outcome.is_none());
        assert!(response.body.contains("location.hash"));
        let body = "access_token=tok&token_type=Bearer&expires_in=100&state=s1";
        let (response, outcome) = route(
            &post(
                "/callback/anilist/token",
                Some("http://localhost:29417"),
                body,
            ),
            "anilist",
            "s1",
        );
        assert!(response.status.starts_with("200"));
        let query = outcome.unwrap().unwrap();
        assert_eq!(query.get("access_token").map(String::as_str), Some("tok"));
    }

    #[test]
    fn foreign_origin_is_refused() {
        let (response, outcome) = route(
            &post(
                "/callback/anilist/token",
                Some("http://evil.example"),
                "access_token=tok&state=s1",
            ),
            "anilist",
            "s1",
        );
        assert!(response.status.starts_with("403"));
        assert!(outcome.is_none());
    }

    #[test]
    fn unknown_paths_are_404() {
        let (response, outcome) = route(&get("/favicon.ico"), "mal", "s1");
        assert!(response.status.starts_with("404"));
        assert!(outcome.is_none());
    }

    #[test]
    fn random_token_shape() {
        let a = random_token();
        assert_eq!(a.len(), 64);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(a, random_token());
    }
}
