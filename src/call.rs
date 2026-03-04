use std::sync::Arc;

use bytes::Bytes;
use ext_php_rs::boxed::ZBox;
use ext_php_rs::prelude::*;
use ext_php_rs::types::{ArrayKey, ZendCallable, ZendHashTable, ZendObject, Zval};
use http::uri::PathAndQuery;
use parking_lot::Mutex;
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

/// Parsed operations from the PHP batch array.
struct BatchOps {
    send_metadata: Vec<(String, String)>,
    send_message: Option<Bytes>,
    recv_initial_metadata: bool,
    recv_message: bool,
    recv_status: bool,
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

    // The callback should return an array of metadata key-value pairs
    let mut metadata = Vec::new();
    if let Some(ht) = result.array() {
        for (key, val) in ht.iter() {
            if let Some(k) = array_key_to_string(&key)
                && let Some(v) = val.string() {
                    metadata.push((k, v));
                }
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

/// Build a metadata array for PHP from a tonic MetadataMap.
fn metadata_to_php(
    map: &tonic::metadata::MetadataMap,
) -> Result<ZBox<ZendHashTable>, GrpcError> {
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
                            new_arr
                                .push(v.shallow_clone())
                                .map_err(|e| GrpcError::InvalidArg(format!("metadata build: {e}")))?;
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

        let target = channel
            .get_target_uri()
            .unwrap_or_default();

        let call_plugin = channel.get_call_plugin();

        Ok(Self {
            channel: tonic_channel,
            method,
            target,
            deadline_usec: deadline.get_usec(),
            host_override,
            call_plugin,
            cancel_token: CancellationToken::new(),
        })
    }

    /// Starts a batch of operations.
    #[php(name = "startBatch")]
    pub fn start_batch(&mut self, ops: &ZendHashTable) -> PhpResult<ZBox<ZendObject>> {
        if self.cancel_token.is_cancelled() {
            return Err(PhpException::from(GrpcError::Status {
                code: 1, // CANCELLED
                message: "Call has been cancelled".into(),
            }));
        }

        // Step 1: Parse the ops array on the PHP thread
        let batch = self.parse_ops(ops)?;

        // Step 2: Invoke call credentials plugin on PHP thread if set
        let mut plugin_metadata = Vec::new();
        if let Some(ref plugin) = self.call_plugin {
            plugin_metadata = invoke_call_plugin(plugin, &self.method)
                .map_err(PhpException::from)?;
        }

        // Step 3: Build the gRPC request and execute in tokio
        let rt = get_runtime().map_err(PhpException::from)?;

        // Collect all Send-safe data before entering async block
        let channel = self.channel.clone();
        let method = self.method.clone();
        let send_metadata = batch.send_metadata;
        let send_message = batch.send_message;
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
                    && let Ok(val) = value.parse() {
                        req_metadata.insert(name, val);
                    }
            }
            for (key, value) in &plugin_metadata {
                if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                    && let Ok(val) = value.parse() {
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

        let (initial_metadata, body, trailing_metadata, status_code, status_message) =
            result.map_err(PhpException::from)?;

        // Step 4: Build the result stdClass
        let mut result_obj = ZendObject::new_stdclass();

        if batch.recv_initial_metadata {
            if let Some(ref md) = initial_metadata {
                result_obj
                    .set_property("metadata", metadata_to_php(md).map_err(PhpException::from)?)
                    .map_err(|e: ext_php_rs::error::Error| PhpException::default(format!("set metadata: {e}")))?;
            } else {
                result_obj
                    .set_property("metadata", ZendHashTable::new())
                    .map_err(|e: ext_php_rs::error::Error| PhpException::default(format!("set metadata: {e}")))?;
            }
        }

        if batch.recv_message {
            if let Some(bytes) = body {
                // Binary<u8> creates a PHP string from raw bytes via set_binary().
                // Vec::from(Bytes) is zero-copy when Bytes has sole ownership.
                let bin: ext_php_rs::binary::Binary<u8> = Vec::from(bytes).into();
                result_obj
                    .set_property("message", bin)
                    .map_err(|e: ext_php_rs::error::Error| PhpException::default(format!("set message: {e}")))?;
            } else {
                let mut null_zval = Zval::new();
                null_zval.set_null();
                result_obj
                    .set_property("message", null_zval)
                    .map_err(|e: ext_php_rs::error::Error| PhpException::default(format!("set message: {e}")))?;
            }
        }

        if batch.recv_status {
            let mut status_obj = ZendObject::new_stdclass();
            status_obj
                .set_property("code", status_code as i64)
                .map_err(|e: ext_php_rs::error::Error| PhpException::default(format!("set status code: {e}")))?;
            status_obj
                .set_property("details", status_message)
                .map_err(|e: ext_php_rs::error::Error| PhpException::default(format!("set status details: {e}")))?;

            if let Some(ref md) = trailing_metadata {
                status_obj
                    .set_property("metadata", metadata_to_php(md).map_err(PhpException::from)?)
                    .map_err(|e: ext_php_rs::error::Error| PhpException::default(format!("set status metadata: {e}")))?;
            } else {
                status_obj
                    .set_property("metadata", ZendHashTable::new())
                    .map_err(|e: ext_php_rs::error::Error| PhpException::default(format!("set status metadata: {e}")))?;
            }

            result_obj
                .set_property("status", status_obj)
                .map_err(|e: ext_php_rs::error::Error| PhpException::default(format!("set status: {e}")))?;
        }

        Ok(result_obj)
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
    /// Parse the ops array into a structured BatchOps.
    fn parse_ops(&self, ops: &ZendHashTable) -> PhpResult<BatchOps> {
        let mut batch = BatchOps {
            send_metadata: Vec::new(),
            send_message: None,
            recv_initial_metadata: false,
            recv_message: false,
            recv_status: false,
        };

        for (key, val) in ops.iter() {
            let op_code = array_key_to_long(&key).map_err(PhpException::from)?;

            match op_code {
                OP_SEND_INITIAL_METADATA => {
                    if let Some(ht) = val.array() {
                        batch.send_metadata = parse_metadata(ht);
                    }
                }
                OP_SEND_MESSAGE => {
                    // The C extension accepts either a string directly or
                    // an array with 'message' key
                    if let Some(s) = val.string() {
                        batch.send_message = Some(Bytes::from(s.into_bytes()));
                    } else if let Some(ht) = val.array()
                        && let Some(msg_zval) = ht.get("message")
                            && let Some(s) = msg_zval.string() {
                                batch.send_message = Some(Bytes::from(s.into_bytes()));
                            }
                }
                OP_SEND_CLOSE_FROM_CLIENT => {
                    // Acknowledged — close is implicit in unary
                }
                OP_RECV_INITIAL_METADATA => {
                    batch.recv_initial_metadata = true;
                }
                OP_RECV_MESSAGE => {
                    batch.recv_message = true;
                }
                OP_RECV_STATUS_ON_CLIENT => {
                    batch.recv_status = true;
                }
                _ => {
                    // Silently ignore unknown ops (forward compat)
                }
            }
        }

        Ok(batch)
    }
}
