//! HTTP client for `opencode serve`. Wraps `reqwest` with the few
//! endpoints aoe actually uses: health, list sessions, create
//! session, get session, prompt (async), reply to permission, and
//! the SSE event stream.
//!
//! The client is `Clone`able: it holds the `reqwest::Client` and a
//! parsed `url::Url`, both of which are cheap to clone. The long-lived
//! `opencode serve` process is the only thing that should hold a
//! connection; the client itself is stateless.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::{Client as HttpClient, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use url::Url;

use super::types::{Agent, Session, SessionStatus, Status as OpencodeStatus};

/// Errors that the opencode client can surface. The HTTP layer
/// (connect, TLS, timeout) is distinguished from a clean 4xx/5xx so
/// callers can decide whether to retry.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("opencode url is invalid: {0}")]
    BadUrl(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("non-2xx status: {status} body={body}")]
    Status { status: StatusCode, body: String },
    #[error("decode error: {0}")]
    Decode(#[from] serde_json::Error),
}

/// Re-exported alias so callers can write `Health` instead of the
/// longer `types::Status` (which would clash with
/// `SessionStatus`).
pub type Health = OpencodeStatus;

#[derive(Debug, Clone)]
pub struct Client {
    base: Url,
    http: HttpClient,
    auth: Option<(String, String)>,
    directory: Option<String>,
}

impl Client {
    /// Build a client for the given `opencode serve` URL. The URL
    /// must include scheme + host + port; the path is ignored (we
    /// always hit well-known absolute paths).
    pub fn new(base_url: &str) -> Result<Self, ClientError> {
        let base = Url::parse(base_url).map_err(|e| ClientError::BadUrl(e.to_string()))?;
        let http = HttpClient::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(ClientError::Http)?;
        Ok(Self {
            base,
            http,
            auth: None,
            directory: None,
        })
    }

    /// Attach HTTP basic-auth. Matches opencode's
    /// `OPENCODE_SERVER_USERNAME` / `OPENCODE_SERVER_PASSWORD` env
    /// vars.
    pub fn with_basic_auth(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.auth = Some((username.into(), password.into()));
        self
    }

    /// Scope project/session calls to one worktree while sharing a
    /// single global OpenCode server across all AoE tasks.
    pub fn with_directory(mut self, directory: impl Into<String>) -> Self {
        self.directory = Some(directory.into());
        self
    }

    /// Return the base URL, useful for logging and for AoE's
    /// structured-view "opencode server" indicator.
    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// Borrow the underlying `reqwest::Client` so the SSE consumer
    /// can build a streaming request with the same connection
    /// pool, headers, and timeouts. The borrow is tied to `&self`
    /// so callers can't outlive the `Client`.
    pub fn http_handle(&self) -> &HttpClient {
        &self.http
    }

    /// Borrow the configured basic-auth credentials, if any. The
    /// SSE consumer uses this to attach the same `Authorization`
    /// header the rest of the API calls do.
    pub fn auth_header(&self) -> Option<(&str, &str)> {
        self.auth.as_ref().map(|(u, p)| (u.as_str(), p.as_str()))
    }

    pub(crate) fn global_event_url(&self) -> Result<Url, ClientError> {
        self.url_for("/global/event")
    }

    /// `GET /global/health` is used by the supervisor to liveness-probe
    /// the opencode process. Faster and less side-effecty than a
    /// full session call.
    pub async fn health(&self) -> Result<Health, ClientError> {
        self.get_json("/global/health").await
    }

    /// `GET /session` lists all sessions on the server.
    pub async fn list_sessions(&self) -> Result<Vec<Session>, ClientError> {
        self.get_json("/session").await
    }

    /// `POST /session` creates a server-assigned session. OpenCode
    /// 1.17 ignores caller-provided ids, so reconnect uses the unique
    /// AoE title until upstream pre-assignment support lands.
    pub async fn create_session(&self, title: Option<&str>) -> Result<Session, ClientError> {
        #[derive(Serialize)]
        struct Body<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            title: Option<&'a str>,
        }
        let body = Body { title };
        self.post_json("/session", &body).await
    }

    /// `GET /session/:id` returns full session details.
    pub async fn get_session(&self, id: &str) -> Result<Session, ClientError> {
        self.get_json(&format!("/session/{id}")).await
    }

    /// `DELETE /session/:id` drops a session and all its data.
    pub async fn delete_session(&self, id: &str) -> Result<bool, ClientError> {
        let resp = self
            .request_without_body(Method::DELETE, &format!("/session/{id}"))
            .await?;
        self.decode_json(resp).await
    }

    /// `GET /session/status` returns `{ [id]: SessionStatus }` for every
    /// session. This is the fleet-view liveness feed.
    pub async fn session_status(&self) -> Result<Vec<(String, SessionStatus)>, ClientError> {
        // The server returns `{ [id]: SessionStatus }`; the keys
        // are dynamic so we decode through a generic map first.
        let raw: HashMapAny = self.get_json("/session/status").await?;
        let mut out = Vec::with_capacity(raw.0.len());
        for (k, v) in raw.0 {
            let status: SessionStatus = serde_json::from_value(v)?;
            out.push((k, status));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// `POST /session/:id/prompt_async` sends a fire-and-forget prompt.
    /// The session becomes busy; the agent streams tool/text parts
    /// on `/event`.
    pub async fn prompt_async(
        &self,
        id: &str,
        text: &str,
        model: Option<(&str, &str)>,
    ) -> Result<(), ClientError> {
        #[derive(Serialize)]
        struct Part<'a> {
            #[serde(rename = "type")]
            kind: &'a str,
            text: &'a str,
        }
        #[derive(Serialize)]
        struct Body<'a> {
            parts: Vec<Part<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            model: Option<Model<'a>>,
        }
        #[derive(Serialize)]
        struct Model<'a> {
            #[serde(rename = "providerID")]
            provider_id: &'a str,
            #[serde(rename = "modelID")]
            model_id: &'a str,
        }
        let body = Body {
            parts: vec![Part { kind: "text", text }],
            model: model.map(|(provider_id, model_id)| Model {
                provider_id,
                model_id,
            }),
        };
        let resp = self
            .request(
                Method::POST,
                &format!("/session/{id}/prompt_async"),
                Some(&body),
            )
            .await?;
        // 204 No Content is the documented success response.
        if !resp.status().is_success() {
            return Err(ClientError::Status {
                status: resp.status(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        Ok(())
    }

    /// `POST /session/:id/permissions/:permission_id` answers a
    /// permission/approval request. `response` is `"once"`,
    /// `"always"`, or `"reject"`.
    pub async fn reply_permission(
        &self,
        session_id: &str,
        permission_id: &str,
        response: &str,
    ) -> Result<bool, ClientError> {
        #[derive(Serialize)]
        struct Body<'a> {
            response: &'a str,
        }
        let body = Body { response };
        let resp = self
            .request(
                Method::POST,
                &format!("/session/{session_id}/permissions/{permission_id}"),
                Some(&body),
            )
            .await?;
        Ok(resp.status().is_success())
    }

    /// `POST /session/:id/abort` cancels the in-flight turn.
    pub async fn abort_session(&self, id: &str) -> Result<bool, ClientError> {
        let resp = self
            .request_without_body(Method::POST, &format!("/session/{id}/abort"))
            .await?;
        self.decode_json(resp).await
    }

    /// `GET /agent` lists available agents (build, plan, ...).
    pub async fn list_agents(&self) -> Result<Vec<Agent>, ClientError> {
        self.get_json("/agent").await
    }

    /// Build a [`super::events::EventStream`] over the `/event` SSE
    /// endpoint. The caller drives consumption via
    /// `EventStream::next()`.
    pub fn event_stream(&self) -> super::events::EventStream {
        super::events::EventStream::new(self.clone())
    }

    fn url_for(&self, path: &str) -> Result<Url, ClientError> {
        let trimmed = path.trim_start_matches('/');
        let mut url = self
            .base
            .join(trimmed)
            .map_err(|e| ClientError::BadUrl(format!("{path}: {e}")))?;
        if path != "/global/event" {
            if let Some(directory) = &self.directory {
                url.query_pairs_mut().append_pair("directory", directory);
            }
        }
        Ok(url)
    }

    fn build_headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some((u, p)) = &self.auth {
            let creds = format!("{u}:{p}");
            let encoded = base64_encode(creds.as_bytes());
            if let Ok(v) = HeaderValue::from_str(&format!("Basic {encoded}")) {
                h.insert(AUTHORIZATION, v);
            }
        }
        h
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let resp = self.request_without_body(Method::GET, path).await?;
        self.decode_json(resp).await
    }

    async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let resp = self.request(Method::POST, path, Some(body)).await?;
        self.decode_json(resp).await
    }

    async fn request<B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<reqwest::Response, ClientError> {
        let url = self.url_for(path)?;
        let mut req = self.http.request(method, url).headers(self.build_headers());
        req = req.timeout(Duration::from_secs(30));
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        Ok(resp)
    }

    async fn request_without_body(
        &self,
        method: Method,
        path: &str,
    ) -> Result<reqwest::Response, ClientError> {
        let url = self.url_for(path)?;
        let resp = self
            .http
            .request(method, url)
            .headers(self.build_headers())
            .timeout(Duration::from_secs(30))
            .send()
            .await?;
        Ok(resp)
    }

    async fn decode_json<T: DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, ClientError> {
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Status { status, body });
        }
        Ok(resp.json().await?)
    }
}

/// Helper: minimal base64 encoder (no external dep). Used only for
/// HTTP basic auth; ~30 LOC and saves a `base64` crate dep.
fn base64_encode(input: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHA[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

/// Wrapper so we can decode `{ "id1": {...}, "id2": {...} }` into a
/// sorted vec. The keys are session ids (server-assigned), the values
/// are `SessionStatus`.
#[derive(Debug, Default)]
struct HashMapAny(serde_json::Map<String, serde_json::Value>);

impl<'de> serde::Deserialize<'de> for HashMapAny {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let m: serde_json::Map<String, serde_json::Value> = serde_json::Map::deserialize(d)?;
        Ok(HashMapAny(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encodes_known_vectors() {
        // RFC 4648 §10 test vectors
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn client_new_rejects_garbage_url() {
        assert!(Client::new("not a url").is_err());
        assert!(Client::new("http://127.0.0.1:4096").is_ok());
        assert!(Client::new("https://example.com/").is_ok());
    }

    #[test]
    fn url_for_joins_paths_safely() {
        let c = Client::new("http://127.0.0.1:4096/").unwrap();
        assert_eq!(
            c.url_for("/global/health").unwrap().as_str(),
            "http://127.0.0.1:4096/global/health"
        );
        // No trailing slash on base.
        let c = Client::new("http://127.0.0.1:4096").unwrap();
        assert_eq!(
            c.url_for("/global/health").unwrap().as_str(),
            "http://127.0.0.1:4096/global/health"
        );
    }

    #[test]
    fn url_for_scopes_requests_by_directory() {
        let c = Client::new("http://127.0.0.1:4096")
            .unwrap()
            .with_directory("/tmp/a project");
        assert_eq!(
            c.url_for("/session").unwrap().as_str(),
            "http://127.0.0.1:4096/session?directory=%2Ftmp%2Fa+project"
        );
        assert_eq!(
            c.global_event_url().unwrap().as_str(),
            "http://127.0.0.1:4096/global/event"
        );
    }

    #[test]
    fn with_basic_auth_attaches_authorization_header() {
        let c = Client::new("http://127.0.0.1:4096")
            .unwrap()
            .with_basic_auth("alice", "secret");
        let h = c.build_headers();
        let got = h.get(AUTHORIZATION).unwrap().to_str().unwrap();
        // base64("alice:secret") = "YWxpY2U6c2VjcmV0"
        assert_eq!(got, "Basic YWxpY2U6c2VjcmV0");
    }
}
