use ext_php_rs::prelude::*;
use ext_php_rs::types::Zval;

use std::time::{SystemTime, UNIX_EPOCH};

#[php_class]
#[php(name = "Grpc\\Timeval")]
#[derive(Debug, Clone)]
pub struct GrpcTimeval {
    usec: i64,
    /// gpr clock semantics: the bare constructor creates a TIMESPAN (relative
    /// duration), while now()/infFuture()/infPast() are absolute times.
    /// C-core resolves a timespan deadline as now + span at call creation —
    /// `new Timeval(5_000_000)` as a deadline means "5s from now", not 1970.
    is_span: bool,
}

#[php_impl]
impl GrpcTimeval {
    /// ext-grpc accepts ints and (weakly coerced) floats/numeric values here;
    /// callers commonly compute deadlines with float arithmetic.
    pub fn __construct(usec: &Zval) -> PhpResult<Self> {
        let usec = if let Some(l) = usec.long() {
            l
        } else if let Some(d) = usec.double() {
            d as i64
        } else {
            return Err(crate::error::invalid_argument(
                "Timeval expects a numeric microseconds value",
            ));
        };
        Ok(Self { usec, is_span: true })
    }

    /// Returns the internal microsecond value (used by other modules).
    pub fn get_usec(&self) -> i64 {
        self.usec
    }

    /// Resolve to absolute epoch microseconds for use as a deadline, the way
    /// C-core converts clocks at call creation: a TIMESPAN becomes now + span.
    pub fn to_absolute_usec(&self) -> i64 {
        if !self.is_span || self.usec == i64::MAX || self.usec == i64::MIN {
            return self.usec;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_micros()).ok())
            .unwrap_or(0);
        now.saturating_add(self.usec)
    }

    pub fn now() -> PhpResult<Self> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| PhpException::default(e.to_string()))?;
        let usec = i64::try_from(duration.as_micros())
            .map_err(|e| PhpException::default(e.to_string()))?;
        Ok(Self {
            usec,
            is_span: false,
        })
    }

    #[php(name = "infFuture")]
    pub fn inf_future() -> Self {
        Self {
            usec: i64::MAX,
            is_span: false,
        }
    }

    #[php(name = "infPast")]
    pub fn inf_past() -> Self {
        Self {
            usec: i64::MIN,
            is_span: false,
        }
    }

    pub fn zero() -> Self {
        Self {
            usec: 0,
            is_span: true,
        }
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
        // gpr time math saturates at infinities: inf ± finite == inf.
        if self.usec == i64::MAX || self.usec == i64::MIN {
            return Self {
                usec: self.usec,
                is_span: self.is_span,
            };
        }
        Self {
            usec: self.usec.saturating_add(other.usec),
            // absolute + span = absolute; span + span = span
            is_span: self.is_span && other.is_span,
        }
    }

    pub fn subtract(&self, other: &GrpcTimeval) -> Self {
        if self.usec == i64::MAX || self.usec == i64::MIN {
            return Self {
                usec: self.usec,
                is_span: self.is_span,
            };
        }
        Self {
            usec: self.usec.saturating_sub(other.usec),
            is_span: self.is_span && other.is_span,
        }
    }

    /// Blocks until this (absolute) time has been reached.
    #[php(name = "sleepUntil")]
    pub fn sleep_until(&self) -> PhpResult<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| PhpException::default(e.to_string()))?;
        let now_usec = i64::try_from(now.as_micros())
            .map_err(|e| PhpException::default(e.to_string()))?;
        let remaining = self.usec.saturating_sub(now_usec);
        if remaining > 0 && self.usec != i64::MAX {
            let rt = crate::runtime::get_runtime().map_err(PhpException::from)?;
            rt.block_on(tokio::time::sleep(std::time::Duration::from_micros(
                remaining as u64,
            )));
        }
        Ok(())
    }
}
