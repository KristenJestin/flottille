//! The size of what a connection's server answers. rmcp sets no limit on the body it reads for
//! an answer over HTTP, and on stdio drops an oversized line without saying so, so the call waits
//! for its 120 seconds: the limit is applied here, while the bytes are read, to one message (a
//! line of the server's output, the body or the event stream of one HTTP answer), never to what a
//! long-lived server has said in all.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use http::{HeaderName, HeaderValue};
use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, SseError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};
use sse_stream::{Sse, SseStream};
use tokio::io::{AsyncRead, ReadBuf};

/// The most bytes of one answer of a server, whatever the transport: above the largest result
/// that is kept (4 images of 2 MiB at most and 5 MiB in all, then the text), `docs/extensions.md`,
/// "Connections", "A result".
pub(super) const ANSWER_LIMIT: usize = 16 * 1024 * 1024;

/// What a call says of an answer that was cut.
pub(super) fn said() -> String {
    format!(
        "the server's answer passed {} MB and was cut off",
        ANSWER_LIMIT >> 20
    )
}

/// Set by a reader or a client that cut an answer for its size: the error that ends the call
/// says only that the transport closed or failed, and the connection is told apart by this.
#[derive(Clone, Default)]
pub(super) struct Cut(Arc<AtomicBool>);

impl Cut {
    fn mark(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether an answer was cut since it was last asked.
    pub(super) fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

/// A server's output, failing once a line of it, whose end is the end of its message, is past
/// `ANSWER_LIMIT` bytes. The stream is then left in the middle of a message: the server is
/// stopped, never read again.
pub(super) struct Lines<R> {
    inner: R,
    /// Bytes of the line that is being read.
    since: usize,
    cut: Cut,
    failed: bool,
}

impl<R> Lines<R> {
    pub(super) fn new(inner: R, cut: Cut) -> Self {
        Self {
            inner,
            since: 0,
            cut,
            failed: false,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Lines<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.failed {
            return Poll::Ready(Err(io::Error::other(said())));
        }
        let before = buf.filled().len();
        ready!(Pin::new(&mut self.inner).poll_read(cx, buf))?;
        let mut since = self.since;
        for part in buf.filled()[before..].split_inclusive(|byte| *byte == b'\n') {
            let ended = part.last() == Some(&b'\n');
            since += part.len() - usize::from(ended);
            if since > ANSWER_LIMIT {
                self.failed = true;
                self.cut.mark();
                // A read that fails has given nothing.
                buf.set_filled(before);
                return Poll::Ready(Err(io::Error::other(said())));
            }
            if ended {
                since = 0;
            }
        }
        self.since = since;
        Poll::Ready(Ok(()))
    }
}

/// The HTTP client of a connection's server: reqwest's, but for the answer to a message it sends,
/// which is read up to `ANSWER_LIMIT` bytes. The stream a server keeps open for what it says of
/// itself, which has no end, is left to rmcp's own limit on one event of it (set to the same).
#[derive(Clone)]
pub(super) struct Bounded {
    client: reqwest::Client,
    cut: Cut,
}

impl Bounded {
    pub(super) fn new(client: reqwest::Client, cut: Cut) -> Self {
        Self { client, cut }
    }

    /// The error of an answer that passed the limit.
    fn too_big(&self) -> StreamableHttpError<reqwest::Error> {
        self.cut.mark();
        StreamableHttpError::UnexpectedServerResponse(Cow::Owned(said()))
    }

    /// The body of `response`: refused at once when it says it is past the limit, and otherwise
    /// failing at the first byte past it, whatever it said.
    async fn body(
        &self,
        response: &mut reqwest::Response,
    ) -> Result<Vec<u8>, StreamableHttpError<reqwest::Error>> {
        if response
            .content_length()
            .is_some_and(|length| length > ANSWER_LIMIT as u64)
        {
            return Err(self.too_big());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > ANSWER_LIMIT {
                return Err(self.too_big());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// The bytes of `response`, as many in all as the limit lets through (the events of an answer are
/// counted together, not one by one), then an error.
fn chunks(response: reqwest::Response, cut: Cut) -> impl Stream<Item = io::Result<Bytes>> {
    futures::stream::unfold(
        (response, 0usize, false),
        move |(mut response, taken, done)| {
            let cut = cut.clone();
            async move {
                if done {
                    return None;
                }
                match response.chunk().await {
                    Ok(Some(chunk)) if taken + chunk.len() > ANSWER_LIMIT => {
                        cut.mark();
                        Some((Err(io::Error::other(said())), (response, taken, true)))
                    }
                    Ok(Some(chunk)) => {
                        let taken = taken + chunk.len();
                        Some((Ok(chunk), (response, taken, false)))
                    }
                    Ok(None) => None,
                    Err(error) => Some((Err(io::Error::other(error)), (response, taken, true))),
                }
            }
        },
    )
}

impl StreamableHttpClient for Bounded {
    type Error = reqwest::Error;

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.client
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_token,
                custom_headers,
                ANSWER_LIMIT,
            )
            .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.client
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_token,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        self.client
            .delete_session(uri, session, auth_token, custom_headers)
            .await
    }

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_token,
            custom_headers,
            ANSWER_LIMIT,
        )
        .await
    }

    /// What rmcp's own client does of a message, written again because rmcp has no hook on the
    /// answer's size: the same requests, checks and errors, but for the limit on what is read, and
    /// for rmcp's legacy discovery, which is not used (the daemon starts a server with
    /// `initialize`).
    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        _max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        use reqwest::StatusCode;
        use reqwest::header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE};
        let mut request = self
            .client
            .post(uri.as_ref())
            .header(ACCEPT, "text/event-stream, application/json");
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        for (name, value) in custom_headers {
            // The headers the transport sets itself are not the user's to set, as in rmcp's client
            // (its check is not public).
            if ["accept", "mcp-session-id", "last-event-id"]
                .iter()
                .any(|reserved| name.as_str().eq_ignore_ascii_case(reserved))
            {
                return Err(StreamableHttpError::ReservedHeaderConflict(
                    name.to_string(),
                ));
            }
            request = request.header(name, value);
        }
        let attached = session_id.is_some();
        if let Some(session) = session_id {
            request = request.header("Mcp-Session-Id", session.as_ref());
        }
        let mut response = request.json(&message).send().await?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED
            && let Some(challenge) = response.headers().get(WWW_AUTHENTICATE)
        {
            let challenge = challenge.to_str().map_err(|_| {
                StreamableHttpError::UnexpectedServerResponse(Cow::Borrowed(
                    "invalid www-authenticate header value",
                ))
            })?;
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                challenge.to_owned(),
            )));
        }
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == StatusCode::NOT_FOUND && attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
        let session = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let is = |kind: &str| {
            content_type
                .as_deref()
                .is_some_and(|ct| ct.starts_with(kind))
        };
        let notice = !matches!(message, ClientJsonRpcMessage::Request(_));
        // Some servers answer a notification with an empty 200.
        if status.is_success() && response.content_length() == Some(0) && notice {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if !status.is_success() {
            let body = self.body(&mut response).await?;
            let body = String::from_utf8_lossy(&body);
            if status != StatusCode::UNAUTHORIZED
                && status != StatusCode::FORBIDDEN
                && is("application/json")
                && let Ok(error @ JsonRpcMessage::Error(_)) =
                    serde_json::from_str::<ServerJsonRpcMessage>(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(error, session));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {body}"),
            )));
        }
        if is("text/event-stream") {
            let events = SseStream::from_bytes_stream(chunks(response, self.cut.clone()));
            return Ok(StreamableHttpPostResponse::Sse(events.boxed(), session));
        }
        if is("application/json") {
            let body = self.body(&mut response).await?;
            return match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                Ok(parsed) => Ok(StreamableHttpPostResponse::Json(parsed, session)),
                // A notification needs no reply: an unusable body is as good as none.
                Err(_) if notice => Ok(StreamableHttpPostResponse::Accepted),
                Err(error) => Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                    format!("could not parse the JSON answer: {error}"),
                ))),
            };
        }
        Err(StreamableHttpError::UnexpectedContentType(content_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// A server's output of `lines`, each `size` bytes and a newline: what reading it all gives, and
    /// whether an answer was cut.
    async fn read(lines: usize, size: usize) -> (io::Result<usize>, bool) {
        let mut output = Vec::new();
        for _ in 0..lines {
            output.extend(std::iter::repeat_n(b'x', size));
            output.push(b'\n');
        }
        let cut = Cut::default();
        let mut reader = Lines::new(io::Cursor::new(output), cut.clone());
        let read = reader.read_to_end(&mut Vec::new()).await;
        (read, cut.take())
    }

    #[tokio::test]
    async fn the_lines_of_a_server_are_never_counted_together() {
        let (read, cut) = read(5, ANSWER_LIMIT / 2).await;
        assert_eq!(read.unwrap(), 5 * (ANSWER_LIMIT / 2 + 1));
        assert!(!cut);
    }

    #[tokio::test]
    async fn a_line_of_the_size_of_the_limit_is_read() {
        let (read, cut) = read(1, ANSWER_LIMIT).await;
        assert!(read.is_ok() && !cut);
    }

    #[tokio::test]
    async fn a_line_past_the_limit_fails_the_read_and_says_it_was_cut() {
        let (read, cut) = read(1, ANSWER_LIMIT + 1).await;
        assert!(read.unwrap_err().to_string().contains("was cut off"));
        assert!(cut);
    }

    #[tokio::test]
    async fn a_line_past_the_limit_without_its_end_fails_the_read() {
        let cut = Cut::default();
        let endless = io::Cursor::new(vec![b'x'; ANSWER_LIMIT + 1]);
        let read = Lines::new(endless, cut.clone())
            .read_to_end(&mut Vec::new())
            .await;
        assert!(read.is_err() && cut.take());
    }

    #[test]
    fn a_cut_is_told_once() {
        let cut = Cut::default();
        assert!(!cut.take());
        cut.mark();
        assert!(cut.take());
        assert!(!cut.take(), "a cut that was told is forgotten");
    }
}
