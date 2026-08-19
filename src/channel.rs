use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use ext_php_rs::prelude::*;
use ext_php_rs::types::{ZendHashTable, Zval};
use parking_lot::Mutex;
use tonic::transport::{Channel, Endpoint};

use crate::credentials::{CredentialsInner, GrpcChannelCredentials};
use crate::error::GrpcError;
use crate::timeval::GrpcTimeval;

/// Connectivity states (Grpc\CHANNEL_* constant values).
const CHANNEL_IDLE: i64 = 0;
pub(crate) const CHANNEL_CONNECTING: i64 = 1;
pub(crate) const CHANNEL_READY: i64 = 2;
pub(crate) const CHANNEL_TRANSIENT_FAILURE: i64 = 3;

/// Transport + connectivity state shared by every Grpc\Channel object (and
/// the calls created from them) that refers to the same underlying channel.
/// Mirrors ext-grpc's persistent channel list: same (target, args, TLS) key
/// reuses one connection, and it outlives Channel::close() on one wrapper.
pub(crate) struct SharedChannel {
    pub(crate) channel: Channel,
    pub(crate) state: Mutex<i64>,
}

/// Process-global persistent channel registry, keyed by target + args + TLS
/// marker. Channels with per-call credentials plugins are NOT persisted
/// (matching ext-grpc, which excludes call-creds composites); their Zvals
/// must never cross threads.
static CHANNEL_REGISTRY: LazyLock<Mutex<HashMap<String, Arc<SharedChannel>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct ChannelInner {
    shared: Arc<SharedChannel>,
    target: String,
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
        let mut creds_id: u64 = 0; // 0 = insecure

        // Validate channel args like ext-grpc: string keys, int/string values.
        // 'credentials' and 'force_new' are extracted before validation there.
        // Also collect a canonical key=value list for the persistence key.
        let mut force_new = false;
        let mut persist_parts: Vec<String> = Vec::new();
        for (key, val) in args.iter() {
            let key_str = match &key {
                ext_php_rs::types::ArrayKey::Long(_) => {
                    return Err(crate::error::invalid_argument("args keys must be strings"));
                }
                other => other.to_string(),
            };
            if key_str == "credentials" {
                continue;
            }
            if key_str == "force_new" {
                force_new = val.bool().unwrap_or(false) || val.long().is_some_and(|l| l != 0);
                continue;
            }
            let val_str = if let Some(l) = val.long() {
                l.to_string()
            } else if let Some(s) = val.string() {
                s
            } else {
                return Err(crate::error::invalid_argument(
                    "args values must be int or string",
                ));
            };
            persist_parts.push(format!("{key_str}={val_str}"));
        }
        persist_parts.sort();

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
                creds_id = creds.id;
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

        // Extract gRPC message size limits early — needed on both the reuse
        // and the build path. -1 means unlimited.
        let max_decoding_message_size = args
            .get("grpc.max_receive_message_length")
            .and_then(|v| v.long())
            .and_then(|v| if v == -1 { Some(usize::MAX) } else if v > 0 { Some(v as usize) } else { None });

        let max_encoding_message_size = args
            .get("grpc.max_send_message_length")
            .and_then(|v| v.long())
            .and_then(|v| if v == -1 { Some(usize::MAX) } else if v > 0 { Some(v as usize) } else { None });

        // Persistent channel reuse (ext-grpc semantics): the same target +
        // args + TLS kind shares one underlying transport across Channel
        // objects and requests. Call-credential composites and force_new
        // channels are never shared.
        let persist_key = if call_plugin.is_none() && !force_new {
            Some(format!(
                "{target}|creds={creds_id}|{}",
                persist_parts.join(",")
            ))
        } else {
            None
        };

        if let Some(ref key) = persist_key {
            let existing = CHANNEL_REGISTRY.lock().get(key).map(Arc::clone);
            if let Some(shared) = existing {
                #[allow(clippy::arc_with_non_send_sync)]
                return Ok(Self {
                    inner: Some(Arc::new(ChannelInner {
                        shared,
                        target,
                        call_plugin,
                        max_decoding_message_size,
                        max_encoding_message_size,
                    })),
                    closed_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                });
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

        // HTTP/2 transport tuning — the C-core channel args ext-grpc forwards
        // to grpc core, mapped onto the tonic/hyper equivalents. Unset args
        // keep hyper's defaults (16 KiB frames, 2 MiB stream / 5 MiB
        // connection windows, adaptive window off).
        if let Some(val) = args.get("grpc.http2.max_frame_size")
            && let Some(n) = val.long()
            && let Ok(n) = u32::try_from(n)
            && (16_384..=16_777_215).contains(&n)
        {
            endpoint = endpoint.max_frame_size(n);
        }
        if let Some(val) = args.get("grpc.http2.lookahead_bytes")
            && let Some(n) = val.long()
            && let Ok(n) = u32::try_from(n)
            && n > 0
        {
            // C-core's per-stream lookahead == initial stream window.
            endpoint = endpoint.initial_stream_window_size(n);
            // Keep the connection window at least as large as the stream
            // window, or the stream window cannot be used.
            endpoint = endpoint.initial_connection_window_size(n.max(5 * 1024 * 1024));
        }
        if let Some(val) = args.get("grpc.http2.bdp_probe")
            && let Some(n) = val.long()
        {
            endpoint = endpoint.http2_adaptive_window(n != 0);
        }

        // Extract user agent
        if let Some(val) = args.get("grpc.primary_user_agent")
            && let Some(ua) = val.string()
        {
            endpoint = endpoint
                .user_agent(ua)
                .map_err(|e| PhpException::from(GrpcError::InvalidArg(e.to_string())))?;
        }

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

        // Bound each TCP connection attempt. Without this a blackholed endpoint
        // ties up a call for the kernel's full SYN-retry budget (~2 min on
        // Linux) even on an infinite-deadline call; C-core's minimum connect
        // deadline is 20 s. Finite call deadlines are enforced separately in
        // the call dispatch paths and fire first when shorter.
        endpoint = endpoint.connect_timeout(Duration::from_secs(20));

        // Use connect_lazy to avoid blocking in constructor
        let channel = endpoint.connect_lazy();

        let shared = Arc::new(SharedChannel {
            channel,
            state: Mutex::new(CHANNEL_IDLE),
        });
        if let Some(key) = persist_key {
            CHANNEL_REGISTRY.lock().insert(key, Arc::clone(&shared));
        }

        // ChannelInner contains Option<Zval> (via call_plugin) which is !Send,
        // but Arc is needed for shared ownership. All access stays on the PHP thread.
        #[allow(clippy::arc_with_non_send_sync)]
        Ok(Self {
            inner: Some(Arc::new(ChannelInner {
                shared,
                target,
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
        let current = *inner.shared.state.lock();

        // try_to_connect=true on an idle channel kicks off a real connection
        // attempt; the state transitions IDLE→CONNECTING→READY/TRANSIENT_FAILURE
        // asynchronously, like C-core.
        let connect = try_to_connect.and_then(Zval::bool).unwrap_or(false);
        if connect && current == CHANNEL_IDLE {
            let shared = Arc::clone(&inner.shared);
            let rt = crate::runtime::get_runtime().map_err(PhpException::from)?;
            *shared.state.lock() = CHANNEL_CONNECTING;
            rt.spawn(async move {
                let mut grpc = tonic::client::Grpc::new(shared.channel.clone());
                let next = match grpc.ready().await {
                    Ok(()) => CHANNEL_READY,
                    Err(_) => CHANNEL_TRANSIENT_FAILURE,
                };
                *shared.state.lock() = next;
            });
            return Ok(current);
        }
        Ok(current)
    }

    /// Watches for a connectivity state change.
    ///
    /// Returns true if state changed before deadline, false on timeout.
    #[php(name = "watchConnectivityState")]
    pub fn watch_connectivity_state(
        &self,
        last_state: i64,
        deadline: &GrpcTimeval,
    ) -> PhpResult<bool> {
        let inner = self.inner.as_ref().ok_or_else(|| {
            crate::error::runtime_exception("watchConnectivityState errorChannel is already closed.")
        })?;

        let shared = Arc::clone(&inner.shared);
        let deadline_usec = deadline.to_absolute_usec();
        let rt = crate::runtime::get_runtime().map_err(PhpException::from)?;

        loop {
            if *shared.state.lock() != last_state {
                return Ok(true);
            }
            let now = crate::call::now_usec();
            if now >= deadline_usec {
                return Ok(false);
            }
            let wait_usec = (deadline_usec.saturating_sub(now)).clamp(1, 10_000);
            rt.block_on(async {
                tokio::time::sleep(Duration::from_micros(wait_usec as u64)).await;
            });
        }
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
        self.inner.as_ref().map(|i| i.shared.channel.clone())
    }

    /// Returns the shared transport/state handle (for state updates by calls).
    pub(crate) fn get_shared(&self) -> Option<Arc<SharedChannel>> {
        self.inner.as_ref().map(|i| Arc::clone(&i.shared))
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
