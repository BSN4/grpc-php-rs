use parking_lot::Mutex;
use tokio::runtime::Runtime;

use crate::error::GrpcError;

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

pub fn get_runtime() -> Result<&'static Runtime, GrpcError> {
    // First, check if already initialized (fast path)
    {
        let guard = RUNTIME.lock();
        if guard.is_some() {
            // SAFETY: Once set, the Runtime is never removed or replaced.
            // The Mutex ensures no concurrent mutation. We return a &'static ref
            // because the static Mutex keeps the Runtime alive for 'static.
            let ptr = guard.as_ref().map(|r| r as *const Runtime);
            if let Some(p) = ptr {
                return Ok(unsafe { &*p });
            }
        }
    }

    // Slow path: initialize
    let rt = Runtime::new().map_err(GrpcError::RuntimeInit)?;
    let mut guard = RUNTIME.lock();
    if guard.is_none() {
        *guard = Some(rt);
    }

    // SAFETY: Same reasoning as above — once set, never removed.
    let ptr = guard
        .as_ref()
        .map(|r| r as *const Runtime)
        .ok_or_else(|| {
            GrpcError::RuntimeInit(std::io::Error::other(
                "runtime initialization failed",
            ))
        })?;
    Ok(unsafe { &*ptr })
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
