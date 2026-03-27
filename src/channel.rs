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
}

#[php_class]
#[php(name = "Grpc\\Channel")]
pub struct GrpcChannel {
    inner: Option<Arc<ChannelInner>>,
}

#[php_impl]
impl GrpcChannel {
    /// Constructs a new Channel.
    ///
    /// PHP signature: __construct(string $target, array $args)
    pub fn __construct(target: String, args: &ZendHashTable) -> PhpResult<Self> {
        let mut tls_config = None;
        let mut call_plugin = None;

        // Extract credentials from args first — we need to know if TLS is
        // required before building the URI.
        if let Some(creds_zval) = args.get("credentials") {
            // If credentials is null, it means insecure (from createInsecure())
            if !creds_zval.is_null()
                && let Some(creds) = creds_zval.extract::<&GrpcChannelCredentials>()
            {
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
            })),
        })
    }

    /// Returns the target URI.
    #[php(name = "getTarget")]
    pub fn get_target(&self) -> PhpResult<String> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| PhpException::default("Channel has been closed".into()))?;
        Ok(inner.target.clone())
    }

    /// Returns the connectivity state.
    #[php(name = "getConnectivityState")]
    pub fn get_connectivity_state(&self, _try_to_connect: Option<bool>) -> PhpResult<i64> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| PhpException::default("Channel has been closed".into()))?;
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
        let _inner = self
            .inner
            .as_ref()
            .ok_or_else(|| PhpException::default("Channel has been closed".into()))?;

        // Tonic doesn't expose connectivity state watching directly.
        // For Phase 1, we return true (state changed) immediately.
        // This matches the common usage pattern where callers just want to proceed.
        Ok(true)
    }

    /// Closes the channel.
    pub fn close(&mut self) {
        self.inner = None;
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

    /// Returns the call plugin if composite credentials were used.
    pub(crate) fn get_call_plugin(&self) -> Option<Arc<Mutex<Option<Zval>>>> {
        self.inner
            .as_ref()
            .and_then(|i| i.call_plugin.as_ref().map(Arc::clone))
    }
}
