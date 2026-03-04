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
