//! Compatibility layer: strips return type declarations from PHP methods.
//!
//! ext-php-rs always generates typed return values (e.g. `: string`, `: int`)
//! for PHP methods.  The official C grpc extension declares NO return types.
//! PHP 8.5 strictly enforces return type covariance, so child classes in the
//! `grpc/grpc` Composer package (e.g. `InterceptorChannel`) fail with:
//!
//!   Fatal error: Declaration of InterceptorChannel::getTarget()
//!   must be compatible with Grpc\Channel::getTarget(): string
//!
//! This module clears the `ZEND_ACC_HAS_RETURN_TYPE` flag and zeroes the
//! return type info on our methods during the first RINIT (request startup),
//! after MINIT has already registered the classes.

use std::ffi::c_char;

use ext_php_rs::ffi::{
    zend_hash_str_find_ptr_lc, zend_internal_function, ZEND_ACC_HAS_RETURN_TYPE,
};
use ext_php_rs::zend::ClassEntry;
use parking_lot::Once;

static INIT: Once = Once::new();

/// Methods on `Grpc\Channel` whose return types must be stripped.
/// Names must be lowercase (zend_hash_str_find_ptr_lc is case-insensitive).
const CHANNEL_METHODS: &[&str] = &[
    "gettarget",
    "getconnectivitystate",
    "watchconnectivitystate",
    "close",
];

/// Methods on `Grpc\Call` whose return types must be stripped.
const CALL_METHODS: &[&str] = &["startbatch", "getpeer", "cancel", "setcredentials"];

/// RINIT handler — runs each request, but the actual patching happens only once.
///
/// # Safety
///
/// Called by the PHP engine during request startup.
pub unsafe extern "C" fn strip_return_types(_ty: i32, _mod_num: i32) -> i32 {
    INIT.call_once(|| {
        // SAFETY: We are in RINIT, classes are registered in the class table.
        // We modify fn_flags and arg_info of internal functions — these are
        // heap-allocated by ext-php-rs and live for the process lifetime.
        unsafe {
            strip_class_methods("Grpc\\Channel", CHANNEL_METHODS);
            strip_class_methods("Grpc\\Call", CALL_METHODS);
        }
    });
    0
}

/// Strip return type declarations from the given methods of a class.
///
/// # Safety
///
/// Must be called in a valid PHP execution context after MINIT.
unsafe fn strip_class_methods(class_name: &str, methods: &[&str]) {
    let Some(ce) = ClassEntry::try_find(class_name) else {
        return;
    };

    for method in methods {
        unsafe {
            strip_method_return_type(ce, method);
        }
    }
}

/// Clear the return type declaration from a single internal method.
///
/// # Safety
///
/// The class entry must be valid and the method must be an internal function.
unsafe fn strip_method_return_type(ce: &ClassEntry, method_name: &str) {
    // Look up the function in the class's function table.
    // zend_hash_str_find_ptr_lc returns a pointer INTO the hash table (not a copy).
    let func_ptr = unsafe {
        zend_hash_str_find_ptr_lc(
            &raw const ce.function_table,
            method_name.as_ptr().cast::<c_char>(),
            method_name.len(),
        )
    };

    if func_ptr.is_null() {
        return;
    }

    // The returned pointer is to a zend_function union.  For internal functions
    // (which all ext-php-rs methods are), we can safely access it as
    // zend_internal_function since the union fields overlap at the same offsets.
    let internal = func_ptr.cast::<zend_internal_function>();

    // Clear the HAS_RETURN_TYPE flag from fn_flags.
    // This is sufficient — PHP checks this flag before reading the return type
    // from arg_info[0], so we don't need to modify the arg_info itself.
    unsafe {
        (*internal).fn_flags &= !ZEND_ACC_HAS_RETURN_TYPE;
    }
}
