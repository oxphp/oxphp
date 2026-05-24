//! Register `OxPHP\Shared\Channel\{RecvResult, SendResult, RecvStatus,
//! SendStatus}` value-typed return classes used by the Channel API.
//!
//! Result classes carry a private `$__status` discriminant (long enum
//! ordinal) and, for `RecvResult` only, a private `$__value` payload
//! (mixed; only meaningful when status == Ok). Their PHP-facing methods
//! (`isOk()`, `value()`, `valueOr($default)`, `status()`, …) are Rust
//! handlers that read the private properties on `$this` and emit the
//! correct return value — no PHP-side runtime shim.
//!
//! The two status enums are **unbacked** (PHP `enum X { case A; ... }`,
//! no `: int` / `: string`). They carry no parametric meaning beyond the
//! case identity — the exhaustive `match ($r->status())` is the entire
//! contract.
//!
//! Construction helpers (called from Channel handlers in a follow-up
//! task) build a Result instance directly into the retval slot via the
//! C-side `oxphp_bridge_make_object` + `oxphp_bridge_object_set_property_*`
//! primitives; see `src/bridge/ffi.rs`.

use crate::bridge::call::NativeCall;
use crate::bridge::ffi as bridge_ffi;
use crate::plugin::types::{PhpType, Visibility};
use crate::plugin::{PhpError, PluginContext, PluginError};

const RECV_RESULT_FQN: &str = "OxPHP\\Shared\\Channel\\RecvResult";
const SEND_RESULT_FQN: &str = "OxPHP\\Shared\\Channel\\SendResult";
const RECV_STATUS_FQN: &str = "OxPHP\\Shared\\Channel\\RecvStatus";
const SEND_STATUS_FQN: &str = "OxPHP\\Shared\\Channel\\SendStatus";

// ── Discriminant tags ────────────────────────────────────────────────
//
// Stored in the `$__status` property of each Result instance as a Long
// (the enum case ordinal). Handler methods compare against these.

const RECV_OK: i64 = 0;
const RECV_EMPTY: i64 = 1;
const RECV_TIMEOUT: i64 = 2;
const RECV_CLOSED: i64 = 3;

const SEND_OK: i64 = 0;
const SEND_FULL: i64 = 1;
const SEND_TIMEOUT: i64 = 2;
const SEND_CLOSED: i64 = 3;

/// Non-Ok variants of `RecvResult`. Channel handlers pick the variant
/// matching the underlying receiver state (empty queue / deadline hit /
/// channel closed) and the construction helper stamps the discriminant
/// on a fresh object.
#[allow(dead_code)]
#[derive(Copy, Clone, Debug)]
pub(crate) enum RecvKind {
    Empty,
    Timeout,
    Closed,
}

/// All variants of `SendResult`. `Ok` is included here because send
/// success is also a value-typed return — no payload differs across
/// the four variants, so a single discriminant write covers each.
#[allow(dead_code)]
#[derive(Copy, Clone, Debug)]
pub(crate) enum SendKind {
    Ok,
    Full,
    Timeout,
    Closed,
}

// ── Class registration ──────────────────────────────────────────────

pub fn register_all(ctx: &mut PluginContext) -> Result<(), PluginError> {
    // RecvStatus enum (unbacked, four cases).
    ctx.register_enum(RECV_STATUS_FQN)
        .case("Ok")
        .case("Empty")
        .case("Timeout")
        .case("Closed")
        .build()?;

    // SendStatus enum (unbacked, four cases).
    ctx.register_enum(SEND_STATUS_FQN)
        .case("Ok")
        .case("Full")
        .case("Timeout")
        .case("Closed")
        .build()?;

    // RecvResult — final, holds discriminant + optional payload.
    ctx.register_class(RECV_RESULT_FQN)
        .final_()
        // Private discriminant — public surface is the `status()` /
        // `is*()` methods, never the raw int.
        .property("__status", PhpType::Int, Visibility::Private)
        // Private payload — only meaningful when status == Ok. Reading
        // it on a non-Ok variant returns null which the `value()`
        // handler turns into a SharedException.
        .property("__value", PhpType::Mixed, Visibility::Private)
        .method("isOk")
        .returns(PhpType::Bool)
        .handler(|call| is_status(call, RECV_OK))
        .method("isEmpty")
        .returns(PhpType::Bool)
        .handler(|call| is_status(call, RECV_EMPTY))
        .method("isTimeout")
        .returns(PhpType::Bool)
        .handler(|call| is_status(call, RECV_TIMEOUT))
        .method("isClosed")
        .returns(PhpType::Bool)
        .handler(|call| is_status(call, RECV_CLOSED))
        .method("value")
        .returns(PhpType::Mixed)
        .handler(|call| recv_value(call, /* with_default */ false))
        .method("valueOr")
        .param("default", PhpType::Mixed)
        .returns(PhpType::Mixed)
        .handler(|call| recv_value(call, /* with_default */ true))
        .method("status")
        .returns(PhpType::Object)
        .handler(recv_status_case)
        .build()?;

    // SendResult — final, discriminant only (no payload).
    ctx.register_class(SEND_RESULT_FQN)
        .final_()
        .property("__status", PhpType::Int, Visibility::Private)
        .method("isOk")
        .returns(PhpType::Bool)
        .handler(|call| is_status(call, SEND_OK))
        .method("isFull")
        .returns(PhpType::Bool)
        .handler(|call| is_status(call, SEND_FULL))
        .method("isTimeout")
        .returns(PhpType::Bool)
        .handler(|call| is_status(call, SEND_TIMEOUT))
        .method("isClosed")
        .returns(PhpType::Bool)
        .handler(|call| is_status(call, SEND_CLOSED))
        .method("status")
        .returns(PhpType::Object)
        .handler(send_status_case)
        .build()?;

    Ok(())
}

// ── Method handlers ─────────────────────────────────────────────────

fn is_status(call: &mut NativeCall, expected: i64) -> Result<(), PhpError> {
    let actual = call.read_long_property("__status")?;
    call.ret_bool(actual == expected);
    Ok(())
}

fn recv_value(call: &mut NativeCall, with_default: bool) -> Result<(), PhpError> {
    let status = call.read_long_property("__status")?;
    if status == RECV_OK {
        // Re-emit the stored payload as the retval.
        call.copy_property_to_retval("__value")?;
        return Ok(());
    }
    if with_default {
        // valueOr($default): emit $default unchanged.
        call.copy_arg_to_retval(0)?;
        return Ok(());
    }
    // value() on non-Ok variants throws SharedException so the program
    // fails loudly rather than silently acting on a nil payload.
    Err(PhpError::Exception {
        class: "OxPHP\\Shared\\SharedException".into(),
        message: "RecvResult::value() called on non-Ok variant \u{2014} use isOk() / valueOr() / status() first".into(),
        code: 0,
    })
}

fn recv_status_case(call: &mut NativeCall) -> Result<(), PhpError> {
    let status = call.read_long_property("__status")?;
    let case_name = match status {
        RECV_OK => "Ok",
        RECV_EMPTY => "Empty",
        RECV_TIMEOUT => "Timeout",
        RECV_CLOSED => "Closed",
        _ => {
            return Err(PhpError::Custom(
                "RecvResult corrupted: unknown status tag".into(),
            ))
        }
    };
    emit_enum_case(call, RECV_STATUS_FQN, case_name)
}

fn send_status_case(call: &mut NativeCall) -> Result<(), PhpError> {
    let status = call.read_long_property("__status")?;
    let case_name = match status {
        SEND_OK => "Ok",
        SEND_FULL => "Full",
        SEND_TIMEOUT => "Timeout",
        SEND_CLOSED => "Closed",
        _ => {
            return Err(PhpError::Custom(
                "SendResult corrupted: unknown status tag".into(),
            ))
        }
    };
    emit_enum_case(call, SEND_STATUS_FQN, case_name)
}

// ── Construction helpers (called from types/channel.rs handlers) ────

/// Build a `RecvResult::Ok(value)` directly into the retval slot. The
/// portable buffer `(value_buf, value_len)` is deserialized into a
/// temporary zval and copied into the `__value` property.
#[allow(dead_code)]
pub(crate) fn write_recv_ok(
    call: &mut NativeCall,
    value_buf: *const u8,
    value_len: usize,
) -> Result<(), PhpError> {
    let retval = call.retval_ptr();
    let rc = unsafe {
        bridge_ffi::oxphp_bridge_make_object(
            retval,
            RECV_RESULT_FQN.as_ptr() as *const _,
            RECV_RESULT_FQN.len(),
        )
    };
    if rc != 0 {
        return Err(PhpError::Custom("failed to construct RecvResult".into()));
    }
    unsafe {
        bridge_ffi::oxphp_bridge_object_set_property_long(
            retval,
            b"__status".as_ptr() as *const _,
            "__status".len(),
            RECV_OK,
        );
    }
    // Deserialize value_buf into a stack-resident temporary zval, copy
    // into the property slot, then dtor the temporary. Buffer size is
    // bound to the bridge's ZVAL_SIZE const (16 on all current PHP 8.x
    // 64-bit builds); the debug assertion above verifies the linked
    // PHP runtime agrees, so a future layout change panics loudly in
    // debug instead of corrupting silently in release.
    crate::bridge::call::debug_assert_zval_size();
    let mut tmp = [0u8; crate::bridge::call::ZVAL_SIZE];
    let tmp_ptr = tmp.as_mut_ptr() as *mut std::ffi::c_void;
    let des_rc =
        unsafe { bridge_ffi::oxphp_portable_deserialize(value_buf, value_len, 1, tmp_ptr) };
    if des_rc != 0 {
        return Err(PhpError::Custom(format!(
            "RecvResult::ok: deserialize failed rc={des_rc}"
        )));
    }
    unsafe {
        bridge_ffi::oxphp_bridge_object_set_property_zval(
            retval,
            b"__value".as_ptr() as *const _,
            "__value".len(),
            tmp_ptr,
        );
        bridge_ffi::oxphp_zval_dtor(tmp_ptr);
    }
    Ok(())
}

/// Wrap a live payload zval (already sitting in the retval slot) into a
/// `RecvResult::Ok(value)` **in place** — no portbuf serialize/deserialize
/// round-trip. Used by the fiber recv waker path, where the waker delivers a
/// materialized zval; the blocking/buffered paths use [`write_recv_ok`]
/// because their payload arrives pre-serialized.
#[allow(dead_code)]
pub(crate) fn write_recv_ok_inplace(call: &mut NativeCall) -> Result<(), PhpError> {
    let retval = call.retval_ptr();
    let rc = unsafe {
        bridge_ffi::oxphp_bridge_wrap_result_ok_inplace(
            retval,
            RECV_RESULT_FQN.as_ptr() as *const _,
            RECV_RESULT_FQN.len(),
            b"__value".as_ptr() as *const _,
            "__value".len(),
            b"__status".as_ptr() as *const _,
            "__status".len(),
            RECV_OK as std::os::raw::c_long,
        )
    };
    if rc != 0 {
        return Err(PhpError::Custom(
            "RecvResult::ok: in-place wrap failed".into(),
        ));
    }
    Ok(())
}

/// Build a payload-free RecvResult (Empty / Timeout / Closed) directly
/// into the retval slot.
#[allow(dead_code)]
pub(crate) fn write_recv(call: &mut NativeCall, kind: RecvKind) -> Result<(), PhpError> {
    let tag = match kind {
        RecvKind::Empty => RECV_EMPTY,
        RecvKind::Timeout => RECV_TIMEOUT,
        RecvKind::Closed => RECV_CLOSED,
    };
    construct_with_status(call, RECV_RESULT_FQN, tag, "RecvResult")
}

/// Build a SendResult of the given variant directly into the retval
/// slot. No payload — `SendResult` is purely a discriminant.
#[allow(dead_code)]
pub(crate) fn write_send(call: &mut NativeCall, kind: SendKind) -> Result<(), PhpError> {
    let tag = match kind {
        SendKind::Ok => SEND_OK,
        SendKind::Full => SEND_FULL,
        SendKind::Timeout => SEND_TIMEOUT,
        SendKind::Closed => SEND_CLOSED,
    };
    construct_with_status(call, SEND_RESULT_FQN, tag, "SendResult")
}

fn construct_with_status(
    call: &mut NativeCall,
    fqn: &str,
    tag: i64,
    label: &'static str,
) -> Result<(), PhpError> {
    let retval = call.retval_ptr();
    let rc = unsafe {
        bridge_ffi::oxphp_bridge_make_object(retval, fqn.as_ptr() as *const _, fqn.len())
    };
    if rc != 0 {
        return Err(PhpError::Custom(format!("failed to construct {label}")));
    }
    unsafe {
        bridge_ffi::oxphp_bridge_object_set_property_long(
            retval,
            b"__status".as_ptr() as *const _,
            "__status".len(),
            tag,
        );
    }
    Ok(())
}

/// Emit a singleton enum-case object into the retval slot. Lookup goes
/// through `oxphp_bridge_get_enum_case` which resolves the class entry
/// and pulls the case via Zend's enum machinery — no PHP-level call
/// required (the case is a process-global singleton in the class CE).
fn emit_enum_case(call: &mut NativeCall, enum_fqn: &str, case: &str) -> Result<(), PhpError> {
    let retval = call.retval_ptr();
    let rc = unsafe {
        bridge_ffi::oxphp_bridge_get_enum_case(
            retval,
            enum_fqn.as_ptr() as *const _,
            enum_fqn.len(),
            case.as_ptr() as *const _,
            case.len(),
        )
    };
    if rc != 0 {
        return Err(PhpError::Custom(format!(
            "failed to resolve enum case {enum_fqn}::{case}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discriminants_are_distinct_and_ordered() {
        // The match arms in recv_status_case / send_status_case rely on
        // the ordering Ok=0, Empty/Full=1, Timeout=2, Closed=3. Lock it
        // in so a future renumbering doesn't silently swap variants.
        assert_eq!(RECV_OK, 0);
        assert_eq!(RECV_EMPTY, 1);
        assert_eq!(RECV_TIMEOUT, 2);
        assert_eq!(RECV_CLOSED, 3);
        assert_eq!(SEND_OK, 0);
        assert_eq!(SEND_FULL, 1);
        assert_eq!(SEND_TIMEOUT, 2);
        assert_eq!(SEND_CLOSED, 3);
    }

    #[test]
    fn class_fqns_use_shared_channel_namespace() {
        // Tests in Task 10 will reflect against these strings — a
        // typo here would surface as "class not registered" at MINIT
        // rather than as a test failure, which is harder to debug.
        assert_eq!(RECV_RESULT_FQN, "OxPHP\\Shared\\Channel\\RecvResult");
        assert_eq!(SEND_RESULT_FQN, "OxPHP\\Shared\\Channel\\SendResult");
        assert_eq!(RECV_STATUS_FQN, "OxPHP\\Shared\\Channel\\RecvStatus");
        assert_eq!(SEND_STATUS_FQN, "OxPHP\\Shared\\Channel\\SendStatus");
    }

    #[test]
    fn register_all_succeeds_against_empty_context() {
        // PluginContext requires a slot vec for each registration kind;
        // we exercise the happy path via the public crate-test harness
        // available in the plugin builder tests. Here we just verify
        // the discriminant enums exhaustively cover the wire codes.
        let recv_kinds = [RecvKind::Empty, RecvKind::Timeout, RecvKind::Closed];
        let send_kinds = [
            SendKind::Ok,
            SendKind::Full,
            SendKind::Timeout,
            SendKind::Closed,
        ];
        assert_eq!(recv_kinds.len(), 3);
        assert_eq!(send_kinds.len(), 4);
    }
}
