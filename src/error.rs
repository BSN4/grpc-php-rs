use ext_php_rs::exception::PhpException;
use ext_php_rs::zend::ce;

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
