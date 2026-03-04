use ext_php_rs::prelude::*;
use ext_php_rs::types::Zval;
use parking_lot::Mutex;
use std::sync::Arc;
use tonic::transport::ClientTlsConfig;

static DEFAULT_ROOTS_PEM: Mutex<Option<String>> = Mutex::new(None);

/// Internal representation of TLS configuration.
#[derive(Debug, Clone)]
pub enum CredentialsInner {
    Ssl {
        tls_config: ClientTlsConfig,
    },
    Composite {
        tls_config: ClientTlsConfig,
        call_plugin: Arc<Mutex<Option<Zval>>>,
    },
}

#[php_class]
#[php(name = "Grpc\\ChannelCredentials")]
pub struct GrpcChannelCredentials {
    pub(crate) inner: CredentialsInner,
}

#[php_impl]
impl GrpcChannelCredentials {
    /// Creates SSL credentials.
    ///
    /// PHP signature: createSsl(?string $pem_root_certs, ?string $pem_key, ?string $pem_cert): ChannelCredentials
    #[php(name = "createSsl")]
    pub fn create_ssl(
        pem_root_certs: Option<String>,
        pem_private_key: Option<String>,
        pem_cert_chain: Option<String>,
    ) -> PhpResult<Self> {
        let mut tls = ClientTlsConfig::new();

        // Apply root certs: parameter takes priority, then default, then system roots
        let roots = pem_root_certs.or_else(|| {
            let guard = DEFAULT_ROOTS_PEM.lock();
            guard.clone()
        });
        if let Some(pem) = roots {
            let cert = tonic::transport::Certificate::from_pem(pem);
            tls = tls.ca_certificate(cert);
        }

        // Apply client identity (mutual TLS)
        if let (Some(cert_chain), Some(private_key)) = (pem_cert_chain, pem_private_key) {
            let identity = tonic::transport::Identity::from_pem(cert_chain, private_key);
            tls = tls.identity(identity);
        }

        Ok(Self {
            inner: CredentialsInner::Ssl { tls_config: tls },
        })
    }

    /// Creates insecure credentials.
    ///
    /// MUST return PHP null for drop-in compatibility.
    /// The Channel constructor checks `credentials === null` to mean insecure.
    #[php(name = "createInsecure")]
    pub fn create_insecure() -> Option<Self> {
        None
    }

    /// Creates composite channel credentials (TLS + call credentials).
    #[php(name = "createComposite")]
    pub fn create_composite(
        channel_creds: &GrpcChannelCredentials,
        call_creds: &GrpcCallCredentials,
    ) -> PhpResult<Self> {
        let tls_config = match &channel_creds.inner {
            CredentialsInner::Ssl { tls_config } => tls_config.clone(),
            CredentialsInner::Composite { tls_config, .. } => tls_config.clone(),
        };

        Ok(Self {
            inner: CredentialsInner::Composite {
                tls_config,
                call_plugin: Arc::clone(&call_creds.plugin),
            },
        })
    }

    /// Creates default SSL credentials (system root CAs or custom default PEM).
    #[php(name = "createDefault")]
    pub fn create_default() -> Self {
        let mut tls = ClientTlsConfig::new();

        let guard = DEFAULT_ROOTS_PEM.lock();
        if let Some(ref pem) = *guard {
            let cert = tonic::transport::Certificate::from_pem(pem.clone());
            tls = tls.ca_certificate(cert);
        }
        drop(guard);

        Self {
            inner: CredentialsInner::Ssl { tls_config: tls },
        }
    }

    /// Sets the default root PEM certificates.
    #[php(name = "setDefaultRootsPem")]
    pub fn set_default_roots_pem(pem: String) {
        let mut guard = DEFAULT_ROOTS_PEM.lock();
        *guard = Some(pem);
    }

    /// Checks if default root PEM has been set.
    #[php(name = "isDefaultRootsPemSet")]
    pub fn is_default_roots_pem_set() -> bool {
        let guard = DEFAULT_ROOTS_PEM.lock();
        guard.is_some()
    }

    /// Invalidates the default root PEM.
    #[php(name = "invalidateDefaultRootsPem")]
    pub fn invalidate_default_roots_pem() {
        let mut guard = DEFAULT_ROOTS_PEM.lock();
        *guard = None;
    }
}

// ---------------------------------------------------------------------------
// CallCredentials
// ---------------------------------------------------------------------------

#[php_class]
#[php(name = "Grpc\\CallCredentials")]
pub struct GrpcCallCredentials {
    pub(crate) plugin: Arc<Mutex<Option<Zval>>>,
}

#[php_impl]
impl GrpcCallCredentials {
    /// Creates call credentials from a PHP callable plugin.
    ///
    /// The callable signature: function(string $service_url): array
    /// Returns ['key' => 'value', ...] metadata entries.
    #[php(name = "createFromPlugin")]
    pub fn create_from_plugin(callback: &Zval) -> PhpResult<Self> {
        if !callback.is_callable() {
            return Err(PhpException::default(
                "createFromPlugin expects a callable".into(),
            ));
        }

        // Zval is !Send, but Arc is needed for shared ownership across PHP objects.
        // The callable is ONLY invoked on the PHP thread (before block_on).
        #[allow(clippy::arc_with_non_send_sync)]
        Ok(Self {
            plugin: Arc::new(Mutex::new(Some(callback.shallow_clone()))),
        })
    }

    /// Creates composite call credentials (chains two call credentials).
    #[php(name = "createComposite")]
    pub fn create_composite(
        creds1: &GrpcCallCredentials,
        creds2: &GrpcCallCredentials,
    ) -> PhpResult<Self> {
        // For composite, we store the first plugin only.
        // A full implementation would chain them, but for Phase 1 we use the first.
        let plugin = {
            let guard = creds1.plugin.lock();
            guard.as_ref().map(|z| z.shallow_clone())
        };

        // If creds1 has no plugin, try creds2
        let plugin = plugin.or_else(|| {
            let guard = creds2.plugin.lock();
            guard.as_ref().map(|z| z.shallow_clone())
        });

        #[allow(clippy::arc_with_non_send_sync)]
        Ok(Self {
            plugin: Arc::new(Mutex::new(plugin)),
        })
    }
}
