//! Incremental parser for the `opencode serve` `/event` SSE stream.

use std::pin::Pin;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use thiserror::Error;

use super::client::Client;
use super::types::{Event, GlobalEvent};

const FRAME_CAP: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum StreamError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("opencode event endpoint returned {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("decode error: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("event frame too large: {size} bytes")]
    FrameTooLarge { size: usize },
}

type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

pub struct EventStream {
    client: Client,
    stream: Option<ByteStream>,
    bytes: Vec<u8>,
    data: String,
    connected: bool,
    closed: bool,
}

impl EventStream {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            stream: None,
            bytes: Vec::new(),
            data: String::new(),
            connected: false,
            closed: false,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub async fn next(&mut self) -> Option<Result<GlobalEvent, StreamError>> {
        if self.closed {
            return None;
        }
        if self.stream.is_none() {
            if let Err(error) = self.open().await {
                self.closed = true;
                return Some(Err(error));
            }
        }

        loop {
            if let Some(result) = self.consume_lines() {
                return Some(result);
            }

            let stream = self.stream.as_mut().expect("opened above");
            match stream.next().await {
                Some(Ok(chunk)) => self.bytes.extend_from_slice(&chunk),
                Some(Err(error)) => {
                    self.closed = true;
                    return Some(Err(StreamError::Http(error)));
                }
                None => {
                    self.closed = true;
                    if self.data.is_empty() {
                        return None;
                    }
                    return Some(self.finish_frame());
                }
            }
        }
    }

    async fn open(&mut self) -> Result<(), StreamError> {
        let url = self.client.global_event_url().map_err(|error| {
            StreamError::Decode(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                error,
            )))
        })?;
        let mut request = self
            .client
            .http_handle()
            .get(url)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache");
        if let Some((username, password)) = self.client.auth_header() {
            request = request.basic_auth(username, Some(password));
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(StreamError::Status { status, body });
        }
        self.stream = Some(Box::pin(response.bytes_stream()));
        Ok(())
    }

    fn consume_lines(&mut self) -> Option<Result<GlobalEvent, StreamError>> {
        loop {
            let newline = self.bytes.iter().position(|byte| *byte == b'\n')?;
            let mut line = self.bytes.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    return Some(self.finish_frame());
                }
                continue;
            }
            if let Some(piece) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(piece.trim_start());
                if self.data.len() > FRAME_CAP {
                    return Some(Err(StreamError::FrameTooLarge {
                        size: self.data.len(),
                    }));
                }
            }
        }
    }

    fn finish_frame(&mut self) -> Result<GlobalEvent, StreamError> {
        let data = std::mem::take(&mut self.data);
        let event: GlobalEvent = serde_json::from_str(&data)?;
        if matches!(event.payload, Event::ServerConnected { .. }) {
            self.connected = true;
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_sse_frame() {
        let client = Client::new("http://127.0.0.1:4096").unwrap();
        let mut stream = EventStream::new(client);
        stream.bytes.extend_from_slice(
            b"data: {\"directory\":\"/tmp/p\",\"payload\":{\"type\":\"session.status\",\"properties\":{\"sessionID\":\"s1\",",
        );
        assert!(stream.consume_lines().is_none());
        stream
            .bytes
            .extend_from_slice(b"\"status\":{\"type\":\"busy\"}}}}\n\n");
        let event = stream.consume_lines().unwrap().unwrap();
        assert_eq!(event.directory.as_deref(), Some("/tmp/p"));
        assert!(matches!(event.payload, Event::SessionStatus { .. }));
    }

    #[test]
    fn server_connected_sets_flag() {
        let client = Client::new("http://127.0.0.1:4096").unwrap();
        let mut stream = EventStream::new(client);
        stream.bytes.extend_from_slice(
            b"data: {\"payload\":{\"type\":\"server.connected\",\"properties\":{}}}\n\n",
        );
        let event = stream.consume_lines().unwrap().unwrap();
        assert!(matches!(event.payload, Event::ServerConnected { .. }));
        assert!(stream.is_connected());
    }
}
