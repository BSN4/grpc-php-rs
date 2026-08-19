//! Tokio runtime ownership.
//!
//! Default: one `current_thread` runtime **per PHP thread**, driven by that PHP
//! thread from inside `block_on` while it waits for an RPC. Measured against
//! the previous shared multi-thread runtime this halves unary latency and cuts
//! per-RPC CPU by ~60%: every RPC used to hop PHP thread → worker → I/O driver
//! → PHP thread; now the PHP thread runs the I/O itself while it would
//! otherwise be parked. Under ZTS each PHP thread gets its own runtime, so
//! there is no cross-thread contention at all.
//!
//! Trade-off: nothing progresses while the PHP thread is outside a gRPC call.
//! Server GOAWAY/PING frames are handled at the next call (worst case one
//! reconnect), eagerly dispatched requests go out when the first wait drives
//! the scheduler, and deadline timers fire on the next entry. PHP futures never
//! touch PHP memory, so the allocator invariant is unchanged.
//!
//! `GRPC_PHP_RS_RUNTIME=multi-thread` restores a process-wide multi-thread
//! runtime (background progress, shared workers) for workloads that need it.

use std::cell::OnceCell;
use std::sync::OnceLock;

use tokio::runtime::{Builder, Runtime};

use crate::error::GrpcError;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    CurrentThread,
    MultiThread,
}

static MODE: OnceLock<Mode> = OnceLock::new();
static MULTI: OnceLock<Runtime> = OnceLock::new();

thread_local! {
    // Leaked on purpose: a `&'static Runtime` keeps the call sites simple and
    // PHP threads are long-lived pools (FPM: one per process; FrankenPHP: a
    // fixed pool), so the count is bounded.
    static LOCAL: OnceCell<&'static Runtime> = const { OnceCell::new() };
}

fn mode() -> Mode {
    *MODE.get_or_init(|| {
        match std::env::var("GRPC_PHP_RS_RUNTIME").ok().as_deref() {
            Some("multi-thread") | Some("multi_thread") | Some("mt") => Mode::MultiThread,
            _ => Mode::CurrentThread,
        }
    })
}

/// Returns the runtime for the current PHP thread (or the shared multi-thread
/// runtime in multi-thread mode). Callers use it with `block_on`, `spawn`, and
/// `enter`, always from the PHP thread that owns it.
pub fn get_runtime() -> Result<&'static Runtime, GrpcError> {
    match mode() {
        Mode::MultiThread => {
            if let Some(rt) = MULTI.get() {
                return Ok(rt);
            }
            let rt = Runtime::new().map_err(GrpcError::RuntimeInit)?;
            let _ = MULTI.set(rt);
            MULTI
                .get()
                .ok_or_else(|| GrpcError::RuntimeInit(std::io::Error::other("runtime init failed")))
        }
        Mode::CurrentThread => LOCAL.with(|cell| {
            if let Some(rt) = cell.get() {
                return Ok(*rt);
            }
            let rt = Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(GrpcError::RuntimeInit)?;
            let rt: &'static Runtime = Box::leak(Box::new(rt));
            // A racing init on the same thread is impossible (thread-local).
            let _ = cell.set(rt);
            Ok(rt)
        }),
    }
}

/// Drive the current thread's runtime briefly without blocking on I/O: each
/// `yield_now` tick lets the scheduler run queued tasks and poll the I/O
/// driver with a zero timeout. Used by state queries that must observe
/// progress of background work (connectivity probes) between gRPC calls,
/// since in current-thread mode nothing runs while PHP is outside `block_on`.
pub fn pump() -> Result<(), GrpcError> {
    let rt = get_runtime()?;
    rt.block_on(async {
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    });
    Ok(())
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
        let ptr1 = rt1.ok().map(|r| r as *const Runtime);
        let ptr2 = rt2.ok().map(|r| r as *const Runtime);
        assert_eq!(ptr1, ptr2, "should return the same runtime instance on one thread");
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

    #[test]
    fn spawned_tasks_run_when_driven() {
        let rt = get_runtime();
        assert!(rt.is_ok());
        if let Ok(rt) = rt {
            let handle = rt.spawn(async { 7 });
            let v = rt.block_on(async { handle.await });
            assert!(matches!(v, Ok(7)));
        }
    }
}
