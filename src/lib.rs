#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    unused_must_use,
    clippy::await_holding_lock,
    clippy::await_holding_refcell_ref,
    clippy::disallowed_types,
    clippy::await_holding_invalid_type,
    clippy::todo,
    clippy::unimplemented,
    clippy::dbg_macro,
    clippy::print_stdout,
    clippy::print_stderr
)]

mod call;
mod channel;
mod codec;
mod credentials;
mod error;
mod runtime;
mod timeval;

use ext_php_rs::prelude::*;

// Re-export PHP classes so ext-php-rs can register them.
pub use call::GrpcCall;
pub use channel::GrpcChannel;
pub use credentials::{GrpcCallCredentials, GrpcChannelCredentials};
pub use timeval::GrpcTimeval;

// ---------------------------------------------------------------------------
// Return-type stripping — ext-php-rs always declares typed returns, but the
// C extension has untyped methods.  PHP 8.5 enforces covariant return types,
// so child classes in grpc/grpc (InterceptorChannel) fail if we declare types.
// We strip them in the first RINIT after classes are registered in MINIT.
// ---------------------------------------------------------------------------

mod compat;

// ---------------------------------------------------------------------------
// Constants — registered via module startup
// ---------------------------------------------------------------------------

fn register_constants(_ty: i32, mod_num: i32) -> i32 {
    use ext_php_rs::constant::IntoConst;

    macro_rules! reg {
        ($name:expr, $val:expr, $mod:expr) => {
            if $val.register_constant($name, $mod).is_err() {
                return -1;
            }
        };
    }

    // Status codes
    reg!("Grpc\\STATUS_OK", 0i64, mod_num);
    reg!("Grpc\\STATUS_CANCELLED", 1i64, mod_num);
    reg!("Grpc\\STATUS_UNKNOWN", 2i64, mod_num);
    reg!("Grpc\\STATUS_INVALID_ARGUMENT", 3i64, mod_num);
    reg!("Grpc\\STATUS_DEADLINE_EXCEEDED", 4i64, mod_num);
    reg!("Grpc\\STATUS_NOT_FOUND", 5i64, mod_num);
    reg!("Grpc\\STATUS_ALREADY_EXISTS", 6i64, mod_num);
    reg!("Grpc\\STATUS_PERMISSION_DENIED", 7i64, mod_num);
    reg!("Grpc\\STATUS_RESOURCE_EXHAUSTED", 8i64, mod_num);
    reg!("Grpc\\STATUS_FAILED_PRECONDITION", 9i64, mod_num);
    reg!("Grpc\\STATUS_ABORTED", 10i64, mod_num);
    reg!("Grpc\\STATUS_OUT_OF_RANGE", 11i64, mod_num);
    reg!("Grpc\\STATUS_UNIMPLEMENTED", 12i64, mod_num);
    reg!("Grpc\\STATUS_INTERNAL", 13i64, mod_num);
    reg!("Grpc\\STATUS_UNAVAILABLE", 14i64, mod_num);
    reg!("Grpc\\STATUS_DATA_LOSS", 15i64, mod_num);
    reg!("Grpc\\STATUS_UNAUTHENTICATED", 16i64, mod_num);

    // Channel connectivity states
    reg!("Grpc\\CHANNEL_IDLE", 0i64, mod_num);
    reg!("Grpc\\CHANNEL_CONNECTING", 1i64, mod_num);
    reg!("Grpc\\CHANNEL_READY", 2i64, mod_num);
    reg!("Grpc\\CHANNEL_TRANSIENT_FAILURE", 3i64, mod_num);
    reg!("Grpc\\CHANNEL_FATAL_FAILURE", 4i64, mod_num);

    // Call error codes
    reg!("Grpc\\CALL_OK", 0i64, mod_num);
    reg!("Grpc\\CALL_ERROR", 1i64, mod_num);
    reg!("Grpc\\CALL_ERROR_NOT_ON_SERVER", 2i64, mod_num);
    reg!("Grpc\\CALL_ERROR_NOT_ON_CLIENT", 3i64, mod_num);
    reg!("Grpc\\CALL_ERROR_ALREADY_ACCEPTED", 4i64, mod_num);
    reg!("Grpc\\CALL_ERROR_ALREADY_INVOKED", 5i64, mod_num);
    reg!("Grpc\\CALL_ERROR_NOT_INVOKED", 6i64, mod_num);
    reg!("Grpc\\CALL_ERROR_ALREADY_FINISHED", 7i64, mod_num);
    reg!("Grpc\\CALL_ERROR_INVALID_FLAGS", 8i64, mod_num);

    // Batch operation codes
    reg!("Grpc\\OP_SEND_INITIAL_METADATA", 0i64, mod_num);
    reg!("Grpc\\OP_SEND_MESSAGE", 1i64, mod_num);
    reg!("Grpc\\OP_SEND_CLOSE_FROM_CLIENT", 2i64, mod_num);
    reg!("Grpc\\OP_SEND_STATUS_FROM_SERVER", 3i64, mod_num);
    reg!("Grpc\\OP_RECV_INITIAL_METADATA", 4i64, mod_num);
    reg!("Grpc\\OP_RECV_MESSAGE", 5i64, mod_num);
    reg!("Grpc\\OP_RECV_STATUS_ON_CLIENT", 6i64, mod_num);
    reg!("Grpc\\OP_RECV_CLOSE_ON_SERVER", 7i64, mod_num);

    // Write flags
    reg!("Grpc\\WRITE_BUFFER_HINT", 1i64, mod_num);
    reg!("Grpc\\WRITE_NO_COMPRESS", 2i64, mod_num);

    // Version
    if "1.78.0"
        .to_string()
        .register_constant("Grpc\\VERSION", mod_num)
        .is_err()
    {
        return -1;
    }

    0
}

#[php_module]
#[php(startup = register_constants)]
pub fn get_module(module: ModuleBuilder) -> ModuleBuilder {
    module
        .name("grpc")
        .class::<GrpcChannel>()
        .class::<GrpcCall>()
        .class::<GrpcChannelCredentials>()
        .class::<GrpcCallCredentials>()
        .class::<GrpcTimeval>()
        .request_startup_function(compat::strip_return_types)
}
