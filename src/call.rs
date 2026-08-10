use std::sync::Arc;

use bytes::Bytes;
use ext_php_rs::boxed::ZBox;
use ext_php_rs::prelude::*;
use ext_php_rs::types::{ArrayKey, ZendCallable, ZendHashTable, ZendObject, Zval};
use http::uri::PathAndQuery;
use parking_lot::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;

use crate::channel::GrpcChannel;
use crate::codec::RawBytesCodec;
use crate::credentials::GrpcCallCredentials;
use crate::error::GrpcError;
use crate::runtime::get_runtime;
use crate::timeval::GrpcTimeval;

// OP_* constants (matching the C extension values)
const OP_SEND_INITIAL_METADATA: i64 = 0;
const OP_SEND_MESSAGE: i64 = 1;
const OP_SEND_CLOSE_FROM_CLIENT: i64 = 2;
const OP_RECV_INITIAL_METADATA: i64 = 4;
const OP_RECV_MESSAGE: i64 = 5;
const OP_RECV_STATUS_ON_CLIENT: i64 = 6;

/// Response-message buffer between the stream-driving tokio task and the PHP
/// thread. Capacity 1 forces a park/wake round trip (~10µs) per message, which
/// dominates high-count streams; a moderate bound lets the driver run ahead
/// while still applying backpressure (worst case: N × max message size held).
const STREAM_MSG_BUFFER: usize = 32;

/// Trailing metadata + status from a completed stream.
struct StreamTrailers {
    code: i32,
    message: String,
    metadata: tonic::metadata::MetadataMap,
}

/// Active streaming state — channels to a background tokio task
/// that drives the tonic stream. Used for server, client, and bidi streaming.
struct ActiveStream {
    /// Send messages to the stream task (client/bidi streaming only).
    /// `None` for server streaming. Dropping closes the request stream.
    msg_tx: Option<tokio::sync::mpsc::Sender<Bytes>>,
    /// Receives messages from the stream task. `Ok(None)` = end of stream.
    msg_rx: tokio::sync::mpsc::Receiver<Result<Option<Bytes>, tonic::Status>>,
    /// Initial metadata from response headers (taken on first RECV_INITIAL_METADATA).
    initial_metadata: Option<tonic::metadata::MetadataMap>,
    /// Deferred initial metadata receiver (for bidi/client streaming where headers
    /// arrive after the stream opens, not immediately).
    meta_rx: Option<tokio::sync::oneshot::Receiver<tonic::metadata::MetadataMap>>,
    /// Cached trailers (populated when we see end-of-stream or error from msg_rx).
    cached_trailers: Option<StreamTrailers>,
    /// Receives trailers from the stream task after it finishes.
    trailers_rx: Option<tokio::sync::oneshot::Receiver<StreamTrailers>>,
}

/// Parsed operations from the PHP batch array.
struct BatchOps {
    send_metadata: Vec<(String, String)>,
    has_send_initial_metadata: bool,
    send_message: Option<Bytes>,
    send_close: bool,
    recv_initial_metadata: bool,
    recv_message: bool,
    recv_status: bool,
    has_send_ops: bool,
    has_recv_ops: bool,
}

/// Helper to extract a string key from an ArrayKey.
fn array_key_to_string(key: &ArrayKey<'_>) -> Option<String> {
    match key {
        ArrayKey::String(s) => Some(s.clone()),
        ArrayKey::Str(s) => Some((*s).to_string()),
        ArrayKey::ZendString(zs) => zs.as_str().ok().map(ToString::to_string),
        ArrayKey::Long(_) => None,
    }
}

/// Helper to extract an i64 key from an ArrayKey.
fn array_key_to_long(key: &ArrayKey<'_>) -> Result<i64, GrpcError> {
    match key {
        ArrayKey::Long(n) => Ok(*n),
        ArrayKey::String(s) => s
            .parse::<i64>()
            .map_err(|_| GrpcError::InvalidArg(format!("invalid op key: {s}"))),
        ArrayKey::Str(s) => s
            .parse::<i64>()
            .map_err(|_| GrpcError::InvalidArg(format!("invalid op key: {s}"))),
        ArrayKey::ZendString(zs) => zs
            .as_str()
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| GrpcError::InvalidArg("invalid op key".into())),
    }
}

/// Wall-clock microseconds since the Unix epoch.
pub(crate) fn now_usec() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0i64, |d| d.as_micros() as i64)
}

/// Map a tonic status onto the (code, message) pair the C extension reports.
///
/// tonic surfaces an expired client deadline as CANCELLED "Timeout expired"
/// (it maps `TimeoutExpired` that way, dropping the source so the cause cannot
/// be recovered from the status itself), where ext-grpc reports
/// DEADLINE_EXCEEDED "Deadline Exceeded". Callers branch on that code —
/// google/gax retries DEADLINE_EXCEEDED and gives up on CANCELLED — so a
/// CANCELLED arriving after the deadline we set is reported as the deadline.
/// Cancellation through `Call::cancel()` never reaches here; it is answered
/// from the cancellation token instead.
/// True when a real deadline is set and has already passed. ext-grpc fails
/// such calls locally as DEADLINE_EXCEEDED without ever sending the request;
/// racing a minimal timeout against a fast response is not equivalent.
fn deadline_already_expired(deadline_usec: i64) -> bool {
    deadline_usec > 0 && deadline_usec < i64::MAX && deadline_usec <= now_usec()
}

fn status_to_php(status: &tonic::Status, deadline_usec: i64) -> (i32, String) {
    let code = status.code() as i32;
    let deadline_set = deadline_usec > 0 && deadline_usec < i64::MAX;

    if code == tonic::Code::Cancelled as i32 && deadline_set && now_usec() >= deadline_usec {
        return (4, "Deadline Exceeded".to_string());
    }

    (code, status.message().to_string())
}

/// Invoke a CallCredentials plugin callable on the PHP thread.
///
/// Returns a vec of (key, value) metadata pairs.
fn invoke_call_plugin(
    plugin: &Arc<Mutex<Option<Zval>>>,
    service_url: &str,
) -> Result<Vec<(String, String)>, GrpcError> {
    let guard = plugin.lock();
    let zval = guard
        .as_ref()
        .ok_or_else(|| GrpcError::CallbackFailed("plugin callable is not set".into()))?;

    let callable = ZendCallable::new(zval)
        .map_err(|_| GrpcError::CallbackFailed("stored value is not callable".into()))?;

    // IMPORTANT: never Debug- or Display-format `ext_php_rs::error::Error` here.
    // The `Exception` variant formats the thrown PHP object graph, which is
    // cyclic (exception traces reference objects that reference the exception)
    // and ext-php-rs's derived Debug has no cycle detection — formatting it
    // recurses until the stack overflows (SIGSEGV). Extract the class name and
    // message properties ourselves instead.
    let result = callable
        .try_call(vec![&service_url.to_string()])
        .map_err(|e| {
            let msg = match &e {
                ext_php_rs::error::Error::Exception(obj) => {
                    let class = obj
                        .get_class_name()
                        .unwrap_or_else(|_| "Exception".to_string());
                    // `$message` is protected — call the public getter instead.
                    let message = obj
                        .try_call_method("getMessage", vec![])
                        .ok()
                        .and_then(|zv| zv.string())
                        .unwrap_or_default();
                    format!("plugin threw {class}: {message}")
                }
                other => format!("{other}"),
            };
            GrpcError::CallbackFailed(msg)
        })?;

    // The callback returns metadata as key => value or key => [values]
    // (Google Cloud SDK returns e.g. ['authorization' => ['Bearer xxx']])
    if let Some(ht) = result.array() {
        Ok(parse_metadata(ht))
    } else {
        Ok(Vec::new())
    }
}

/// True when a metadata key is legal per gRPC header rules (the check
/// ext-grpc applies via `grpc_header_key_is_legal`): non-empty, lowercase
/// alphanumerics plus `-` `_` `.`.
fn header_key_is_legal(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.'))
}

/// Strict parser for OP_SEND_INITIAL_METADATA, matching ext-grpc's
/// `create_metadata_array`: shape must be `array<string, list<string>>` with
/// legal header keys; anything else throws InvalidArgumentException.
/// (Plugin return values go through the lenient `parse_metadata` instead —
/// their failure mode is a failed call, not a thrown argument error.)
fn parse_send_metadata(ht: &ZendHashTable) -> PhpResult<Vec<(String, String)>> {
    let mut metadata = Vec::new();
    for (key, val) in ht.iter() {
        let k = array_key_to_string(&key)
            .ok_or_else(|| crate::error::invalid_argument("Bad metadata value given"))?;
        if !header_key_is_legal(&k) {
            return Err(crate::error::invalid_argument("Bad metadata value given"));
        }
        let values = val
            .array()
            .ok_or_else(|| crate::error::invalid_argument("Bad metadata value given"))?;
        for (_idx, v) in values.iter() {
            let s = v
                .zend_str()
                .ok_or_else(|| crate::error::invalid_argument("Bad metadata value given"))?;
            metadata.push((k.clone(), String::from_utf8_lossy(s.as_bytes()).into_owned()));
        }
    }
    Ok(metadata)
}

/// Parse metadata from a PHP array (ZendHashTable).
fn parse_metadata(ht: &ZendHashTable) -> Vec<(String, String)> {
    let mut metadata = Vec::new();
    for (key, val) in ht.iter() {
        if let Some(k) = array_key_to_string(&key) {
            // Metadata values can be arrays (multiple values per key) or strings
            if let Some(arr) = val.array() {
                for (_idx, v) in arr.iter() {
                    if let Some(s) = v.string() {
                        metadata.push((k.clone(), s));
                    }
                }
            } else if let Some(s) = val.string() {
                metadata.push((k, s));
            }
        }
    }
    metadata
}

/// A single metadata entry as raw bytes, grouped per key in iteration order.
///
/// ASCII values are surfaced as their underlying byte slice (HTTP/2 wire form,
/// which is also the human-readable form for ASCII keys). Binary (`-bin`) values
/// are surfaced as the decoded raw bytes, matching `ext-grpc`'s observable
/// behavior, where PHP user code receives the unmodified payload as a (binary
/// safe) PHP string.
type MetadataEntries = Vec<(String, Vec<Vec<u8>>)>;

/// Collect a tonic `MetadataMap` into a flat list of `(key, values)` pairs.
///
/// Pure function with no PHP dependencies; unit-tested below. The PHP-binding
/// layer (`metadata_to_php`) is a thin wrapper that converts the result into a
/// `ZendHashTable`.
///
/// Behaviors:
///  - Iteration order of the original `MetadataMap` is preserved.
///  - Repeated keys are collapsed into a `Vec<Vec<u8>>`, matching the historic
///    "each key maps to an array of values" PHP shape.
///  - For `KeyAndValueRef::Binary`, the value is decoded via `to_bytes()`
///    (tonic's API for the decoded payload; `as_encoded_bytes()` would return
///    the base64 wire form, which we do NOT want). If a malformed binary value
///    fails to decode, that single entry is skipped rather than failing the
///    whole call: matches the ASCII branch's prior tolerance of `to_str()`
///    failures.
fn collect_metadata(map: &tonic::metadata::MetadataMap) -> MetadataEntries {
    let mut entries: MetadataEntries = Vec::new();
    for key_and_value in map.iter() {
        let (key_str, bytes): (&str, Vec<u8>) = match key_and_value {
            tonic::metadata::KeyAndValueRef::Ascii(key, value) => {
                // Preserve the prior behavior of dropping ASCII values that
                // fail `to_str()` (non-visible-ASCII). In practice tonic only
                // admits visible-ASCII bytes into `MetadataValue<Ascii>`, so
                // this branch effectively always succeeds; keeping `to_str()`
                // here makes the patch purely additive with respect to the
                // ASCII path.
                match value.to_str() {
                    Ok(s) => (key.as_str(), s.as_bytes().to_vec()),
                    Err(_) => continue,
                }
            }
            tonic::metadata::KeyAndValueRef::Binary(key, value) => {
                // Binary trailers (e.g. `grpc-status-details-bin`) carry
                // base64-encoded payloads on the wire. `to_bytes()` returns the
                // decoded bytes, exactly what ext-grpc surfaces to PHP.
                // `as_encoded_bytes()` would return the base64 wire form, which
                // we do NOT want.
                match value.to_bytes() {
                    Ok(decoded) => (key.as_str(), decoded.to_vec()),
                    // Drop malformed binary values rather than failing the
                    // whole map; mirrors the ASCII branch's tolerance pattern.
                    Err(_) => continue,
                }
            }
        };

        // Group by key, preserving the order in which keys are first seen.
        if let Some(slot) = entries.iter_mut().find(|(k, _)| k == key_str) {
            slot.1.push(bytes);
        } else {
            entries.push((key_str.to_owned(), vec![bytes]));
        }
    }
    entries
}

/// Convert a slice of raw bytes into a PHP string (binary-safe) wrapped in a
/// `Zval`. PHP strings are length-prefixed and may contain arbitrary bytes
/// including null bytes; same shape `ext-grpc` produces for binary trailers.
fn bytes_to_php_string_zval(bytes: &[u8]) -> Zval {
    let mut zv = Zval::new();
    // `ZendStr::new` takes `impl AsRef<[u8]>` and copies the bytes into a
    // Zend-allocated string; no UTF-8 validation involved.
    zv.set_zend_string(ext_php_rs::types::ZendStr::new(bytes, false));
    zv
}

/// Build a metadata array for PHP from a tonic MetadataMap.
///
/// Output shape: `array<string, array<int, string>>`; each metadata key maps
/// to an ordered list of values. ASCII values are surfaced as their raw bytes
/// (typically printable ASCII). Binary (`-bin`) values are surfaced as the
/// decoded raw payload in a PHP binary-safe string, matching `ext-grpc`.
/// Transport/protocol headers that C-core consumes before surfacing metadata
/// to PHP. tonic leaves them in (response headers wholesale; raw trailer maps
/// on the success path), so filter at the PHP boundary. Note that
/// `grpc-status-details-bin` is NOT reserved — ext-grpc surfaces it.
fn is_reserved_header(key: &str) -> bool {
    matches!(
        key,
        "content-type" | "grpc-status" | "grpc-message" | "grpc-encoding" | "grpc-accept-encoding"
    )
}

fn metadata_to_php(map: &tonic::metadata::MetadataMap) -> Result<ZBox<ZendHashTable>, GrpcError> {
    let mut ht = ZendHashTable::new();
    for (key, values) in collect_metadata(map) {
        if is_reserved_header(&key) {
            continue;
        }
        let mut arr = ZendHashTable::new();
        for v in &values {
            arr.push(bytes_to_php_string_zval(v))
                .map_err(|e| GrpcError::InvalidArg(format!("metadata build: {e}")))?;
        }
        ht.insert(key.as_str(), arr)
            .map_err(|e| GrpcError::InvalidArg(format!("metadata build: {e}")))?;
    }
    Ok(ht)
}

/// Reconstruct the metadata that ext-grpc exposes for an error status.
///
/// tonic treats `grpc-status-details-bin` as a transport header: when it
/// creates a `Status` from response trailers, it decodes that header into
/// `Status::details()` and removes it from `Status::metadata()`.  ext-grpc
/// instead exposes the decoded bytes as ordinary trailing metadata, so put it
/// back before converting the metadata to PHP.
fn status_metadata_for_php(status: &tonic::Status) -> tonic::metadata::MetadataMap {
    let mut metadata = status.metadata().clone();
    if !status.details().is_empty() {
        metadata.insert_bin(
            "grpc-status-details-bin",
            tonic::metadata::MetadataValue::from_bytes(status.details()),
        );
    }
    metadata
}

/// Async result from a gRPC call.
type CallResult = (
    Option<tonic::metadata::MetadataMap>,
    Option<Bytes>,
    Option<tonic::metadata::MetadataMap>,
    i32,
    String,
);

#[php_class]
#[php(name = "Grpc\\Call")]
pub struct GrpcCall {
    channel: Channel,
    method: String,
    target: String,
    deadline_usec: i64,
    #[allow(dead_code)] // reserved for :authority override support
    host_override: Option<String>,
    call_plugin: Option<Arc<Mutex<Option<Zval>>>>,
    cancel_token: CancellationToken,
    max_decoding_message_size: Option<usize>,
    max_encoding_message_size: Option<usize>,
    /// Buffered send data from a send-only startBatch (split send/recv pattern).
    pending_metadata: Vec<(String, String)>,
    pending_message: Option<Bytes>,
    /// Whether OP_SEND_CLOSE_FROM_CLIENT has been seen.
    send_closed: bool,
    /// Whether OP_SEND_INITIAL_METADATA has been seen (a second one is a
    /// sequencing error, like C-core's GRPC_CALL_ERROR_TOO_MANY_OPERATIONS).
    initial_metadata_sent: bool,
    /// Whether a RECV_STATUS_ON_CLIENT has been delivered — the call is over;
    /// any further batch is a LogicException like ext-grpc (never a re-run).
    finished: bool,
    /// Set when the originating PHP Grpc\Channel is closed; startBatch must
    /// then fail with RuntimeException like ext-grpc.
    channel_closed: Arc<std::sync::atomic::AtomicBool>,
    /// Shared transport handle — call outcomes update the channel's
    /// connectivity state (READY on responses, TRANSIENT_FAILURE on
    /// connection errors).
    channel_shared: Option<Arc<crate::channel::SharedChannel>>,
    /// Active streaming state (None for unary or not yet started).
    stream_state: Option<ActiveStream>,
}

#[php_impl]
impl GrpcCall {
    /// Constructs a new Call.
    pub fn __construct(
        channel: &GrpcChannel,
        method: String,
        deadline: &GrpcTimeval,
        host_override: Option<String>,
    ) -> PhpResult<Self> {
        let tonic_channel = channel.get_tonic_channel().ok_or_else(|| {
            crate::error::invalid_argument("Call cannot be constructed from a closed Channel")
        })?;

        let target = channel.get_target_uri().unwrap_or_default();

        let call_plugin = channel.get_call_plugin();
        let max_decoding_message_size = channel.get_max_decoding_message_size();
        let max_encoding_message_size = channel.get_max_encoding_message_size();

        Ok(Self {
            channel: tonic_channel,
            method,
            target,
            deadline_usec: deadline.to_absolute_usec(),
            host_override,
            call_plugin,
            max_decoding_message_size,
            max_encoding_message_size,
            cancel_token: CancellationToken::new(),
            pending_metadata: Vec::new(),
            pending_message: None,
            send_closed: false,
            initial_metadata_sent: false,
            finished: false,
            channel_closed: channel.get_closed_flag(),
            channel_shared: channel.get_shared(),
            stream_state: None,
        })
    }

    /// Starts a batch of operations.
    ///
    /// Supports both unary and streaming RPCs by detecting the calling pattern:
    /// - Unary: recv_message + recv_status in the same batch
    /// - Server streaming: recv_message without recv_status (multiple startBatch calls)
    ///
    /// For streaming, a background tokio task drives the tonic stream and
    /// communicates with PHP via channels. State persists across startBatch calls.
    #[php(name = "startBatch")]
    pub fn start_batch(&mut self, ops: &ZendHashTable) -> PhpResult<ZBox<ZendObject>> {
        if self.channel_closed.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::error::runtime_exception(
                "startBatch Error. Channel is closed",
            ));
        }
        if self.cancel_token.is_cancelled() {
            return Err(PhpException::from(GrpcError::Status {
                code: 1, // CANCELLED
                message: "Call has been cancelled".into(),
            }));
        }

        let batch = self.parse_ops(ops)?;

        // A batch with no recognized ops does nothing (C-core returns OK
        // without touching the call).
        if !batch.has_send_ops && !batch.has_recv_ops {
            return Ok(ZendObject::new_stdclass());
        }

        // Once RECV_STATUS_ON_CLIENT was delivered the call is over — any
        // further batch is a sequencing error in ext-grpc, never a re-run.
        if self.finished {
            return Err(crate::error::logic_exception(
                "start_batch was called incorrectly",
                8, // GRPC_CALL_ERROR_TOO_MANY_OPERATIONS
            ));
        }

        if batch.has_send_initial_metadata {
            if self.initial_metadata_sent {
                return Err(crate::error::logic_exception(
                    "start_batch was called incorrectly",
                    8, // GRPC_CALL_ERROR_TOO_MANY_OPERATIONS
                ));
            }
            self.initial_metadata_sent = true;
        }

        // Track SEND_CLOSE_FROM_CLIENT
        if batch.send_close {
            self.send_closed = true;
        }

        let sent_md = batch.has_send_initial_metadata;
        let sent_msg = batch.send_message.is_some();
        let sent_close = batch.send_close;
        let recv_status = batch.recv_status;

        let mut result_obj = self.dispatch_batch(batch)?;

        if recv_status {
            self.finished = true;
        }

        // ext-grpc acks each completed send op with a boolean on the result.
        for (done, prop) in [
            (sent_md, "send_metadata"),
            (sent_msg, "send_message"),
            (sent_close, "send_close"),
        ] {
            if done {
                result_obj.set_property(prop, true).map_err(
                    |e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set {prop}: {e}"))
                    },
                )?;
            }
        }

        Ok(result_obj)
    }

    /// Returns the peer address in C-core's transport format
    /// (ipv4:IP:port / ipv6:[..]:port). host_override never appears here.
    #[php(name = "getPeer")]
    pub fn get_peer(&self) -> String {
        let target = self.target.clone();
        let Ok(rt) = get_runtime() else {
            return target;
        };
        let resolved = rt.block_on(async {
            tokio::net::lookup_host(target.as_str())
                .await
                .ok()
                .and_then(|mut addrs| addrs.next())
        });
        match resolved {
            Some(std::net::SocketAddr::V4(a)) => format!("ipv4:{a}"),
            Some(std::net::SocketAddr::V6(a)) => format!("ipv6:{a}"),
            None => target,
        }
    }

    /// Cancels the call.
    pub fn cancel(&mut self) {
        self.cancel_token.cancel();
    }

    /// Sets call credentials.
    #[php(name = "setCredentials")]
    pub fn set_credentials(&mut self, creds: &GrpcCallCredentials) -> i64 {
        self.call_plugin = Some(Arc::clone(&creds.plugin));
        0 // CALL_OK
    }
}

impl GrpcCall {
    /// Route a parsed batch to the unary/streaming machinery.
    fn dispatch_batch(&mut self, mut batch: BatchOps) -> PhpResult<ZBox<ZendObject>> {
        // ── CASE 1: Send-only batch — buffer, forward to stream, or start bidi ──
        if batch.has_send_ops && !batch.has_recv_ops {
            if self.stream_state.is_some() {
                // Active bidi stream — forward message and/or close
                if let Some(msg) = batch.send_message {
                    self.send_stream_message(msg)?;
                }
                if batch.send_close {
                    self.close_stream_send();
                }
            } else if !batch.send_close && batch.send_message.is_some() {
                // First SEND_MESSAGE without SEND_CLOSE — start bidi/client stream
                let send_metadata = std::mem::take(&mut self.pending_metadata);
                let mut plugin_metadata = Vec::new();
                if let Some(ref plugin) = self.call_plugin {
                    plugin_metadata =
                        invoke_call_plugin(plugin, &self.method).map_err(PhpException::from)?;
                }
                self.start_bidi_stream(send_metadata, plugin_metadata)?;
                if let Some(msg) = batch.send_message {
                    self.send_stream_message(msg)?;
                }
            } else {
                // Buffer for unary or server streaming (existing behavior)
                self.pending_metadata = batch.send_metadata;
                self.pending_message = batch.send_message;

                if let Some(ref plugin) = self.call_plugin {
                    let plugin_md =
                        invoke_call_plugin(plugin, &self.method).map_err(PhpException::from)?;
                    self.pending_metadata.extend(plugin_md);
                }

                // Half-closed split start (gax UnaryCall / ServerStreamingCall
                // start()): dispatch the request now so N async calls overlap
                // on the wire instead of each paying its full round trip in
                // wait() (issue #18). The recv batch collects the in-flight
                // result via the stream machinery — a unary response is just
                // a stream of one message plus trailers.
                if batch.send_close {
                    let send_metadata = std::mem::take(&mut self.pending_metadata);
                    let send_message = self.pending_message.take();
                    self.start_server_stream(send_metadata, Vec::new(), send_message)?;
                }
            }

            return Ok(ZendObject::new_stdclass());
        }

        // ── CASE 2: Stream already active — handle send ops + read from it ──
        if self.stream_state.is_some() {
            // Extract send data before borrowing batch for the result
            let send_msg = if batch.has_send_ops {
                batch.send_message.take()
            } else {
                None
            };
            let do_close = batch.has_send_ops && batch.send_close;

            if let Some(msg) = send_msg {
                self.send_stream_message(msg)?;
            }
            if do_close {
                self.close_stream_send();
            }
            return self.build_stream_result(&batch);
        }

        // ── CASE 3: No active stream, has recv ops — decide unary vs streaming ──

        // Capture recv flags before moving fields out of batch
        let recv_initial_metadata = batch.recv_initial_metadata;
        let recv_message = batch.recv_message;
        let recv_status = batch.recv_status;
        let has_send_ops = batch.has_send_ops;

        // Resolve metadata and message (from this batch or buffered)
        let send_metadata = if has_send_ops {
            batch.send_metadata
        } else {
            std::mem::take(&mut self.pending_metadata)
        };

        let send_message = if has_send_ops {
            batch.send_message
        } else {
            self.pending_message.take()
        };

        let mut plugin_metadata = Vec::new();
        if has_send_ops && let Some(ref plugin) = self.call_plugin {
            plugin_metadata =
                invoke_call_plugin(plugin, &self.method).map_err(PhpException::from)?;
        }

        // Unary pattern: recv_message + recv_status in the same batch
        if recv_message && recv_status {
            let result = self.execute_call(send_metadata, plugin_metadata, send_message)?;
            return self.build_unary_result(
                recv_initial_metadata,
                recv_message,
                recv_status,
                result,
            );
        }

        // Server streaming pattern: recv ops without recv_status, send already closed
        if self.send_closed {
            self.start_server_stream(send_metadata, plugin_metadata, send_message)?;
            // build_stream_result still uses BatchOps — create a minimal one for the recv flags
            let recv_batch = BatchOps {
                send_metadata: Vec::new(),
                has_send_initial_metadata: false,
                send_message: None,
                send_close: false,
                recv_initial_metadata,
                recv_message,
                recv_status,
                has_send_ops: false,
                has_recv_ops: true,
            };
            return self.build_stream_result(&recv_batch);
        }

        // Fallback: treat as unary (handles edge cases like recv_status alone)
        let result = self.execute_call(send_metadata, plugin_metadata, send_message)?;
        self.build_unary_result(recv_initial_metadata, recv_message, recv_status, result)
    }

    /// Execute the gRPC unary call in the tokio runtime.
    fn execute_call(
        &self,
        send_metadata: Vec<(String, String)>,
        plugin_metadata: Vec<(String, String)>,
        send_message: Option<Bytes>,
    ) -> PhpResult<CallResult> {
        let rt = get_runtime().map_err(PhpException::from)?;

        let channel = self.channel.clone();
        let method = self.method.clone();
        let deadline_usec = self.deadline_usec;
        let cancel_token = self.cancel_token.clone();
        let max_decoding = self.max_decoding_message_size;
        let max_encoding = self.max_encoding_message_size;

        if deadline_already_expired(deadline_usec) {
            return Ok((
                None,
                None,
                Some(tonic::metadata::MetadataMap::default()),
                4, // DEADLINE_EXCEEDED
                "Deadline Exceeded".to_string(),
            ));
        }

        let result: Result<CallResult, GrpcError> = rt.block_on(async move {
            // Build the path
            let path = PathAndQuery::try_from(method.as_str())
                .map_err(|e| GrpcError::InvalidArg(format!("invalid method path: {e}")))?;

            // Build the request
            let message = send_message.unwrap_or_default();
            let mut request = tonic::Request::new(message);

            // Apply metadata
            let req_metadata = request.metadata_mut();
            for (key, value) in &send_metadata {
                if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                    && let Ok(val) = value.parse()
                {
                    req_metadata.insert(name, val);
                }
            }
            for (key, value) in &plugin_metadata {
                if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                    && let Ok(val) = value.parse()
                {
                    req_metadata.insert(name, val);
                }
            }

            // Apply deadline/timeout
            if deadline_usec < i64::MAX && deadline_usec > 0 {
                // A deadline already in the past still sets a minimal timeout
                // so the call reports DEADLINE_EXCEEDED (via status_to_php)
                // like ext-grpc, instead of running without one.
                let timeout_usec = deadline_usec.saturating_sub(now_usec()).max(1);
                request.set_timeout(std::time::Duration::from_micros(timeout_usec as u64));
            }

            // Make the unary call using the raw codec, with cancellation support
            let mut grpc_client = tonic::client::Grpc::new(channel);
            if let Some(limit) = max_decoding {
                grpc_client = grpc_client.max_decoding_message_size(limit);
            }
            if let Some(limit) = max_encoding {
                grpc_client = grpc_client.max_encoding_message_size(limit);
            }
            grpc_client.ready().await.map_err(GrpcError::Transport)?;

            let call_future = grpc_client.unary(request, path, RawBytesCodec);

            // Race the gRPC call against the cancellation token
            tokio::select! {
                response = call_future => {
                    match response {
                        Ok(resp) => {
                            let (resp_metadata, body, _extensions) = resp.into_parts();
                            // Return the body even when empty — the C-based grpc extension
                            // returns "" for 0-byte messages, and PHP libraries depend on that.
                            Ok((Some(resp_metadata), Some(body), None, 0i32, String::new()))
                        }
                        Err(status) => {
                            let (code, msg) = status_to_php(&status, deadline_usec);
                            let md = status_metadata_for_php(&status);
                            Ok((None, None, Some(md), code, msg))
                        }
                    }
                }
                () = cancel_token.cancelled() => {
                    Err(GrpcError::Status {
                        code: 1,
                        message: "Call cancelled".into(),
                    })
                }
            }
        });

        if let Some(ref shared) = self.channel_shared {
            let next = match &result {
                Ok((_, _, _, code, _)) if *code == 14 => {
                    crate::channel::CHANNEL_TRANSIENT_FAILURE
                }
                Ok(_) => crate::channel::CHANNEL_READY,
                Err(_) => crate::channel::CHANNEL_TRANSIENT_FAILURE,
            };
            *shared.state.lock() = next;
        }

        result.map_err(PhpException::from)
    }

    /// Parse the ops array into a structured BatchOps.
    fn parse_ops(&self, ops: &ZendHashTable) -> PhpResult<BatchOps> {
        let mut batch = BatchOps {
            send_metadata: Vec::new(),
            has_send_initial_metadata: false,
            send_message: None,
            send_close: false,
            recv_initial_metadata: false,
            recv_message: false,
            recv_status: false,
            has_send_ops: false,
            has_recv_ops: false,
        };

        for (key, val) in ops.iter() {
            let op_code = array_key_to_long(&key).map_err(PhpException::from)?;

            match op_code {
                OP_SEND_INITIAL_METADATA => {
                    batch.has_send_ops = true;
                    batch.has_send_initial_metadata = true;
                    let ht = val.array().ok_or_else(|| {
                        crate::error::invalid_argument("Bad metadata value given")
                    })?;
                    batch.send_metadata = parse_send_metadata(ht)?;
                }
                OP_SEND_MESSAGE => {
                    batch.has_send_ops = true;
                    // ext-grpc contract: value must be ['message' => string,
                    // 'flags' => int (optional)]. Message bytes are binary —
                    // use zend_str() (raw bytes) instead of string() which
                    // rejects non-UTF-8 data.
                    let ht = val.array().ok_or_else(|| {
                        crate::error::invalid_argument("Expected an array for send message")
                    })?;
                    if let Some(flags) = ht.get("flags")
                        && flags.long().is_none()
                    {
                        return Err(crate::error::invalid_argument(
                            "Expected an int for message flags",
                        ));
                    }
                    let zs = ht
                        .get("message")
                        .and_then(|z| z.zend_str())
                        .ok_or_else(|| {
                            crate::error::invalid_argument("Expected a string for send message")
                        })?;
                    batch.send_message = Some(Bytes::copy_from_slice(zs.as_bytes()));
                }
                OP_SEND_CLOSE_FROM_CLIENT => {
                    batch.has_send_ops = true;
                    batch.send_close = true;
                }
                OP_RECV_INITIAL_METADATA => {
                    batch.has_recv_ops = true;
                    batch.recv_initial_metadata = true;
                }
                OP_RECV_MESSAGE => {
                    batch.has_recv_ops = true;
                    batch.recv_message = true;
                }
                OP_RECV_STATUS_ON_CLIENT => {
                    batch.has_recv_ops = true;
                    batch.recv_status = true;
                }
                _ => {
                    return Err(crate::error::invalid_argument("Unrecognized key in batch"));
                }
            }
        }

        Ok(batch)
    }

    /// Build a PHP result object from a completed unary call.
    fn build_unary_result(
        &self,
        recv_initial_metadata: bool,
        recv_message: bool,
        recv_status: bool,
        result: CallResult,
    ) -> PhpResult<ZBox<ZendObject>> {
        let (initial_metadata, body, trailing_metadata, status_code, status_message) = result;

        let mut result_obj = ZendObject::new_stdclass();

        if recv_initial_metadata {
            if let Some(ref md) = initial_metadata {
                result_obj
                    .set_property("metadata", metadata_to_php(md).map_err(PhpException::from)?)
                    .map_err(|e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set metadata: {e}"))
                    })?;
            } else {
                result_obj
                    .set_property("metadata", ZendHashTable::new())
                    .map_err(|e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set metadata: {e}"))
                    })?;
            }
        }

        if recv_message {
            if let Some(bytes) = body {
                let bin: ext_php_rs::binary::Binary<u8> = Vec::from(bytes).into();
                result_obj.set_property("message", bin).map_err(
                    |e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set message: {e}"))
                    },
                )?;
            } else {
                let mut null_zval = Zval::new();
                null_zval.set_null();
                result_obj.set_property("message", null_zval).map_err(
                    |e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set message: {e}"))
                    },
                )?;
            }
        }

        if recv_status {
            // Property order matches ext-grpc: metadata, code, details.
            let mut status_obj = ZendObject::new_stdclass();
            if let Some(ref md) = trailing_metadata {
                status_obj
                    .set_property("metadata", metadata_to_php(md).map_err(PhpException::from)?)
                    .map_err(|e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set status metadata: {e}"))
                    })?;
            } else {
                status_obj
                    .set_property("metadata", ZendHashTable::new())
                    .map_err(|e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set status metadata: {e}"))
                    })?;
            }
            status_obj
                .set_property("code", status_code as i64)
                .map_err(|e: ext_php_rs::error::Error| {
                    PhpException::default(format!("set status code: {e}"))
                })?;
            status_obj.set_property("details", status_message).map_err(
                |e: ext_php_rs::error::Error| {
                    PhpException::default(format!("set status details: {e}"))
                },
            )?;

            result_obj.set_property("status", status_obj).map_err(
                |e: ext_php_rs::error::Error| PhpException::default(format!("set status: {e}")),
            )?;
        }

        Ok(result_obj)
    }

    /// Build a tonic request with metadata and deadline applied.
    fn build_request(
        send_metadata: &[(String, String)],
        plugin_metadata: &[(String, String)],
        message: Bytes,
        deadline_usec: i64,
    ) -> tonic::Request<Bytes> {
        let mut request = tonic::Request::new(message);

        let req_metadata = request.metadata_mut();
        for (key, value) in send_metadata {
            if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                && let Ok(val) = value.parse()
            {
                req_metadata.insert(name, val);
            }
        }
        for (key, value) in plugin_metadata {
            if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                && let Ok(val) = value.parse()
            {
                req_metadata.insert(name, val);
            }
        }

        if deadline_usec < i64::MAX && deadline_usec > 0 {
            // See run_unary_call: expired deadlines still set a minimal
            // timeout so the call fails as DEADLINE_EXCEEDED like ext-grpc.
            let timeout_usec = deadline_usec.saturating_sub(now_usec()).max(1);
            request.set_timeout(std::time::Duration::from_micros(timeout_usec as u64));
        }

        request
    }

    /// Stream state for a call whose deadline had already passed at dispatch:
    /// no task is spawned and nothing is sent; reads see end-of-stream and
    /// the status reports DEADLINE_EXCEEDED, matching ext-grpc.
    fn expired_stream_state() -> ActiveStream {
        let (_tx, msg_rx) =
            tokio::sync::mpsc::channel::<Result<Option<Bytes>, tonic::Status>>(1);
        ActiveStream {
            msg_tx: None,
            msg_rx,
            initial_metadata: None,
            meta_rx: None,
            cached_trailers: Some(StreamTrailers {
                code: 4, // DEADLINE_EXCEEDED
                message: "Deadline Exceeded".to_string(),
                metadata: tonic::metadata::MetadataMap::default(),
            }),
            trailers_rx: None,
        }
    }

    /// Initiate a server streaming call. Spawns a tokio task that drives
    /// the stream and sends messages/trailers back via channels.
    fn start_server_stream(
        &mut self,
        send_metadata: Vec<(String, String)>,
        plugin_metadata: Vec<(String, String)>,
        send_message: Option<Bytes>,
    ) -> PhpResult<()> {
        let rt = get_runtime().map_err(PhpException::from)?;

        if deadline_already_expired(self.deadline_usec) {
            self.stream_state = Some(Self::expired_stream_state());
            return Ok(());
        }

        let channel = self.channel.clone();
        let method = self.method.clone();
        let deadline_usec = self.deadline_usec;
        let cancel_token = self.cancel_token.clone();
        let max_decoding = self.max_decoding_message_size;
        let max_encoding = self.max_encoding_message_size;
        let shared_state = self.channel_shared.clone();

        let path = PathAndQuery::try_from(method.as_str()).map_err(|e| {
            PhpException::from(GrpcError::InvalidArg(format!("invalid method path: {e}")))
        })?;

        let request = Self::build_request(
            &send_metadata,
            &plugin_metadata,
            send_message.unwrap_or_default(),
            deadline_usec,
        );

        // Channels: messages (bounded, see STREAM_MSG_BUFFER), initial metadata, trailers
        let (msg_tx, msg_rx) =
            tokio::sync::mpsc::channel::<Result<Option<Bytes>, tonic::Status>>(STREAM_MSG_BUFFER);
        let (meta_tx, meta_rx) = tokio::sync::oneshot::channel::<tonic::metadata::MetadataMap>();
        let (trailers_tx, trailers_rx) = tokio::sync::oneshot::channel::<StreamTrailers>();

        // Spawn the stream-driving task
        rt.spawn(async move {
            let mut grpc_client = tonic::client::Grpc::new(channel);
            if let Some(limit) = max_decoding {
                grpc_client = grpc_client.max_decoding_message_size(limit);
            }
            if let Some(limit) = max_encoding {
                grpc_client = grpc_client.max_encoding_message_size(limit);
            }
            if let Err(e) = grpc_client.ready().await {
                let status = tonic::Status::from_error(Box::new(e));
                let _ = meta_tx.send(tonic::metadata::MetadataMap::default());
                let _ = msg_tx.send(Err(status)).await;
                return;
            }

            let response = grpc_client
                .server_streaming(request, path, RawBytesCodec)
                .await;

            if let Some(ref sh) = shared_state {
                let next = match &response {
                    Err(status) if status.code() == tonic::Code::Unavailable => {
                        crate::channel::CHANNEL_TRANSIENT_FAILURE
                    }
                    _ => crate::channel::CHANNEL_READY,
                };
                *sh.state.lock() = next;
            }

            match response {
                Ok(resp) => {
                    let initial_md = resp.metadata().clone();
                    let _ = meta_tx.send(initial_md);
                    let mut body_stream = resp.into_inner();

                    loop {
                        tokio::select! {
                            msg = body_stream.message() => {
                                match msg {
                                    Ok(Some(bytes)) => {
                                        if msg_tx.send(Ok(Some(bytes))).await.is_err() {
                                            return;
                                        }
                                    }
                                    Ok(None) => {
                                        // End of stream — send sentinel then trailers
                                        let _ = msg_tx.send(Ok(None)).await;
                                        let trailers_md = body_stream
                                            .trailers()
                                            .await
                                            .unwrap_or_default()
                                            .unwrap_or_default();
                                        let _ = trailers_tx.send(StreamTrailers {
                                            code: 0,
                                            message: String::new(),
                                            metadata: trailers_md,
                                        });
                                        return;
                                    }
                                    Err(status) => {
                                        let (code, message) =
                                            status_to_php(&status, deadline_usec);
                                        let md = status_metadata_for_php(&status);
                                        let _ = msg_tx.send(Ok(None)).await;
                                        let _ = trailers_tx.send(StreamTrailers {
                                            code,
                                            message,
                                            metadata: md,
                                        });
                                        return;
                                    }
                                }
                            }
                            () = cancel_token.cancelled() => {
                                let _ = msg_tx.send(Ok(None)).await;
                                let _ = trailers_tx.send(StreamTrailers {
                                    code: 1, // CANCELLED
                                    message: "Call cancelled".into(),
                                    metadata: tonic::metadata::MetadataMap::default(),
                                });
                                return;
                            }
                        }
                    }
                }
                Err(status) => {
                    // Connection-level or early error — no stream opened
                    let _ = meta_tx.send(tonic::metadata::MetadataMap::default());
                    let (code, message) = status_to_php(&status, deadline_usec);
                    let md = status_metadata_for_php(&status);
                    let _ = msg_tx.send(Ok(None)).await;
                    let _ = trailers_tx.send(StreamTrailers {
                        code,
                        message,
                        metadata: md,
                    });
                }
            }
        });

        // Initial metadata stays deferred: build_stream_result resolves the
        // receiver on the first RECV_INITIAL_METADATA. Blocking here would
        // serialize eagerly-dispatched calls (issue #18) — headers only
        // arrive once the server responds.
        self.stream_state = Some(ActiveStream {
            msg_tx: None,
            msg_rx,
            initial_metadata: None,
            meta_rx: Some(meta_rx),
            cached_trailers: None,
            trailers_rx: Some(trailers_rx),
        });

        Ok(())
    }

    /// Initiate a bidi streaming call (used for both client streaming and bidi).
    /// Spawns a tokio task with an mpsc channel for the request stream.
    fn start_bidi_stream(
        &mut self,
        send_metadata: Vec<(String, String)>,
        plugin_metadata: Vec<(String, String)>,
    ) -> PhpResult<()> {
        let rt = get_runtime().map_err(PhpException::from)?;

        if deadline_already_expired(self.deadline_usec) {
            self.stream_state = Some(Self::expired_stream_state());
            return Ok(());
        }

        let channel = self.channel.clone();
        let method = self.method.clone();
        let deadline_usec = self.deadline_usec;
        let cancel_token = self.cancel_token.clone();
        let max_decoding = self.max_decoding_message_size;
        let max_encoding = self.max_encoding_message_size;
        let shared_state = self.channel_shared.clone();

        let path = PathAndQuery::try_from(method.as_str()).map_err(|e| {
            PhpException::from(GrpcError::InvalidArg(format!("invalid method path: {e}")))
        })?;

        // Request stream: PHP sends messages via req_tx, tonic reads from req_rx
        let (req_tx, req_rx) = tokio::sync::mpsc::channel::<Bytes>(4);

        // Build a request wrapping the stream, with metadata and deadline
        let mut request = tonic::Request::new(ReceiverStream::new(req_rx));
        let req_metadata = request.metadata_mut();
        for (key, value) in &send_metadata {
            if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                && let Ok(val) = value.parse()
            {
                req_metadata.insert(name, val);
            }
        }
        for (key, value) in &plugin_metadata {
            if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                && let Ok(val) = value.parse()
            {
                req_metadata.insert(name, val);
            }
        }
        if deadline_usec < i64::MAX && deadline_usec > 0 {
            // See run_unary_call: expired deadlines still set a minimal
            // timeout so the call fails as DEADLINE_EXCEEDED like ext-grpc.
            let timeout_usec = deadline_usec.saturating_sub(now_usec()).max(1);
            request.set_timeout(std::time::Duration::from_micros(timeout_usec as u64));
        }

        // Response channels
        let (msg_tx, msg_rx) =
            tokio::sync::mpsc::channel::<Result<Option<Bytes>, tonic::Status>>(STREAM_MSG_BUFFER);
        let (meta_tx, meta_rx) = tokio::sync::oneshot::channel::<tonic::metadata::MetadataMap>();
        let (trailers_tx, trailers_rx) = tokio::sync::oneshot::channel::<StreamTrailers>();

        rt.spawn(async move {
            let mut grpc_client = tonic::client::Grpc::new(channel);
            if let Some(limit) = max_decoding {
                grpc_client = grpc_client.max_decoding_message_size(limit);
            }
            if let Some(limit) = max_encoding {
                grpc_client = grpc_client.max_encoding_message_size(limit);
            }
            if let Err(e) = grpc_client.ready().await {
                let status = tonic::Status::from_error(Box::new(e));
                let _ = meta_tx.send(tonic::metadata::MetadataMap::default());
                let _ = msg_tx.send(Err(status)).await;
                return;
            }

            let response = grpc_client.streaming(request, path, RawBytesCodec).await;

            if let Some(ref sh) = shared_state {
                let next = match &response {
                    Err(status) if status.code() == tonic::Code::Unavailable => {
                        crate::channel::CHANNEL_TRANSIENT_FAILURE
                    }
                    _ => crate::channel::CHANNEL_READY,
                };
                *sh.state.lock() = next;
            }

            match response {
                Ok(resp) => {
                    let initial_md = resp.metadata().clone();
                    let _ = meta_tx.send(initial_md);
                    let mut body_stream = resp.into_inner();

                    loop {
                        tokio::select! {
                            msg = body_stream.message() => {
                                match msg {
                                    Ok(Some(bytes)) => {
                                        if msg_tx.send(Ok(Some(bytes))).await.is_err() {
                                            return;
                                        }
                                    }
                                    Ok(None) => {
                                        let _ = msg_tx.send(Ok(None)).await;
                                        let trailers_md = body_stream
                                            .trailers()
                                            .await
                                            .unwrap_or_default()
                                            .unwrap_or_default();
                                        let _ = trailers_tx.send(StreamTrailers {
                                            code: 0,
                                            message: String::new(),
                                            metadata: trailers_md,
                                        });
                                        return;
                                    }
                                    Err(status) => {
                                        let (code, message) =
                                            status_to_php(&status, deadline_usec);
                                        let md = status_metadata_for_php(&status);
                                        let _ = msg_tx.send(Ok(None)).await;
                                        let _ = trailers_tx.send(StreamTrailers {
                                            code,
                                            message,
                                            metadata: md,
                                        });
                                        return;
                                    }
                                }
                            }
                            () = cancel_token.cancelled() => {
                                let _ = msg_tx.send(Ok(None)).await;
                                let _ = trailers_tx.send(StreamTrailers {
                                    code: 1,
                                    message: "Call cancelled".into(),
                                    metadata: tonic::metadata::MetadataMap::default(),
                                });
                                return;
                            }
                        }
                    }
                }
                Err(status) => {
                    let _ = meta_tx.send(tonic::metadata::MetadataMap::default());
                    let (code, message) = status_to_php(&status, deadline_usec);
                    let md = status_metadata_for_php(&status);
                    let _ = msg_tx.send(Ok(None)).await;
                    let _ = trailers_tx.send(StreamTrailers {
                        code,
                        message,
                        metadata: md,
                    });
                }
            }
        });

        self.stream_state = Some(ActiveStream {
            msg_tx: Some(req_tx),
            msg_rx,
            initial_metadata: None,
            meta_rx: Some(meta_rx),
            cached_trailers: None,
            trailers_rx: Some(trailers_rx),
        });

        Ok(())
    }

    /// Send a message through the active bidi stream's request channel.
    fn send_stream_message(&mut self, message: Bytes) -> PhpResult<()> {
        let rt = get_runtime().map_err(PhpException::from)?;
        let state = self
            .stream_state
            .as_ref()
            .ok_or_else(|| PhpException::default("no active stream".into()))?;

        let tx = state
            .msg_tx
            .as_ref()
            .ok_or_else(|| PhpException::default("stream does not support sending".into()))?;

        rt.block_on(async { tx.send(message).await })
            .map_err(|_| PhpException::default("stream send channel closed".into()))?;

        Ok(())
    }

    /// Close the send side of a bidi stream (drops the sender).
    fn close_stream_send(&mut self) {
        if let Some(ref mut state) = self.stream_state {
            state.msg_tx.take(); // Drop sender → closes the request stream
        }
    }

    /// Build a PHP result object from the active stream state.
    fn build_stream_result(&mut self, batch: &BatchOps) -> PhpResult<ZBox<ZendObject>> {
        let rt = get_runtime().map_err(PhpException::from)?;
        let deadline_usec = self.deadline_usec;
        let state = self
            .stream_state
            .as_mut()
            .ok_or_else(|| PhpException::default("no active stream".into()))?;

        let mut result_obj = ZendObject::new_stdclass();

        // RECV_INITIAL_METADATA — resolve deferred or take stored metadata
        if batch.recv_initial_metadata {
            // If metadata hasn't arrived yet (bidi/client streaming), await it now
            if state.initial_metadata.is_none()
                && let Some(rx) = state.meta_rx.take()
            {
                state.initial_metadata = rt.block_on(rx).ok();
            }
            if let Some(ref md) = state.initial_metadata.take() {
                result_obj
                    .set_property("metadata", metadata_to_php(md).map_err(PhpException::from)?)
                    .map_err(|e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set metadata: {e}"))
                    })?;
            } else {
                result_obj
                    .set_property("metadata", ZendHashTable::new())
                    .map_err(|e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set metadata: {e}"))
                    })?;
            }
        }

        // RECV_MESSAGE — read next message from the stream
        if batch.recv_message {
            if state.cached_trailers.is_some() {
                // Stream already ended — return null
                let mut null_zval = Zval::new();
                null_zval.set_null();
                result_obj.set_property("message", null_zval).map_err(
                    |e: ext_php_rs::error::Error| {
                        PhpException::default(format!("set message: {e}"))
                    },
                )?;
            } else {
                // Fast path: a buffered message skips block_on (and its
                // runtime-context entry + park) entirely.
                let msg: Option<Result<Option<Bytes>, tonic::Status>> =
                    match state.msg_rx.try_recv() {
                        Ok(m) => Some(m),
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                            rt.block_on(async { state.msg_rx.recv().await })
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => None,
                    };
                match msg {
                    Some(Ok(Some(bytes))) => {
                        let bin: ext_php_rs::binary::Binary<u8> = Vec::from(bytes).into();
                        result_obj.set_property("message", bin).map_err(
                            |e: ext_php_rs::error::Error| {
                                PhpException::default(format!("set message: {e}"))
                            },
                        )?;
                    }
                    Some(Ok(None)) | None => {
                        // End of stream — return null message
                        let mut null_zval = Zval::new();
                        null_zval.set_null();
                        result_obj.set_property("message", null_zval).map_err(
                            |e: ext_php_rs::error::Error| {
                                PhpException::default(format!("set message: {e}"))
                            },
                        )?;
                    }
                    Some(Err(status)) => {
                        // Mid-stream error — cache as trailers, return null message
                        let (code, message) = status_to_php(&status, deadline_usec);
                        state.cached_trailers = Some(StreamTrailers {
                            code,
                            message,
                            metadata: status_metadata_for_php(&status),
                        });
                        let mut null_zval = Zval::new();
                        null_zval.set_null();
                        result_obj.set_property("message", null_zval).map_err(
                            |e: ext_php_rs::error::Error| {
                                PhpException::default(format!("set message: {e}"))
                            },
                        )?;
                    }
                }
            }
        }

        // RECV_STATUS_ON_CLIENT — return final status
        if batch.recv_status {
            let trailers = if let Some(cached) = state.cached_trailers.take() {
                cached
            } else if let Some(rx) = state.trailers_rx.take() {
                rt.block_on(rx).unwrap_or(StreamTrailers {
                    code: 2, // UNKNOWN
                    message: "stream task terminated unexpectedly".into(),
                    metadata: tonic::metadata::MetadataMap::default(),
                })
            } else {
                StreamTrailers {
                    code: 2,
                    message: "stream already consumed".into(),
                    metadata: tonic::metadata::MetadataMap::default(),
                }
            };

            // Property order matches ext-grpc: metadata, code, details.
            let mut status_obj = ZendObject::new_stdclass();
            status_obj
                .set_property(
                    "metadata",
                    metadata_to_php(&trailers.metadata).map_err(PhpException::from)?,
                )
                .map_err(|e: ext_php_rs::error::Error| {
                    PhpException::default(format!("set status metadata: {e}"))
                })?;
            status_obj
                .set_property("code", trailers.code as i64)
                .map_err(|e: ext_php_rs::error::Error| {
                    PhpException::default(format!("set status code: {e}"))
                })?;
            status_obj
                .set_property("details", trailers.message)
                .map_err(|e: ext_php_rs::error::Error| {
                    PhpException::default(format!("set status details: {e}"))
                })?;

            result_obj.set_property("status", status_obj).map_err(
                |e: ext_php_rs::error::Error| PhpException::default(format!("set status: {e}")),
            )?;
        }

        Ok(result_obj)
    }
}

#[cfg(test)]
mod tests {
    use super::{collect_metadata, status_metadata_for_php};
    use tonic::metadata::{AsciiMetadataValue, BinaryMetadataValue, MetadataKey, MetadataMap};

    /// ASCII metadata is surfaced as raw bytes (the on-wire form), grouped by
    /// key with insertion order preserved per key.
    #[test]
    fn ascii_metadata_is_collected() {
        let mut map = MetadataMap::new();
        map.insert("foo", AsciiMetadataValue::from_static("bar"));
        map.append("foo", AsciiMetadataValue::from_static("baz"));
        map.insert("other", AsciiMetadataValue::from_static("qux"));

        let collected = collect_metadata(&map);

        let foo = collected.iter().find(|(k, _)| k == "foo");
        assert!(foo.is_some(), "foo key present");
        if let Some((_, values)) = foo {
            assert_eq!(values, &vec![b"bar".to_vec(), b"baz".to_vec()]);
        }

        let other = collected.iter().find(|(k, _)| k == "other");
        assert!(other.is_some(), "other key present");
        if let Some((_, values)) = other {
            assert_eq!(values, &vec![b"qux".to_vec()]);
        }
    }

    /// Binary (`-bin`) metadata is surfaced as the *decoded* raw payload,
    /// matching ext-grpc. This is the case the previous implementation silently
    /// dropped, breaking `grpc-status-details-bin` and other rich-status
    /// propagation.
    #[test]
    fn binary_metadata_is_surfaced_as_decoded_bytes() {
        // Arbitrary binary payload, includes a null byte and a non-UTF-8
        // sequence to demonstrate that PHP-side surfacing must be byte-safe and
        // must NOT be base64-encoded.
        let payload: &[u8] = b"\x00\x01\x02\xff\xfehello\x00world";

        let mut map = MetadataMap::new();
        let key: Result<MetadataKey<tonic::metadata::Binary>, _> = "x-custom-bin".parse();
        assert!(key.is_ok(), "binary metadata key must parse");
        if let Ok(k) = key {
            map.insert_bin(k, BinaryMetadataValue::from_bytes(payload));
        }

        let collected = collect_metadata(&map);

        let entry = collected.iter().find(|(k, _)| k == "x-custom-bin");
        assert!(entry.is_some(), "binary key present in collected output");
        if let Some((_, values)) = entry {
            assert_eq!(values.len(), 1, "single value expected");
            if let Some(first) = values.first() {
                assert_eq!(
                    first.as_slice(),
                    payload,
                    "binary value must be the decoded raw bytes, not the base64 wire form"
                );
            }
        }
    }

    /// Mixed map: both ASCII and binary entries must coexist.
    #[test]
    fn mixed_ascii_and_binary_are_both_present() {
        let mut map = MetadataMap::new();
        map.insert(
            "content-type",
            AsciiMetadataValue::from_static("application/grpc"),
        );
        let key: Result<MetadataKey<tonic::metadata::Binary>, _> =
            "grpc-status-details-bin".parse();
        assert!(key.is_ok());
        // 4-byte marker that resembles a small protobuf payload.
        let payload: &[u8] = b"\x08\x05\x12\x00";
        if let Ok(k) = key {
            map.insert_bin(k, BinaryMetadataValue::from_bytes(payload));
        }

        let collected = collect_metadata(&map);

        assert_eq!(collected.len(), 2);
        assert!(
            collected
                .iter()
                .any(|(k, v)| k == "content-type" && v == &vec![b"application/grpc".to_vec()])
        );
        assert!(
            collected
                .iter()
                .any(|(k, v)| k == "grpc-status-details-bin" && v == &vec![payload.to_vec()])
        );
    }

    /// `Status::from_header_map` removes this reserved header from metadata
    /// and stores its decoded payload in `Status::details()`. Re-inject it for
    /// PHP so the observable trailing metadata matches ext-grpc.
    #[test]
    fn status_details_bin_is_reinjected_after_tonic_parses_headers() {
        let payload: &[u8] = b"\x08\x0d\x12\x0brich status";
        // This is the exact trailer representation emitted by gRPC on the
        // wire. `CA0SC3JpY2ggc3RhdHVz` is the base64 encoding of `payload`.
        let mut header_map = http::HeaderMap::new();
        header_map.insert("grpc-status", http::HeaderValue::from_static("13"));
        header_map.insert(
            "grpc-message",
            http::HeaderValue::from_static("test%20error"),
        );
        header_map.insert(
            "grpc-status-details-bin",
            http::HeaderValue::from_static("CA0SC3JpY2ggc3RhdHVz"),
        );

        assert!(
            header_map.contains_key("grpc-status-details-bin"),
            "the wire header uses tonic's reserved key"
        );
        let status = tonic::Status::from_header_map(&header_map);
        assert!(status.is_some(), "tonic must parse a status header map");
        let Some(status) = status else {
            return;
        };
        assert_eq!(status.details(), payload);
        assert!(
            status
                .metadata()
                .get_bin("grpc-status-details-bin")
                .is_none(),
            "tonic removes its reserved header from Status metadata"
        );

        let collected = collect_metadata(&status_metadata_for_php(&status));
        assert!(
            collected.iter().any(|(key, values)| {
                key == "grpc-status-details-bin" && values == &vec![payload.to_vec()]
            }),
            "PHP trailing metadata receives the decoded rich-status payload"
        );
    }

    /// Sanity check: an empty `MetadataMap` produces no entries.
    #[test]
    fn empty_metadata_produces_empty_entries() {
        let map = MetadataMap::new();
        let collected = collect_metadata(&map);
        assert!(collected.is_empty());
    }
}
