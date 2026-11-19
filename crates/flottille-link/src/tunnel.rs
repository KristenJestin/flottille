//! A tunnel's connections on a link (`docs/remote.md`, "Tunnels"): each TCP connection is one
//! stream of the link, opened by one line (`{"tunnel": "<id>"}`) and raw bytes either way after
//! it. The transport already gives an ordered, flow-controlled stream with its own end in each
//! direction, which is all a TCP connection is: framing the bytes (as a bundle's chunks are, in
//! JSON) would only add a copy and a base64 expansion to every page.

use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::link::{Link, LinkError};
use crate::transport::Raw;

/// One connection of a tunnel: the bytes of a TCP connection, both ways. Shutting down its
/// writing ends what the other side reads; dropped, the stream ends both ways.
pub struct TunnelStream(Raw);

impl TunnelStream {
    pub(crate) fn new(raw: Raw) -> TunnelStream {
        TunnelStream(raw)
    }
}

impl AsyncRead for TunnelStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for TunnelStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl Link {
    /// Opens one connection of the tunnel `id` on this link, a client link: the other side takes
    /// it only for a tunnel it opened for this machine, and closes it otherwise.
    pub async fn tunnel(&self, id: &str) -> Result<TunnelStream, LinkError> {
        let header = serde_json::to_vec(&serde_json::json!({ "tunnel": id }))
            .expect("a tunnel's header is JSON");
        self.conn
            .open_raw(&header)
            .await
            .map(TunnelStream)
            .map_err(|failure| self.lost(failure))
    }
}
