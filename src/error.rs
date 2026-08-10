use ext_php_rs::exception::PhpException;
use ext_php_rs::zend::{ClassEntry, ce};

/// Build a PhpException with a specific global exception class (SPL or core),
/// matching ext-grpc's use of InvalidArgumentException / LogicException /
/// RuntimeException. Falls back to \Exception if the class is not found.
fn class_exception(class: &str, message: String, code: i32) -> PhpException {
    let entry = ClassEntry::try_find(class).unwrap_or_else(ce::exception);
    PhpException::new(message, code, entry)
}

/// `InvalidArgumentException` with code 1, as thrown by ext-grpc for
/// argument/validation errors.
pub fn invalid_argument(message: impl Into<String>) -> PhpException {
    class_exception("InvalidArgumentException", message.into(), 1)
}

/// `LogicException` carrying a `grpc_call_error` code, as thrown by ext-grpc
/// for batch sequencing errors.
pub fn logic_exception(message: impl Into<String>, call_error: i32) -> PhpException {
    class_exception("LogicException", message.into(), call_error)
}

/// `RuntimeException`, as thrown by ext-grpc for closed-resource misuse.
pub fn runtime_exception(message: impl Into<String>) -> PhpException {
    class_exception("RuntimeException", message.into(), 0)
}

#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error("failed to initialize tokio runtime: {0}")]
    RuntimeInit(#[from] std::io::Error),

    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("gRPC status {code}: {message}")]
    Status { code: i32, message: String },

    #[error("invalid argument: {0}")]
    InvalidArg(String),

    #[error("invalid URI: {0}")]
    InvalidUri(String),

    #[error("callback failed: {0}")]
    CallbackFailed(String),
}

impl From<GrpcError> for PhpException {
    fn from(err: GrpcError) -> Self {
        PhpException::new(err.to_string(), 0, ce::exception())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_messages() {
        let err = GrpcError::InvalidArg("bad input".into());
        assert_eq!(err.to_string(), "invalid argument: bad input");

        let err = GrpcError::InvalidUri("not://valid".into());
        assert_eq!(err.to_string(), "invalid URI: not://valid");

        let err = GrpcError::CallbackFailed("plugin error".into());
        assert_eq!(err.to_string(), "callback failed: plugin error");

        let err = GrpcError::Status {
            code: 14,
            message: "unavailable".into(),
        };
        assert_eq!(err.to_string(), "gRPC status 14: unavailable");

        let io_err = std::io::Error::other("test io");
        let err = GrpcError::RuntimeInit(io_err);
        assert!(err.to_string().contains("test io"));
    }

    #[test]
    fn error_from_io() {
        let io_err = std::io::Error::other("fail");
        let grpc_err: GrpcError = io_err.into();
        assert!(matches!(grpc_err, GrpcError::RuntimeInit(_)));
    }
}
