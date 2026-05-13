use std::sync::OnceLock;

use tokio::runtime::Runtime;

use crate::error::GrpcError;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub fn get_runtime() -> Result<&'static Runtime, GrpcError> {
    // OnceLock::get_or_try_init is unstable, so we use a two-step approach:
    // 1. Fast path: already initialized
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    // 2. Slow path: build a runtime and try to set it (only one thread wins)
    let rt = Runtime::new().map_err(GrpcError::RuntimeInit)?;
    // If another thread beat us, our `rt` is dropped (harmless).
    let _ = RUNTIME.set(rt);
    RUNTIME
        .get()
        .ok_or_else(|| GrpcError::RuntimeInit(std::io::Error::other("runtime init failed")))
}

/// Force-initialize the tokio runtime and exercise a dummy async block so that
/// all `thread_local!` statics in tokio, hyper, h2, and rustls eagerly allocate
/// their `pthread_key_t` slots on the **main thread during MINIT** — before
/// FrankenPHP's Go runtime creates PHP worker threads.
///
/// Why this matters: Go/CGo allocates 3 pthread keys per OS thread.  With 350
/// FrankenPHP worker threads that is 1 050 keys — exceeding PTHREAD_KEYS_MAX
/// (1 024).  If any Rust `thread_local!` is still lazy when a worker thread
/// first touches it, `pthread_key_create` returns EAGAIN and the Rust stdlib
/// aborts with "out of TLS keys".  Warming up here ensures every Rust key is
/// allocated while slots are still available; subsequent per-thread access only
/// calls `pthread_setspecific` (which never fails).
pub fn warmup_tls() {
    if let Ok(rt) = get_runtime() {
        // Enter the runtime context — triggers TLS init in tokio's
        // context, driver, and scheduler modules.
        let _guard = rt.enter();

        // Run a trivial future that touches the hyper/h2 code path
        // (Endpoint::connect_lazy → Channel) to force lazy statics in
        // the HTTP/2 and TLS stacks to initialize their keys now.
        rt.block_on(async {
            // A minimal tonic Endpoint + connect_lazy touches:
            //   - hyper connection pool thread-locals
            //   - h2 codec thread-locals
            //   - rustls session cache thread-locals
            let ep = tonic::transport::Endpoint::from_static("http://[::1]:1");
            let _ch = ep.connect_lazy();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_runtime_returns_same_instance() {
        let rt1 = get_runtime();
        assert!(rt1.is_ok(), "first get_runtime() should succeed");
        let rt2 = get_runtime();
        assert!(rt2.is_ok(), "second get_runtime() should succeed");
        // Both should point to the same Runtime
        let ptr1 = rt1.ok().map(|r| r as *const Runtime);
        let ptr2 = rt2.ok().map(|r| r as *const Runtime);
        assert_eq!(ptr1, ptr2, "should return the same runtime instance");
    }

    #[test]
    fn runtime_can_spawn_task() {
        let rt = get_runtime();
        assert!(rt.is_ok());
        let rt = rt.ok();
        assert!(rt.is_some());
        if let Some(rt) = rt {
            let result = rt.block_on(async { 42 });
            assert_eq!(result, 42);
        }
    }
}
