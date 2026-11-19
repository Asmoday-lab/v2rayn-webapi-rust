//! SSE events endpoint (Api/WebApiEndpoints.cs:277-405).
//!
//! The C# EventHub publishes 15 event types driven by Core/subscription/
//! speedtest activity. The headless Termux build has no such internal activity
//! in the MVP, so the stream keeps the connection alive with the exact C#
//! heartbeat (`: keep-alive\n\n` every 60s). Session authentication (one-shot
//! sse_ticket) is enforced by `security::security_middleware` before this
//! handler runs.

use axum::response::sse::{Event, KeepAlive, Sse};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::IntervalStream;

/// GET /api/events — authenticated via one-shot `?sse_ticket=` in middleware.
pub async fn events() -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    // C# uses a 1-minute PeriodicTimer and writes `: keep-alive\n\n`.
    let interval = tokio::time::interval(Duration::from_secs(60));
    let stream = IntervalStream::new(interval).map(|_| Ok(Event::default().comment("keep-alive")));
    // Push axum's built-in keepalive probe far out so only the C#-faithful
    // 60s heartbeat from our own stream is emitted on the wire.
    let probe = KeepAlive::new().interval(Duration::from_secs(86400));
    Sse::new(stream).keep_alive(probe)
}
