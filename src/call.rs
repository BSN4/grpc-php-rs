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
    }
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

    let result = callable
        .try_call(vec![&service_url.to_string()])
        .map_err(|e| GrpcError::CallbackFailed(format!("{e:?}")))?;

    // The callback returns metadata as key => value or key => [values]
    // (Google Cloud SDK returns e.g. ['authorization' => ['Bearer xxx']])
    if let Some(ht) = result.array() {
        Ok(parse_metadata(ht))
    } else {
        Ok(Vec::new())
    }
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

/// Build a metadata array for PHP from a tonic MetadataMap.
fn metadata_to_php(map: &tonic::metadata::MetadataMap) -> Result<ZBox<ZendHashTable>, GrpcError> {
    let mut ht = ZendHashTable::new();
    for key_and_value in map.iter() {
        if let tonic::metadata::KeyAndValueRef::Ascii(key, value) = key_and_value {
            let key_str = key.as_str();
            if let Ok(val_str) = value.to_str() {
                // Each metadata key maps to an array of values
                let existing = ht.get(key_str);
                if let Some(existing_zval) = existing {
                    if let Some(arr) = existing_zval.array() {
                        let mut new_arr = ZendHashTable::new();
                        for (_k, v) in arr.iter() {
                            new_arr.push(v.shallow_clone()).map_err(|e| {
                                GrpcError::InvalidArg(format!("metadata build: {e}"))
                            })?;
                        }
                        new_arr
                            .push(val_str.to_string())
                            .map_err(|e| GrpcError::InvalidArg(format!("metadata build: {e}")))?;
                        ht.insert(key_str, new_arr)
                            .map_err(|e| GrpcError::InvalidArg(format!("metadata build: {e}")))?;
                    }
                } else {
                    let mut arr = ZendHashTable::new();
                    arr.push(val_str.to_string())
                        .map_err(|e| GrpcError::InvalidArg(format!("metadata build: {e}")))?;
                    ht.insert(key_str, arr)
                        .map_err(|e| GrpcError::InvalidArg(format!("metadata build: {e}")))?;
                }
            }
        }
    }
    Ok(ht)
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
    host_override: Option<String>,
    call_plugin: Option<Arc<Mutex<Option<Zval>>>>,
    cancel_token: CancellationToken,
    /// Buffered send data from a send-only startBatch (split send/recv pattern).
    pending_metadata: Vec<(String, String)>,
    pending_message: Option<Bytes>,
    /// Whether OP_SEND_CLOSE_FROM_CLIENT has been seen.
    send_closed: bool,
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
            PhpException::from(GrpcError::InvalidArg("Channel has been closed".into()))
        })?;

        let target = channel.get_target_uri().unwrap_or_default();

        let call_plugin = channel.get_call_plugin();

        Ok(Self {
            channel: tonic_channel,
            method,
            target,
            deadline_usec: deadline.get_usec(),
            host_override,
            call_plugin,
            cancel_token: CancellationToken::new(),
            pending_metadata: Vec::new(),
            pending_message: None,
            send_closed: false,
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
        if self.cancel_token.is_cancelled() {
            return Err(PhpException::from(GrpcError::Status {
                code: 1, // CANCELLED
                message: "Call has been cancelled".into(),
            }));
        }

        let mut batch = self.parse_ops(ops)?;

        // Track SEND_CLOSE_FROM_CLIENT
        if batch.send_close {
            self.send_closed = true;
        }

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

    /// Returns the peer URI.
    #[php(name = "getPeer")]
    pub fn get_peer(&self) -> String {
        self.host_override
            .clone()
            .unwrap_or_else(|| self.target.clone())
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
                let now_usec = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0i64, |d| d.as_micros() as i64);
                let timeout_usec = deadline_usec.saturating_sub(now_usec);
                if timeout_usec > 0 {
                    request.set_timeout(std::time::Duration::from_micros(timeout_usec as u64));
                }
            }

            // Make the unary call using the raw codec, with cancellation support
            let mut grpc_client = tonic::client::Grpc::new(channel);
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
                            let code = status.code() as i32;
                            let msg = status.message().to_string();
                            let md = status.metadata().clone();
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

        result.map_err(PhpException::from)
    }

    /// Parse the ops array into a structured BatchOps.
    fn parse_ops(&self, ops: &ZendHashTable) -> PhpResult<BatchOps> {
        let mut batch = BatchOps {
            send_metadata: Vec::new(),
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
                    if let Some(ht) = val.array() {
                        batch.send_metadata = parse_metadata(ht);
                    }
                }
                OP_SEND_MESSAGE => {
                    batch.has_send_ops = true;
                    // Protobuf messages are binary — use zend_str() (raw bytes)
                    // instead of string() which rejects non-UTF-8 data.
                    if let Some(zs) = val.zend_str() {
                        batch.send_message = Some(Bytes::copy_from_slice(zs.as_bytes()));
                    } else if let Some(ht) = val.array()
                        && let Some(msg_zval) = ht.get("message")
                        && let Some(zs) = msg_zval.zend_str()
                    {
                        batch.send_message = Some(Bytes::copy_from_slice(zs.as_bytes()));
                    }
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
                    // Silently ignore unknown ops (forward compat)
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
            let mut status_obj = ZendObject::new_stdclass();
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
            let now_usec = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0i64, |d| d.as_micros() as i64);
            let timeout_usec = deadline_usec.saturating_sub(now_usec);
            if timeout_usec > 0 {
                request.set_timeout(std::time::Duration::from_micros(timeout_usec as u64));
            }
        }

        request
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

        let channel = self.channel.clone();
        let method = self.method.clone();
        let deadline_usec = self.deadline_usec;
        let cancel_token = self.cancel_token.clone();

        let path = PathAndQuery::try_from(method.as_str()).map_err(|e| {
            PhpException::from(GrpcError::InvalidArg(format!("invalid method path: {e}")))
        })?;

        let request = Self::build_request(
            &send_metadata,
            &plugin_metadata,
            send_message.unwrap_or_default(),
            deadline_usec,
        );

        // Channels: messages (backpressure buffer=1), initial metadata, trailers
        let (msg_tx, msg_rx) =
            tokio::sync::mpsc::channel::<Result<Option<Bytes>, tonic::Status>>(1);
        let (meta_tx, meta_rx) = tokio::sync::oneshot::channel::<tonic::metadata::MetadataMap>();
        let (trailers_tx, trailers_rx) = tokio::sync::oneshot::channel::<StreamTrailers>();

        // Spawn the stream-driving task
        rt.spawn(async move {
            let mut grpc_client = tonic::client::Grpc::new(channel);
            if let Err(e) = grpc_client.ready().await {
                let status = tonic::Status::from_error(Box::new(e));
                let _ = meta_tx.send(tonic::metadata::MetadataMap::default());
                let _ = msg_tx.send(Err(status)).await;
                return;
            }

            let response = grpc_client
                .server_streaming(request, path, RawBytesCodec)
                .await;

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
                                        let code = status.code() as i32;
                                        let message = status.message().to_string();
                                        let md = status.metadata().clone();
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
                    let code = status.code() as i32;
                    let message = status.message().to_string();
                    let md = status.metadata().clone();
                    let _ = msg_tx.send(Ok(None)).await;
                    let _ = trailers_tx.send(StreamTrailers {
                        code,
                        message,
                        metadata: md,
                    });
                }
            }
        });

        // Synchronously wait for initial metadata (the stream task sends it
        // as soon as the server responds with headers).
        let initial_metadata = rt.block_on(meta_rx).ok();

        self.stream_state = Some(ActiveStream {
            msg_tx: None,
            msg_rx,
            initial_metadata,
            meta_rx: None,
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

        let channel = self.channel.clone();
        let method = self.method.clone();
        let deadline_usec = self.deadline_usec;
        let cancel_token = self.cancel_token.clone();

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
            let now_usec = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0i64, |d| d.as_micros() as i64);
            let timeout_usec = deadline_usec.saturating_sub(now_usec);
            if timeout_usec > 0 {
                request.set_timeout(std::time::Duration::from_micros(timeout_usec as u64));
            }
        }

        // Response channels
        let (msg_tx, msg_rx) =
            tokio::sync::mpsc::channel::<Result<Option<Bytes>, tonic::Status>>(1);
        let (meta_tx, meta_rx) = tokio::sync::oneshot::channel::<tonic::metadata::MetadataMap>();
        let (trailers_tx, trailers_rx) = tokio::sync::oneshot::channel::<StreamTrailers>();

        rt.spawn(async move {
            let mut grpc_client = tonic::client::Grpc::new(channel);
            if let Err(e) = grpc_client.ready().await {
                let status = tonic::Status::from_error(Box::new(e));
                let _ = meta_tx.send(tonic::metadata::MetadataMap::default());
                let _ = msg_tx.send(Err(status)).await;
                return;
            }

            let response = grpc_client.streaming(request, path, RawBytesCodec).await;

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
                                        let code = status.code() as i32;
                                        let message = status.message().to_string();
                                        let md = status.metadata().clone();
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
                    let code = status.code() as i32;
                    let message = status.message().to_string();
                    let md = status.metadata().clone();
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
                let msg: Option<Result<Option<Bytes>, tonic::Status>> =
                    rt.block_on(async { state.msg_rx.recv().await });
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
                        state.cached_trailers = Some(StreamTrailers {
                            code: status.code() as i32,
                            message: status.message().to_string(),
                            metadata: status.metadata().clone(),
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

            let mut status_obj = ZendObject::new_stdclass();
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
            status_obj
                .set_property(
                    "metadata",
                    metadata_to_php(&trailers.metadata).map_err(PhpException::from)?,
                )
                .map_err(|e: ext_php_rs::error::Error| {
                    PhpException::default(format!("set status metadata: {e}"))
                })?;

            result_obj.set_property("status", status_obj).map_err(
                |e: ext_php_rs::error::Error| PhpException::default(format!("set status: {e}")),
            )?;
        }

        Ok(result_obj)
    }
}
