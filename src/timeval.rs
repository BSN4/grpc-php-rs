use ext_php_rs::prelude::*;

use std::time::{SystemTime, UNIX_EPOCH};

#[php_class]
#[php(name = "Grpc\\Timeval")]
#[derive(Debug, Clone)]
pub struct GrpcTimeval {
    usec: i64,
}

#[php_impl]
impl GrpcTimeval {
    pub fn __construct(usec: i64) -> Self {
        Self { usec }
    }

    /// Returns the internal microsecond value (used by other modules).
    pub fn get_usec(&self) -> i64 {
        self.usec
    }

    pub fn now() -> PhpResult<Self> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| PhpException::default(e.to_string()))?;
        let usec = i64::try_from(duration.as_micros())
            .map_err(|e| PhpException::default(e.to_string()))?;
        Ok(Self { usec })
    }

    #[php(name = "infFuture")]
    pub fn inf_future() -> Self {
        Self { usec: i64::MAX }
    }

    #[php(name = "infPast")]
    pub fn inf_past() -> Self {
        Self { usec: i64::MIN }
    }

    pub fn zero() -> Self {
        Self { usec: 0 }
    }

    pub fn similar(a: &GrpcTimeval, b: &GrpcTimeval, threshold: &GrpcTimeval) -> bool {
        let diff = (a.usec.saturating_sub(b.usec)).saturating_abs();
        diff <= threshold.usec.saturating_abs()
    }

    pub fn compare(a: &GrpcTimeval, b: &GrpcTimeval) -> i64 {
        match a.usec.cmp(&b.usec) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    pub fn add(&self, other: &GrpcTimeval) -> Self {
        Self {
            usec: self.usec.saturating_add(other.usec),
        }
    }

    pub fn subtract(&self, other: &GrpcTimeval) -> Self {
        Self {
            usec: self.usec.saturating_sub(other.usec),
        }
    }
}
