//! The stdin/stdout driver for the JSON-RPC server.
//!
//! JSON-RPC framing and dispatch stay with `jsonrpc_core`; this module only
//! moves bytes. It exists so the backend can push messages to the frontend
//! while a request is being handled: a single writer task owns stdout and is
//! fed by a channel, which both the request loop and any [`MessageSender`]
//! write into.

use futures::StreamExt;
use jsonrpc_core::{IoHandler, Notification, Params, Version};
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};
use tokio_util::codec::{FramedRead, LinesCodec};

/// An item on the outgoing queue.
///
/// The queue is FIFO, which is what makes shutdown ordering work: everything
/// enqueued before [`Outgoing::Shutdown`] is written before the writer task
/// stops.
enum Outgoing {
    Line(String),
    Shutdown,
}

/// A handle for sending JSON-RPC notifications to the connected frontend.
///
/// Cheap to clone; every clone feeds the same writer task, so messages never
/// interleave on the wire. Sending is non-blocking and infallible from the
/// caller's perspective: once the connection is gone, messages are dropped.
#[derive(Clone)]
pub struct MessageSender {
    outgoing: mpsc::UnboundedSender<Outgoing>,
}

impl MessageSender {
    /// Send a JSON-RPC notification to the frontend.
    ///
    /// Returns `false` if the message could not be queued because the
    /// connection has shut down. Callers that are themselves part of the
    /// logging path must not log that failure, or a dropped message turns into
    /// an infinite loop.
    pub fn notify<P: Serialize>(&self, method: &str, params: &P) -> bool {
        let Ok(serde_json::Value::Object(params)) = serde_json::to_value(params) else {
            // Every notification in the protocol takes named parameters. A type
            // that does not serialize to an object is a bug in the caller, not
            // something the frontend can act on.
            debug_assert!(false, "notification parameters must serialize to an object");
            return false;
        };

        let notification = Notification {
            jsonrpc: Some(Version::V2),
            method: method.to_string(),
            params: Params::Map(params),
        };

        let Ok(encoded) = serde_json::to_string(&notification) else {
            debug_assert!(false, "a Notification always serializes");
            return false;
        };

        self.outgoing.send(Outgoing::Line(encoded)).is_ok()
    }
}

impl std::fmt::Debug for MessageSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageSender")
            .field("connected", &!self.outgoing.is_closed())
            .finish()
    }
}

/// Runs the JSON-RPC server over stdin/stdout until stdin reaches EOF.
///
/// Requests are handled one at a time, in the order they arrive. Messages sent
/// through the returned [`MessageSender`] are written as soon as the writer
/// task picks them up, including while a request is still being handled.
pub(crate) async fn serve(io: IoHandler, incoming: Incoming) {
    serve_requests(io, tokio::io::stdin(), incoming).await;
}

/// [`serve`] with the request source injected, so tests can drive it without
/// touching the process' real stdin.
async fn serve_requests<R: AsyncRead + Unpin>(io: IoHandler, requests: R, incoming: Incoming) {
    let Incoming { outgoing, writer } = incoming;

    let mut framed_stdin = FramedRead::new(requests, LinesCodec::new());
    while let Some(line) = framed_stdin.next().await {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                tracing::warn!("failed to read a request from stdin: {err}");
                continue;
            }
        };

        // `handle_request` yields `None` for notifications, which by definition
        // have no response to write back.
        if let Some(response) = io.handle_request(&line).await {
            let _connected = outgoing.send(Outgoing::Line(response));
        }
    }

    // Stdin is closed, so no further requests can arrive. Enqueueing the
    // shutdown marker rather than dropping the channel is what guarantees that
    // messages sent moments ago are written first: the writer drains the queue
    // in order and only then stops. Note that clones of the `MessageSender`
    // outlive this function, so the channel closing is not a usable signal.
    let _connected = outgoing.send(Outgoing::Shutdown);
    let _joined = writer.await;
}

/// The receiving half of the outgoing channel, plus the writer task that drains
/// it. Created together with the [`MessageSender`] by [`channel`].
pub(crate) struct Incoming {
    outgoing: mpsc::UnboundedSender<Outgoing>,
    writer: tokio::task::JoinHandle<()>,
}

/// Create the outgoing message channel and spawn the task that writes it to
/// stdout.
///
/// This is separate from [`serve`] so the sender can be handed to the logging
/// layer before the server itself is built.
pub(crate) fn channel() -> (MessageSender, Incoming) {
    channel_to(tokio::io::stdout())
}

/// [`channel`] with the sink injected, so tests can observe what the writer
/// task produces without touching the process' real stdout.
fn channel_to<W: AsyncWrite + Send + Unpin + 'static>(sink: W) -> (MessageSender, Incoming) {
    let (outgoing, mut queued) = mpsc::unbounded_channel::<Outgoing>();

    let writer = tokio::spawn(async move {
        let mut sink = sink;
        while let Some(Outgoing::Line(message)) = queued.recv().await {
            if let Err(err) = write_line(&mut sink, &message).await {
                // stdout is the only channel back to the frontend, so there is
                // nowhere useful to report this. The frontend observes it as a
                // closed pipe.
                tracing::debug!("failed to write to stdout: {err}");
                break;
            }
        }
    });

    (
        MessageSender {
            outgoing: outgoing.clone(),
        },
        Incoming { outgoing, writer },
    )
}

/// Write a single message as one line, flushing so the frontend -- which may be
/// blocking on a response -- sees it immediately.
async fn write_line<W: AsyncWrite + Unpin>(
    sink: &mut W,
    message: &str,
) -> Result<(), std::io::Error> {
    // The frontend frames on newlines, so an embedded one would split the
    // message in two. Serialized JSON escapes newlines inside strings, leaving
    // only pretty-printing as a source of these.
    let mut line = message.replace('\n', "");
    line.push('\n');
    sink.write_all(line.as_bytes()).await?;
    sink.flush().await
}

#[cfg(test)]
mod tests {
    use jsonrpc_core::{IoHandler, Value};
    use tokio::io::{AsyncBufReadExt, BufReader};

    use super::{channel_to, serve_requests};

    /// Drive the server with `requests` and return the lines it wrote back.
    async fn exchange(io: IoHandler, requests: &str) -> Vec<String> {
        let (sink, collected) = tokio::io::duplex(8 * 1024);
        let (sender, incoming) = channel_to(sink);

        // The sender is what the logging layer would hold. Drop it here so the
        // only writes are the ones the handlers make.
        drop(sender);

        serve_requests(io, requests.as_bytes(), incoming).await;

        let mut lines = Vec::new();
        let mut reader = BufReader::new(collected).lines();
        while let Some(line) = reader
            .next_line()
            .await
            .expect("reading the sink cannot fail")
        {
            lines.push(line);
        }
        lines
    }

    /// The baseline the old `jsonrpc-stdio-server` loop provided: one request
    /// in, one response out, in order.
    #[tokio::test]
    async fn responds_to_requests_in_order() {
        let mut io = IoHandler::new();
        io.add_sync_method("echo", |params: jsonrpc_core::Params| {
            Ok(Value::String(format!("{params:?}")))
        });

        let lines = exchange(
            io,
            concat!(
                r#"{"jsonrpc":"2.0","id":1,"method":"echo","params":[]}"#,
                "\n",
                r#"{"jsonrpc":"2.0","id":2,"method":"echo","params":[]}"#,
                "\n",
            ),
        )
        .await;

        assert_eq!(lines.len(), 2, "expected one response per request");
        assert!(lines[0].contains(r#""id":1"#), "got {}", lines[0]);
        assert!(lines[1].contains(r#""id":2"#), "got {}", lines[1]);
    }

    /// The reason this driver replaced `jsonrpc-stdio-server`: a handler can
    /// push a notification to the frontend *before* it produces its response.
    /// The old loop awaited the handler and then wrote, so nothing could leave
    /// the process mid-request.
    #[tokio::test]
    async fn notifications_are_written_while_a_request_is_in_flight() {
        let (sink, collected) = tokio::io::duplex(8 * 1024);
        let (sender, incoming) = channel_to(sink);

        let mut io = IoHandler::new();
        let notifier = sender.clone();
        io.add_method("slow", move |_params| {
            let notifier = notifier.clone();
            async move {
                assert!(notifier.notify("log/message", &serde_json::json!({"step": "started"})));
                // Let the writer task run while this request is still pending.
                tokio::task::yield_now().await;
                assert!(notifier.notify("log/message", &serde_json::json!({"step": "finishing"})));
                Ok(Value::Bool(true))
            }
        });
        drop(sender);

        serve_requests(
            io,
            concat!(r#"{"jsonrpc":"2.0","id":7,"method":"slow"}"#, "\n").as_bytes(),
            incoming,
        )
        .await;

        let mut lines = Vec::new();
        let mut reader = BufReader::new(collected).lines();
        while let Some(line) = reader
            .next_line()
            .await
            .expect("reading the sink cannot fail")
        {
            lines.push(line);
        }

        assert_eq!(
            lines.len(),
            3,
            "two notifications and one response: {lines:?}"
        );
        assert!(lines[0].contains(r#""step":"started""#), "got {}", lines[0]);
        assert!(
            lines[1].contains(r#""step":"finishing""#),
            "got {}",
            lines[1]
        );
        assert!(
            lines[2].contains(r#""id":7"#),
            "the response must come last: {}",
            lines[2]
        );
    }

    /// Notifications emitted just before stdin closes must still reach the
    /// frontend. The sender is deliberately kept alive across the call, the way
    /// the logging layer holds one for the life of the process: the channel
    /// therefore never closes, and only the ordered shutdown marker stops the
    /// writer once the queue has drained.
    #[tokio::test]
    async fn queued_notifications_are_flushed_on_shutdown() {
        let (sink, collected) = tokio::io::duplex(8 * 1024);
        let (sender, incoming) = channel_to(sink);

        for step in 0..8 {
            assert!(sender.notify("log/message", &serde_json::json!({ "step": step })));
        }

        // No requests at all: stdin is already at EOF, so the loop goes
        // straight to shutdown.
        serve_requests(IoHandler::new(), &b""[..], incoming).await;
        drop(sender);

        let mut lines = Vec::new();
        let mut reader = BufReader::new(collected).lines();
        while let Some(line) = reader
            .next_line()
            .await
            .expect("reading the sink cannot fail")
        {
            lines.push(line);
        }

        assert_eq!(lines.len(), 8, "every queued notification must be flushed");
        assert!(lines[7].contains(r#""step":7"#), "got {}", lines[7]);
    }

    /// A notification carries no id, so `jsonrpc-core` produces no response for
    /// it. Writing an empty line would desync the frontend's line framing.
    #[tokio::test]
    async fn inbound_notifications_produce_no_response() {
        let mut io = IoHandler::new();
        io.add_notification("ping", |_params| {});

        let lines = exchange(io, concat!(r#"{"jsonrpc":"2.0","method":"ping"}"#, "\n")).await;

        assert!(lines.is_empty(), "expected no output, got {lines:?}");
    }
}
