use std::sync::Arc;
use std::time::Duration;

use ext_php_rs::prelude::*;
use ext_php_rs::types::{ZendHashTable, Zval};
use parking_lot::Mutex;
use tonic::transport::{Channel, Endpoint};

use crate::credentials::{CredentialsInner, GrpcChannelCredentials};
use crate::error::GrpcError;
use crate::timeval::GrpcTimeval;

/// Connectivity state: idle (initial state).
const CHANNEL_IDLE: i64 = 0;

struct ChannelInner {
    channel: Channel,
    target: String,
    state: Mutex<i64>,
    call_plugin: Option<Arc<Mutex<Option<Zval>>>>,
    max_decoding_message_size: Option<usize>,
    max_encoding_message_size: Option<usize>,
}

#[php_class]
#[php(name = "Grpc\\Channel")]
pub struct GrpcChannel {
    inner: Option<Arc<ChannelInner>>,
    /// Shared with calls created from this channel so startBatch can reject
    /// use-after-close like ext-grpc.
    closed_flag: Arc<std::sync::atomic::AtomicBool>,
}

#[php_impl]
impl GrpcChannel {
    /// Constructs a new Channel.
    ///
    /// PHP signature: __construct(string $target, array $args)
    pub fn __construct(target: String, args: &ZendHashTable) -> PhpResult<Self> {
        let mut tls_config = None;
        let mut call_plugin = None;

        // Validate channel args like ext-grpc: string keys, int/string values.
        // 'credentials' and 'force_new' are extracted before validation there.
        for (key, val) in args.iter() {
            let key_str = match &key {
                ext_php_rs::types::ArrayKey::Long(_) => {
                    return Err(crate::error::invalid_argument("args keys must be strings"));
                }
                other => other.to_string(),
            };
            if key_str == "credentials" || key_str == "force_new" {
                continue;
            }
            if val.long().is_none() && val.string().is_none() {
                return Err(crate::error::invalid_argument(
                    "args values must be int or string",
                ));
            }
        }

        // Extract credentials from args first — we need to know if TLS is
        // required before building the URI.
        if let Some(creds_zval) = args.get("credentials") {
            // If credentials is null, it means insecure (from createInsecure());
            // any other non-ChannelCredentials value is an error, never a
            // silent plaintext fallback.
            if !creds_zval.is_null() {
                let creds = creds_zval.extract::<&GrpcChannelCredentials>().ok_or_else(|| {
                    crate::error::invalid_argument(
                        "credentials must be a ChannelCredentials object",
                    )
                })?;
                match &creds.inner {
                    CredentialsInner::Ssl {
                        tls_config: tls_cfg,
                    } => {
                        tls_config = Some(tls_cfg.clone());
                    }
                    CredentialsInner::Composite {
                        tls_config: tls_cfg,
                        call_plugin: plugin,
                    } => {
                        tls_config = Some(tls_cfg.clone());
                        call_plugin = Some(Arc::clone(plugin));
                    }
                }
            }
        }

        // The C gRPC extension accepts bare "host:port" targets, but tonic needs
        // a URI with scheme. Use https:// for TLS credentials, http:// for insecure.
        let uri_target = if target.contains("://") {
            target.clone()
        } else if tls_config.is_some() {
            format!("https://{target}")
        } else {
            format!("http://{target}")
        };

        let mut endpoint = Endpoint::from_shared(uri_target)
            .map_err(|e| PhpException::from(GrpcError::InvalidUri(e.to_string())))?;

        // Extract keepalive settings
        if let Some(val) = args.get("grpc.keepalive_time_ms")
            && let Some(ms) = val.long()
            && ms > 0
        {
            endpoint = endpoint.http2_keep_alive_interval(Duration::from_millis(ms as u64));
        }

        if let Some(val) = args.get("grpc.keepalive_timeout_ms")
            && let Some(ms) = val.long()
            && ms > 0
        {
            endpoint = endpoint.keep_alive_timeout(Duration::from_millis(ms as u64));
        }

        // Extract SSL target name override
        if let Some(val) = args.get("grpc.ssl_target_name_override")
            && let Some(name) = val.string()
            && let Some(ref mut tls) = tls_config
        {
            *tls = tls.clone().domain_name(name);
        }

        // Extract user agent
        if let Some(val) = args.get("grpc.primary_user_agent")
            && let Some(ua) = val.string()
        {
            endpoint = endpoint
                .user_agent(ua)
                .map_err(|e| PhpException::from(GrpcError::InvalidArg(e.to_string())))?;
        }

        // Extract gRPC message size limits (maps to tonic's max_decoding/encoding_message_size).
        // The C extension uses grpc.max_receive_message_length / grpc.max_send_message_length;
        // tonic defaults to 4 MiB decoding / unlimited encoding. A value of -1 means unlimited.
        let max_decoding_message_size = args
            .get("grpc.max_receive_message_length")
            .and_then(|v| v.long())
            .and_then(|v| if v == -1 { Some(usize::MAX) } else if v > 0 { Some(v as usize) } else { None });

        let max_encoding_message_size = args
            .get("grpc.max_send_message_length")
            .and_then(|v| v.long())
            .and_then(|v| if v == -1 { Some(usize::MAX) } else if v > 0 { Some(v as usize) } else { None });

        // Apply TLS config.
        // Always call tls_config() when credentials were provided — tonic requires
        // explicit TLS config even for https:// URLs (it doesn't auto-enable).
        if let Some(tls) = tls_config {
            endpoint = endpoint
                .tls_config(tls)
                .map_err(|e| PhpException::from(GrpcError::Transport(e)))?;
        }

        // Enter the Tokio runtime context so hyper can find the reactor
        let rt = crate::runtime::get_runtime().map_err(PhpException::from)?;
        let _guard = rt.enter();

        // Use connect_lazy to avoid blocking in constructor
        let channel = endpoint.connect_lazy();

        // ChannelInner contains Option<Zval> (via call_plugin) which is !Send,
        // but Arc is needed for shared ownership. All access stays on the PHP thread.
        #[allow(clippy::arc_with_non_send_sync)]
        Ok(Self {
            inner: Some(Arc::new(ChannelInner {
                channel,
                target,
                state: Mutex::new(CHANNEL_IDLE),
                call_plugin,
                max_decoding_message_size,
                max_encoding_message_size,
            })),
            closed_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    /// Returns the target URI.
    #[php(name = "getTarget")]
    pub fn get_target(&self) -> PhpResult<String> {
        let inner = self.inner.as_ref().ok_or_else(|| {
            crate::error::runtime_exception("getTarget error.Channel is already closed.")
        })?;
        Ok(inner.target.clone())
    }

    /// Returns the connectivity state.
    #[php(name = "getConnectivityState")]
    pub fn get_connectivity_state(&self, try_to_connect: Option<&Zval>) -> PhpResult<i64> {
        // ext-grpc rejects non-bool arguments instead of coercing.
        if let Some(zv) = try_to_connect
            && zv.bool().is_none()
        {
            return Err(crate::error::invalid_argument(
                "getConnectivityState expects a bool",
            ));
        }
        let inner = self.inner.as_ref().ok_or_else(|| {
            crate::error::runtime_exception(
                "getConnectivityState error.Channel is already closed.",
            )
        })?;
        let state = inner.state.lock();
        Ok(*state)
    }

    /// Watches for a connectivity state change.
    ///
    /// Returns true if state changed before deadline, false on timeout.
    #[php(name = "watchConnectivityState")]
    pub fn watch_connectivity_state(
        &self,
        _last_state: i64,
        _deadline: &GrpcTimeval,
    ) -> PhpResult<bool> {
        let _inner = self.inner.as_ref().ok_or_else(|| {
            crate::error::runtime_exception("watchConnectivityState errorChannel is already closed.")
        })?;

        // Tonic doesn't expose connectivity state watching directly.
        // For Phase 1, we return true (state changed) immediately.
        // This matches the common usage pattern where callers just want to proceed.
        Ok(true)
    }

    /// Closes the channel.
    pub fn close(&mut self) {
        self.inner = None;
        self.closed_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

// Internal methods — NOT exposed to PHP
impl GrpcChannel {
    /// Returns the tonic channel (for internal use by Call).
    pub(crate) fn get_tonic_channel(&self) -> Option<Channel> {
        self.inner.as_ref().map(|i| i.channel.clone())
    }

    /// Returns the target URI string.
    pub(crate) fn get_target_uri(&self) -> Option<String> {
        self.inner.as_ref().map(|i| i.target.clone())
    }

    /// Shared closed-state flag, observed by calls created from this channel.
    pub(crate) fn get_closed_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.closed_flag)
    }

    /// Returns the call plugin if composite credentials were used.
    pub(crate) fn get_call_plugin(&self) -> Option<Arc<Mutex<Option<Zval>>>> {
        self.inner
            .as_ref()
            .and_then(|i| i.call_plugin.as_ref().map(Arc::clone))
    }

    /// Returns the configured max decoding (receive) message size, if any.
    pub(crate) fn get_max_decoding_message_size(&self) -> Option<usize> {
        self.inner.as_ref().and_then(|i| i.max_decoding_message_size)
    }

    /// Returns the configured max encoding (send) message size, if any.
    pub(crate) fn get_max_encoding_message_size(&self) -> Option<usize> {
        self.inner.as_ref().and_then(|i| i.max_encoding_message_size)
    }
}
