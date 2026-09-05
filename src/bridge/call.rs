use super::ffi;
use super::types::ValType;
use crate::plugin::PhpError;
use std::marker::PhantomData;
use std::os::raw::{c_char, c_void};

/// Zval-sized slot with correct alignment (8 bytes for the pointer/long/double union).
/// PHP zval is always 16 bytes on 64-bit: 8-byte value + 4-byte type_info + 4-byte u2.
#[repr(C, align(8))]
#[derive(Clone)]
pub(crate) struct ZvalSlot([u8; 16]);

/// Safe wrapper over raw zval pointers for a plugin function call.
///
/// Provides bounds-checked, type-checked access to arguments and
/// direct zval writing for the return value. The lifetime `'a` is
/// tied to the current function invocation — do not store or send.
pub struct NativeCall<'a> {
    args: *mut c_void,
    argc: u32,
    retval: *mut c_void,
    object_id: Option<u64>,
    rust_data: Option<*mut c_void>,
    this_zval: *mut c_void,
    _marker: PhantomData<&'a ()>,
}

impl<'a> NativeCall<'a> {
    /// Create from raw pointers (called from dispatch callback).
    ///
    /// # Safety
    /// `args` and `retval` must be valid zval pointers for the duration of `'a`.
    /// `rust_data`, if `Some`, must point to a valid object of the expected type
    /// for the duration of `'a`.
    /// `this_zval`, if non-null, must point to a valid object zval for `'a`.
    #[allow(dead_code)]
    pub(crate) unsafe fn new(
        args: *mut c_void,
        argc: u32,
        retval: *mut c_void,
        object_id: Option<u64>,
        rust_data: Option<*mut c_void>,
    ) -> Self {
        Self {
            args,
            argc,
            retval,
            object_id,
            rust_data,
            this_zval: std::ptr::null_mut(),
            _marker: PhantomData,
        }
    }

    /// Create from raw pointers including the `$this` zval pointer.
    ///
    /// # Safety
    /// Same as `new`, plus: `this_zval` must point to a valid object zval
    /// for the duration of `'a`, or be null for static / free-function calls.
    #[allow(dead_code)]
    pub(crate) unsafe fn new_with_this(
        args: *mut c_void,
        argc: u32,
        retval: *mut c_void,
        object_id: Option<u64>,
        rust_data: Option<*mut c_void>,
        this_zval: *mut c_void,
    ) -> Self {
        Self {
            args,
            argc,
            retval,
            object_id,
            rust_data,
            this_zval,
            _marker: PhantomData,
        }
    }

    // ── Raw pointer access (for FFI-heavy handlers) ──

    /// Get a raw pointer to the zval at argument index `idx`.
    ///
    /// # Safety
    /// The returned pointer is valid only for the duration of the NativeCall.
    /// Caller must ensure `idx < self.argc`.
    pub unsafe fn raw_arg_ptr(&self, idx: u32) -> *mut c_void {
        let zval_size = super::ffi::oxphp_zval_size();
        (self.args as *mut u8).add(idx as usize * zval_size) as *mut c_void
    }

    /// Get a raw pointer to the return value zval.
    ///
    /// Used by handlers that need to pass retval to FFI functions
    /// which write directly into it (e.g., `oxphp_bridge_await_dispatch`).
    pub fn retval_ptr(&self) -> *mut c_void {
        self.retval
    }

    // ── Metadata ──

    /// Number of arguments passed.
    pub fn argc(&self) -> u32 {
        self.argc
    }

    /// Object ID (for class methods). None for free functions.
    pub fn object_id(&self) -> Option<u64> {
        self.object_id
    }

    /// Raw `$this` zval pointer for instance method calls. Null for static
    /// method calls, free functions, and contexts where the dispatch path
    /// did not provide it. Use with FFI helpers such as
    /// `oxphp_object_read_property` to access PHP-level properties.
    pub fn this_ptr(&self) -> *mut c_void {
        self.this_zval
    }

    /// Get typed immutable reference to Rust storage for current object.
    pub fn storage<T: std::any::Any + Send + Sync>(&self) -> Result<&T, PhpError> {
        let ptr = self.rust_data.ok_or_else(|| {
            PhpError::Custom("storage() called outside method context or no storage".into())
        })?;
        if ptr.is_null() {
            return Err(PhpError::Custom("object storage not initialized".into()));
        }
        Ok(unsafe { &*(ptr as *const T) })
    }

    /// Get typed mutable reference to Rust storage for current object.
    pub fn storage_mut<T: std::any::Any + Send + Sync>(&mut self) -> Result<&mut T, PhpError> {
        let ptr = self.rust_data.ok_or_else(|| {
            PhpError::Custom("storage_mut() called outside method context or no storage".into())
        })?;
        if ptr.is_null() {
            return Err(PhpError::Custom("object storage not initialized".into()));
        }
        Ok(unsafe { &mut *(ptr as *mut T) })
    }

    // ── Argument reading ──

    /// Type of argument at index.
    pub fn arg_type(&self, idx: u32) -> Result<ValType, PhpError> {
        self.check_idx(idx)?;
        Ok(ValType::from_u8(unsafe {
            ffi::oxphp_val_arg_type(self.args, idx)
        }))
    }

    /// Read int argument.
    pub fn arg_long(&self, idx: u32) -> Result<i64, PhpError> {
        self.check_idx(idx)?;
        self.check_type(idx, ValType::Long)?;
        Ok(unsafe { ffi::oxphp_arg_long(self.args, idx) })
    }

    /// Read a backed-int enum argument (e.g. `OxPHP\Shared\Ordering`) as its
    /// underlying `i64` value. The argument must be an object; the PHP
    /// type-hint at the method declaration enforces it's specifically the
    /// expected enum class. Returns 0 if the object is not a backed-int
    /// enum (e.g. unit enum or string-backed) — callers that need stricter
    /// checking should validate the value separately.
    pub fn arg_enum_long(&self, idx: u32) -> Result<i64, PhpError> {
        self.check_idx(idx)?;
        self.check_type(idx, ValType::Object)?;
        Ok(unsafe { ffi::oxphp_arg_enum_long(self.args, idx) })
    }

    /// Read float argument.
    pub fn arg_double(&self, idx: u32) -> Result<f64, PhpError> {
        self.check_idx(idx)?;
        self.check_type(idx, ValType::Double)?;
        Ok(unsafe { ffi::oxphp_arg_double(self.args, idx) })
    }

    /// Read bool argument.
    pub fn arg_bool(&self, idx: u32) -> Result<bool, PhpError> {
        self.check_idx(idx)?;
        let t = ValType::from_u8(unsafe { ffi::oxphp_val_arg_type(self.args, idx) });
        if !t.is_bool() {
            return Err(PhpError::TypeError {
                expected: "bool",
                got: type_name(t),
            });
        }
        Ok(unsafe { ffi::oxphp_arg_bool(self.args, idx) != 0 })
    }

    /// Read string argument. Zero-copy — returns slice into PHP memory.
    /// Valid only for the duration of the current call.
    pub fn arg_str(&self, idx: u32) -> Result<&'a str, PhpError> {
        self.check_idx(idx)?;
        self.check_type(idx, ValType::String)?;
        let mut len = 0usize;
        let ptr = unsafe { ffi::oxphp_arg_str(self.args, idx, &mut len) };
        if ptr.is_null() {
            return Err(PhpError::Custom("NULL string pointer".into()));
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        std::str::from_utf8(bytes)
            .map_err(|_| PhpError::Custom("invalid UTF-8 in string arg".into()))
    }

    /// Read string argument as raw bytes (for binary data).
    pub fn arg_bytes(&self, idx: u32) -> Result<&'a [u8], PhpError> {
        self.check_idx(idx)?;
        self.check_type(idx, ValType::String)?;
        let mut len = 0usize;
        let ptr = unsafe { ffi::oxphp_arg_str(self.args, idx, &mut len) };
        if ptr.is_null() {
            return Err(PhpError::Custom("NULL string pointer".into()));
        }
        Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    /// Check if argument is null.
    pub fn arg_is_null(&self, idx: u32) -> Result<bool, PhpError> {
        self.check_idx(idx)?;
        Ok(self.arg_type(idx)? == ValType::Null)
    }

    /// Count of elements in an array argument.
    pub fn arg_array_count(&self, idx: u32) -> Result<u32, PhpError> {
        self.check_idx(idx)?;
        self.check_type(idx, ValType::Array)?;
        Ok(unsafe { ffi::oxphp_arg_array_count(self.args, idx) })
    }

    /// Iterate over an array argument.
    pub fn arg_array_foreach<F>(&self, idx: u32, mut f: F) -> Result<(), PhpError>
    where
        F: FnMut(ArrayKey<'a>, Val<'a>),
    {
        self.check_idx(idx)?;
        self.check_type(idx, ValType::Array)?;
        let arr = unsafe { ffi::oxphp_arg_array(self.args, idx) };
        if arr.is_null() {
            return Err(PhpError::Custom("NULL array pointer".into()));
        }

        unsafe extern "C" fn trampoline<'a, F: FnMut(ArrayKey<'a>, Val<'a>)>(
            key: *const u8,
            key_len: usize,
            num_idx: i64,
            val: *mut c_void,
            user_data: *mut c_void,
        ) {
            let f = unsafe { &mut *(user_data as *mut F) };
            let k = if !key.is_null() && key_len > 0 {
                let bytes = unsafe { std::slice::from_raw_parts(key, key_len) };
                ArrayKey::Str(std::str::from_utf8(bytes).unwrap_or(""))
            } else {
                ArrayKey::Int(num_idx)
            };
            f(
                k,
                Val {
                    ptr: val,
                    _marker: PhantomData,
                },
            );
        }

        unsafe {
            ffi::oxphp_array_foreach(arr, trampoline::<F>, &mut f as *mut F as *mut c_void);
        }
        Ok(())
    }

    /// If arg `idx` is a Throwable, invoke `f` with `(class, message,
    /// stacktrace)` valid for the callback's duration. Message / stacktrace are
    /// `None` when absent, and are length-delimited and decoded lossily (a PHP
    /// string may hold non-UTF-8 or embedded-NUL bytes). No-op if the arg is not
    /// a Throwable (then `f` is never called).
    pub fn capture_exception_arg<F>(&self, idx: u32, mut f: F)
    where
        F: FnMut(&str, Option<&str>, Option<&str>),
    {
        // Guard the C-side raw index `((zval*)args)+idx` — unlike the fallible
        // arg_* accessors this is infallible, so an out-of-range index no-ops
        // rather than reading past the argument array.
        if idx >= self.argc() {
            return;
        }

        unsafe extern "C" fn trampoline<F: FnMut(&str, Option<&str>, Option<&str>)>(
            cls: *const c_char,
            cls_len: usize,
            msg: *const c_char,
            msg_len: usize,
            trace: *const c_char,
            trace_len: usize,
            user_data: *mut c_void,
        ) {
            let f = unsafe { &mut *(user_data as *mut F) };
            let class = unsafe { crate::bridge::decode::bytes_lossy(cls, cls_len) };
            let msg = unsafe { crate::bridge::decode::bytes_lossy(msg, msg_len) };
            let trace = unsafe { crate::bridge::decode::bytes_lossy(trace, trace_len) };
            f(
                class.as_deref().unwrap_or(""),
                msg.as_deref(),
                trace.as_deref(),
            );
        }

        unsafe {
            ffi::oxphp_arg_exception_capture(
                self.args,
                idx,
                trampoline::<F>,
                &mut f as *mut F as *mut c_void,
            );
        }
    }

    /// Iterate over an array argument yielding RAW string-key bytes.
    ///
    /// Unlike [`arg_array_foreach`](Self::arg_array_foreach), which coerces a
    /// string key to UTF-8 (`ArrayKey::Str(&str)`), this hands the callback the
    /// key's raw bytes so binary array keys round-trip faithfully. The callback
    /// receives `(Some(&[u8]), 0, val)` for string keys and `(None, num_idx,
    /// val)` for integer keys. Uses the same C entry points as
    /// `arg_array_foreach`.
    pub fn arg_array_foreach_raw<F>(&self, idx: u32, mut f: F) -> Result<(), PhpError>
    where
        F: FnMut(Option<&'a [u8]>, i64, Val<'a>),
    {
        self.check_idx(idx)?;
        self.check_type(idx, ValType::Array)?;
        let arr = unsafe { ffi::oxphp_arg_array(self.args, idx) };
        if arr.is_null() {
            return Err(PhpError::Custom("NULL array pointer".into()));
        }

        unsafe extern "C" fn trampoline<'a, F: FnMut(Option<&'a [u8]>, i64, Val<'a>)>(
            key: *const u8,
            key_len: usize,
            num_idx: i64,
            val: *mut c_void,
            user_data: *mut c_void,
        ) {
            let f = unsafe { &mut *(user_data as *mut F) };
            let k = if !key.is_null() && key_len > 0 {
                Some(unsafe { std::slice::from_raw_parts(key, key_len) })
            } else {
                None
            };
            f(
                k,
                num_idx,
                Val {
                    ptr: val,
                    _marker: PhantomData,
                },
            );
        }

        unsafe {
            ffi::oxphp_array_foreach(arr, trampoline::<F>, &mut f as *mut F as *mut c_void);
        }
        Ok(())
    }

    // ── Return value writing ──

    /// Return null.
    pub fn ret_null(&mut self) {
        unsafe { ffi::oxphp_ret_null(self.retval) };
    }

    /// Return bool.
    pub fn ret_bool(&mut self, val: bool) {
        unsafe { ffi::oxphp_ret_bool(self.retval, val as i32) };
    }

    /// Return int.
    pub fn ret_long(&mut self, val: i64) {
        unsafe { ffi::oxphp_ret_long(self.retval, val) };
    }

    /// Return float.
    pub fn ret_double(&mut self, val: f64) {
        unsafe { ffi::oxphp_ret_double(self.retval, val) };
    }

    /// Return string (copies into PHP memory).
    pub fn ret_str(&mut self, val: &str) {
        unsafe { ffi::oxphp_ret_str(self.retval, val.as_ptr(), val.len()) };
    }

    /// Return bytes (copies into PHP memory).
    pub fn ret_bytes(&mut self, val: &[u8]) {
        unsafe { ffi::oxphp_ret_str(self.retval, val.as_ptr(), val.len()) };
    }

    /// Return an array. The callback receives an `ArrayBuilder` for populating it.
    pub fn ret_array(&mut self, size_hint: u32, f: impl FnOnce(&mut ArrayBuilder)) {
        unsafe { ffi::oxphp_ret_array_init(self.retval, size_hint) };
        let mut builder = ArrayBuilder {
            arr: self.retval,
            _marker: PhantomData,
        };
        f(&mut builder);
    }

    // ── Property access on `$this` ──

    /// Read a declared `int` property of `$this`. Returns 0 if `$this` is
    /// null, the property does not exist, or it is unset / null in storage.
    /// Used by value-typed return classes (RecvResult / SendResult) to
    /// decode their `__status` discriminant.
    pub fn read_long_property(&self, name: &str) -> Result<i64, PhpError> {
        let this = self.this_zval;
        if this.is_null() {
            return Ok(0);
        }
        if name.as_bytes().contains(&0) {
            return Err(PhpError::Custom("property name contains NUL".into()));
        }
        // Stack-bounded null-terminated copy (property names are short).
        if name.len() >= 64 {
            return Err(PhpError::Custom("property name too long".into()));
        }
        let mut buf = [0u8; 64];
        buf[..name.len()].copy_from_slice(name.as_bytes());
        let prop = unsafe { ffi::oxphp_object_read_property(this, buf.as_ptr() as *const _) };
        if prop.is_null() || unsafe { ffi::oxphp_zval_is_null_or_unset(prop) } != 0 {
            return Ok(0);
        }
        Ok(unsafe { ffi::oxphp_val_long(prop) })
    }

    /// Copy a declared property of `$this` into the retval. If `$this` is
    /// null or the property is unset / null, the retval is set to PHP null.
    pub fn copy_property_to_retval(&mut self, name: &str) -> Result<(), PhpError> {
        let this = self.this_zval;
        if this.is_null() {
            self.ret_null();
            return Ok(());
        }
        if name.as_bytes().contains(&0) {
            return Err(PhpError::Custom("property name contains NUL".into()));
        }
        if name.len() >= 64 {
            return Err(PhpError::Custom("property name too long".into()));
        }
        let mut buf = [0u8; 64];
        buf[..name.len()].copy_from_slice(name.as_bytes());
        let prop = unsafe { ffi::oxphp_object_read_property(this, buf.as_ptr() as *const _) };
        if prop.is_null() || unsafe { ffi::oxphp_zval_is_null_or_unset(prop) } != 0 {
            self.ret_null();
            return Ok(());
        }
        unsafe { ffi::oxphp_zval_copy_to_retval(prop, self.retval) };
        Ok(())
    }

    /// Copy the argument at `idx` into the retval (ZVAL_COPY semantics).
    /// Used by `valueOr($default)` to return `$default` unchanged.
    pub fn copy_arg_to_retval(&mut self, idx: u32) -> Result<(), PhpError> {
        self.check_idx(idx)?;
        let src = unsafe { self.raw_arg_ptr(idx) };
        unsafe { ffi::oxphp_zval_copy_to_retval(src, self.retval) };
        Ok(())
    }

    /// Call a PHP function by name. Arguments are built via `ArgBuilder`.
    pub fn call_php(
        &self,
        func_name: &str,
        argc: u32,
        build_args: impl FnOnce(&mut ArgBuilder),
    ) -> Result<OwnedResult, PhpError> {
        // Null-terminate the function name on the stack (avoids CString heap alloc).
        if func_name.as_bytes().contains(&0) {
            return Err(PhpError::Custom("function name contains NUL".into()));
        }
        if func_name.len() >= 256 {
            return Err(PhpError::Custom("function name too long".into()));
        }
        let mut name_buf = [0u8; 256];
        name_buf[..func_name.len()].copy_from_slice(func_name.as_bytes());
        let c_name = name_buf.as_ptr() as *const std::os::raw::c_char;

        // Call — result is placed in an aligned, owned buffer.
        let mut result_slot = std::mem::MaybeUninit::<ZvalSlot>::uninit();
        let result_ptr = result_slot.as_mut_ptr() as *mut c_void;
        let rc = with_arg_buffer(argc, build_args, |args_ptr| unsafe {
            ffi::oxphp_call_php_native(c_name, args_ptr, argc, result_ptr)
        });

        if rc != 0 {
            unsafe { ffi::oxphp_zval_dtor(result_ptr) };
            return Err(PhpError::CallFailed(format!(
                "call_user_function failed for {func_name}"
            )));
        }

        // Safety: FFI wrote into result_slot. ZvalSlot is [u8; 16], so assume_init
        // is always sound. PHP zvals don't contain self-referential pointers,
        // so moving the bytes preserves validity of internal heap pointers.
        Ok(OwnedResult {
            slot: unsafe { result_slot.assume_init() },
        })
    }

    /// Invoke the callable held in argument `idx`, passing `argc` arguments
    /// built by `build_args`.
    ///
    /// Unlike [`call_php`](Self::call_php) the target is a *value*, not a name,
    /// so a Closure, a `"func"` / `"Cls::method"` string, an `[obj, 'method']`
    /// array and an invokable object all work, and no `call_user_func()` frame
    /// is interposed between the caller and the callback — which matters
    /// wherever the resulting stack is itself recorded, and keeps the call
    /// independent of the user's `disable_functions`.
    ///
    /// `Err` means the index is out of range; everything the PHP side can do is
    /// a [`CallableOutcome`].
    pub fn call_arg_callable(
        &self,
        idx: u32,
        argc: u32,
        build_args: impl FnOnce(&mut ArgBuilder),
    ) -> Result<CallableOutcome, PhpError> {
        self.check_idx(idx)?;
        let callable = unsafe { self.raw_arg_ptr(idx) };

        // The shim writes NULL into the slot before doing anything else, and
        // the host mock leaves it untouched (all-zero = IS_UNDEF); both are
        // safe to drop, so no path here can release an uninitialized zval.
        let mut result = OwnedResult::undef();
        let rc = with_arg_buffer(argc, build_args, |args_ptr| unsafe {
            ffi::oxphp_call_callable_native(callable, args_ptr, argc, result.as_mut_ptr())
        });

        match rc {
            0 => Ok(CallableOutcome::Returned(result)),
            -1 => Ok(CallableOutcome::NotCallable),
            -2 => Ok(CallableOutcome::Threw),
            // Not folded into `NotCallable`: a code this side does not know
            // about is a changed contract, and reading it as "the user passed
            // a bad value" would blame the caller for it.
            other => Err(PhpError::CallFailed(format!(
                "oxphp_call_callable_native returned {other}"
            ))),
        }
    }

    // ── Private helpers ──

    fn check_idx(&self, idx: u32) -> Result<(), PhpError> {
        if idx >= self.argc {
            Err(PhpError::ArgCount {
                expected: (idx + 1) as usize,
                got: self.argc as usize,
            })
        } else {
            Ok(())
        }
    }

    fn check_type(&self, idx: u32, expected: ValType) -> Result<(), PhpError> {
        let actual = ValType::from_u8(unsafe { ffi::oxphp_val_arg_type(self.args, idx) });
        if actual != expected {
            // Allow null for any type (nullable args)
            if actual == ValType::Null {
                return Ok(());
            }
            // Allow Long → Double coercion (PHP does this implicitly,
            // and the C-side oxphp_arg_double handles it correctly)
            if expected == ValType::Double && actual == ValType::Long {
                return Ok(());
            }
            Err(PhpError::TypeError {
                expected: type_name(expected),
                got: type_name(actual),
            })
        } else {
            Ok(())
        }
    }
}

/// Array element key.
#[derive(Debug, Clone)]
pub enum ArrayKey<'a> {
    Int(i64),
    Str(&'a str),
}

/// Read-only wrapper over a zval* element (from array iteration or call result).
pub struct Val<'a> {
    pub(crate) ptr: *mut c_void,
    pub(crate) _marker: PhantomData<&'a ()>,
}

impl<'a> Val<'a> {
    pub fn val_type(&self) -> ValType {
        ValType::from_u8(unsafe { ffi::oxphp_val_type(self.ptr) })
    }
    pub fn as_long(&self) -> i64 {
        unsafe { ffi::oxphp_val_long(self.ptr) }
    }
    pub fn as_double(&self) -> f64 {
        unsafe { ffi::oxphp_val_double(self.ptr) }
    }
    pub fn as_bool(&self) -> bool {
        unsafe { ffi::oxphp_val_bool(self.ptr) != 0 }
    }
    pub fn as_str(&self) -> Option<&'a str> {
        let mut len = 0;
        let ptr = unsafe { ffi::oxphp_val_str(self.ptr, &mut len) };
        if ptr.is_null() {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        std::str::from_utf8(bytes).ok()
    }
    pub fn as_bytes(&self) -> Option<&'a [u8]> {
        let mut len = 0;
        let ptr = unsafe { ffi::oxphp_val_str(self.ptr, &mut len) };
        if ptr.is_null() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    }
    pub fn is_null(&self) -> bool {
        self.val_type() == ValType::Null
    }
    pub fn array_count(&self) -> u32 {
        unsafe { ffi::oxphp_val_array_count(self.ptr) }
    }

    /// Raw zval pointer backing this value. Valid only for the duration
    /// of the current call. Used by FFI-heavy handlers that pass the
    /// zval to a bridge serializer (e.g. `oxphp_portable_serialize`).
    pub fn as_ptr(&self) -> *mut c_void {
        self.ptr
    }

    /// Iterate if this is an array.
    pub fn foreach<F: FnMut(ArrayKey<'a>, Val<'a>)>(&self, mut f: F) {
        if self.val_type() != ValType::Array {
            return;
        }
        unsafe extern "C" fn trampoline<'a, F: FnMut(ArrayKey<'a>, Val<'a>)>(
            key: *const u8,
            key_len: usize,
            num_idx: i64,
            val: *mut c_void,
            user_data: *mut c_void,
        ) {
            let f = unsafe { &mut *(user_data as *mut F) };
            let k = if !key.is_null() && key_len > 0 {
                let bytes = unsafe { std::slice::from_raw_parts(key, key_len) };
                ArrayKey::Str(std::str::from_utf8(bytes).unwrap_or(""))
            } else {
                ArrayKey::Int(num_idx)
            };
            f(
                k,
                Val {
                    ptr: val,
                    _marker: PhantomData,
                },
            );
        }
        unsafe {
            ffi::oxphp_array_foreach(self.ptr, trampoline::<F>, &mut f as *mut F as *mut c_void);
        }
    }
}

/// Builder for constructing a PHP array directly in zval memory.
pub struct ArrayBuilder<'a> {
    arr: *mut c_void,
    _marker: PhantomData<&'a ()>,
}

impl<'a> ArrayBuilder<'a> {
    // ── Keyed (associative) ──

    pub fn null(&mut self, key: &str) {
        unsafe { ffi::oxphp_arr_add_null(self.arr, key.as_ptr() as _, key.len()) };
    }
    pub fn bool(&mut self, key: &str, val: bool) {
        unsafe { ffi::oxphp_arr_add_bool(self.arr, key.as_ptr() as _, key.len(), val as i32) };
    }
    pub fn long(&mut self, key: &str, val: i64) {
        unsafe { ffi::oxphp_arr_add_long(self.arr, key.as_ptr() as _, key.len(), val) };
    }
    pub fn double(&mut self, key: &str, val: f64) {
        unsafe { ffi::oxphp_arr_add_double(self.arr, key.as_ptr() as _, key.len(), val) };
    }
    pub fn str(&mut self, key: &str, val: &str) {
        unsafe {
            ffi::oxphp_arr_add_str(
                self.arr,
                key.as_ptr() as _,
                key.len(),
                val.as_ptr(),
                val.len(),
            )
        };
    }
    pub fn bytes(&mut self, key: &str, val: &[u8]) {
        unsafe {
            ffi::oxphp_arr_add_str(
                self.arr,
                key.as_ptr() as _,
                key.len(),
                val.as_ptr(),
                val.len(),
            )
        };
    }
    pub fn array(&mut self, key: &str, size_hint: u32, f: impl FnOnce(&mut ArrayBuilder)) {
        let sub =
            unsafe { ffi::oxphp_arr_add_array(self.arr, key.as_ptr() as _, key.len(), size_hint) };
        let mut builder = ArrayBuilder {
            arr: sub,
            _marker: PhantomData,
        };
        f(&mut builder);
    }

    // ── Indexed (list / push) ──

    pub fn push_null(&mut self) {
        unsafe { ffi::oxphp_arr_push_null(self.arr) };
    }
    pub fn push_bool(&mut self, val: bool) {
        unsafe { ffi::oxphp_arr_push_bool(self.arr, val as i32) };
    }
    pub fn push_long(&mut self, val: i64) {
        unsafe { ffi::oxphp_arr_push_long(self.arr, val) };
    }
    pub fn push_double(&mut self, val: f64) {
        unsafe { ffi::oxphp_arr_push_double(self.arr, val) };
    }
    pub fn push_str(&mut self, val: &str) {
        unsafe { ffi::oxphp_arr_push_str(self.arr, val.as_ptr(), val.len()) };
    }
    pub fn push_bytes(&mut self, val: &[u8]) {
        unsafe { ffi::oxphp_arr_push_str(self.arr, val.as_ptr(), val.len()) };
    }
    pub fn push_array(&mut self, size_hint: u32, f: impl FnOnce(&mut ArrayBuilder)) {
        let sub = unsafe { ffi::oxphp_arr_push_array(self.arr, size_hint) };
        let mut builder = ArrayBuilder {
            arr: sub,
            _marker: PhantomData,
        };
        f(&mut builder);
    }
}

/// Builder for constructing arguments to pass to `call_php`.
pub struct ArgBuilder {
    args: *mut c_void,
    idx: u32,
}

impl ArgBuilder {
    pub fn null(&mut self) {
        unsafe { ffi::oxphp_ret_null(self.next()) };
    }
    pub fn bool(&mut self, v: bool) {
        unsafe { ffi::oxphp_ret_bool(self.next(), v as i32) };
    }
    pub fn long(&mut self, v: i64) {
        unsafe { ffi::oxphp_ret_long(self.next(), v) };
    }
    pub fn double(&mut self, v: f64) {
        unsafe { ffi::oxphp_ret_double(self.next(), v) };
    }
    pub fn str(&mut self, v: &str) {
        unsafe { ffi::oxphp_ret_str(self.next(), v.as_ptr(), v.len()) };
    }

    /// Forward an existing zval into the next argument slot by refcount
    /// (`ZVAL_COPY`), preserving object identity. Used to pass an existing
    /// argument (e.g. a Traversable / Generator) into a `call_php`
    /// invocation. A deep copy is wrong here — objects (Generators are
    /// uncloneable) would be lost — so this shares the value and bumps its
    /// refcount; it is released when the call's argument buffer is dropped.
    ///
    /// # Safety
    /// `src` must point to a valid, readable zval for the duration of
    /// this call.
    pub unsafe fn zval_copy(&mut self, src: *const c_void) {
        let dst = self.next();
        unsafe { ffi::oxphp_zval_copy_to_retval(src, dst) };
    }

    fn next(&mut self) -> *mut c_void {
        let ptr =
            unsafe { (self.args as *mut u8).add(self.idx as usize * ZVAL_SIZE) as *mut c_void };
        self.idx += 1;
        ptr
    }
}

/// Build `argc` argument zvals, hand the buffer to `invoke`, then release every
/// slot and return whatever `invoke` returned.
///
/// Both callers need the same bookkeeping: the engine copies each argument into
/// the callee frame (`ZVAL_COPY`) without taking ownership, so any refcounted
/// argument — a `b.str` zend_string, an object forwarded by `b.zval_copy` — is
/// still ours to release afterwards, on the failure path as much as on the
/// success one. Leaving that out leaks for the rest of the request.
fn with_arg_buffer<R>(
    argc: u32,
    build_args: impl FnOnce(&mut ArgBuilder),
    invoke: impl FnOnce(*mut c_void) -> R,
) -> R {
    // Stack-allocate for <= 8 args, heap (via global allocator / mimalloc) for more.
    // ZvalSlot has align(8) matching zval's alignment requirement. Zero-init
    // (all-zero bytes = IS_UNDEF) so any slot a builder leaves unfilled is
    // safe to pass through zval_ptr_dtor below.
    let mut stack_args: [ZvalSlot; 8] = [const { ZvalSlot([0u8; 16]) }; 8];
    let mut heap_args: Vec<ZvalSlot>;

    let args_ptr = if argc == 0 {
        std::ptr::null_mut()
    } else if argc <= 8 {
        stack_args.as_mut_ptr() as *mut c_void
    } else {
        // Vec uses the global allocator (mimalloc), not libc::calloc.
        heap_args = vec![ZvalSlot([0u8; 16]); argc as usize];
        heap_args.as_mut_ptr() as *mut c_void
    };

    if argc > 0 {
        let mut builder = ArgBuilder {
            args: args_ptr,
            idx: 0,
        };
        build_args(&mut builder);
    }

    let out = invoke(args_ptr);

    // Scalars / IS_UNDEF dtor as no-ops.
    if !args_ptr.is_null() {
        for i in 0..argc as usize {
            let slot = unsafe { (args_ptr as *mut u8).add(i * ZVAL_SIZE) as *mut c_void };
            unsafe { ffi::oxphp_zval_dtor(slot) };
        }
    }

    // heap_args dropped here automatically (Vec destructor via mimalloc)
    out
}

/// What came back from invoking a PHP callable value.
///
/// The split that matters to a caller is whether a PHP exception is already
/// pending. `Threw` means one is, and it must be left alone to propagate;
/// `NotCallable` means none is, and reporting the bad argument is the caller's
/// job (usually a `TypeError` naming its own argument).
pub enum CallableOutcome {
    /// Ran to completion; carries its return value.
    Returned(OwnedResult),
    /// A PHP exception is pending and untouched. Usually the callable threw —
    /// but resolving a callable can raise one too (a throwing autoloader
    /// behind a `"Cls::method"` string, an error handler that throws on the
    /// deprecation for `"self::m"`), and then nothing ran. Both propagate the
    /// same way, so they are not worth telling apart here.
    Threw,
    /// The value was not callable: nothing ran and no exception is pending.
    NotCallable,
}

/// Sizeof(zval) — 16 on all 64-bit PHP 8.x builds. Verified at runtime via
/// `oxphp_zval_size()` in debug builds (see [`debug_assert_zval_size`]).
pub(crate) const ZVAL_SIZE: usize = 16;

/// Runtime sanity check that the linked PHP build still uses a 16-byte zval.
/// No-op in release; loud panic in debug if the assumption breaks (a future
/// PHP layout change would otherwise silently corrupt every fixed-buffer
/// zval slot in the bridge — `ZvalSlot([u8; 16])`, the `[0u8; ZVAL_SIZE]`
/// allocations in handlers, the `ArgBuilder` stride, etc.).
#[inline]
#[allow(dead_code)]
pub(crate) fn debug_assert_zval_size() {
    debug_assert_eq!(
        unsafe { super::ffi::oxphp_zval_size() },
        ZVAL_SIZE,
        "PHP zval layout changed: sizeof(zval) is no longer {ZVAL_SIZE}. \
         Update ZVAL_SIZE, ZvalSlot, and every fixed-buffer site in the bridge."
    );
}

/// A zval slot whose contents this value owns — calls `zval_ptr_dtor` on drop
/// to prevent refcounted value leaks (strings, arrays, objects). Returned by
/// `call_php`, and used directly wherever an FFI call materializes a value into
/// a temporary slot that must be released once it has been handed on.
/// Pointer is computed from `slot` field address on each access rather than
/// cached as a raw pointer — avoids use-after-move UB.
pub struct OwnedResult {
    slot: ZvalSlot,
}

impl OwnedResult {
    /// An empty owned slot. All-zero bytes are `IS_UNDEF`, so dropping one that
    /// was never written is a no-op — which makes it safe to hand
    /// [`as_mut_ptr`](Self::as_mut_ptr) to an FFI call that only writes on some
    /// of its return paths.
    // Not `pub`: handing plugin authors a raw pointer into a live zval slot is
    // the footgun this type exists to remove. Crate-internal, hence the lint
    // waiver for feature combinations that do not build the callers.
    #[allow(dead_code)]
    pub(crate) fn undef() -> Self {
        debug_assert_zval_size();
        OwnedResult {
            slot: ZvalSlot([0u8; ZVAL_SIZE]),
        }
    }

    /// Writable pointer to the slot, for an FFI call that populates a zval.
    /// Whatever it writes becomes this value's to release on drop.
    #[allow(dead_code)]
    pub(crate) fn as_mut_ptr(&mut self) -> *mut c_void {
        &mut self.slot as *mut ZvalSlot as *mut c_void
    }

    /// Borrow as Val for reading.
    /// Pointer is computed from field address on each call — safe after moves.
    pub fn val(&self) -> Val<'_> {
        Val {
            ptr: &self.slot as *const ZvalSlot as *mut c_void,
            _marker: PhantomData,
        }
    }

    /// Move the 16-byte zval payload into `dst` (a writable zval slot) and
    /// suppress the destructor on self — ownership of any refcounted PHP
    /// value (string / array / object) transfers to whatever now owns `dst`.
    ///
    /// # Safety
    /// `dst` must point to a writable zval-shaped slot (16 bytes, 8-byte
    /// aligned). Any existing contents of `dst` are overwritten without
    /// being dropped — call `oxphp_zval_dtor` on `dst` first if it might
    /// hold a refcounted value.
    pub fn write_into(self, dst: *mut c_void) {
        unsafe {
            std::ptr::copy_nonoverlapping(self.slot.0.as_ptr(), dst as *mut u8, self.slot.0.len());
        }
        std::mem::forget(self);
    }
}

impl Drop for OwnedResult {
    fn drop(&mut self) {
        // Release any refcounted PHP value (string, array, object).
        // Pointer computed from field address — safe after moves.
        unsafe { ffi::oxphp_zval_dtor(&mut self.slot as *mut ZvalSlot as *mut c_void) };
    }
}

/// Human-readable name for a ValType.
pub fn type_name(t: ValType) -> &'static str {
    match t {
        ValType::Null => "null",
        ValType::False | ValType::True => "bool",
        ValType::Long => "int",
        ValType::Double => "float",
        ValType::String => "string",
        ValType::Array => "array",
        ValType::Object => "object",
        ValType::Resource => "resource",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_name() {
        assert_eq!(type_name(ValType::Null), "null");
        assert_eq!(type_name(ValType::True), "bool");
        assert_eq!(type_name(ValType::False), "bool");
        assert_eq!(type_name(ValType::Long), "int");
        assert_eq!(type_name(ValType::Double), "float");
        assert_eq!(type_name(ValType::String), "string");
        assert_eq!(type_name(ValType::Array), "array");
        assert_eq!(type_name(ValType::Object), "object");
        assert_eq!(type_name(ValType::Resource), "resource");
    }

    #[test]
    fn test_native_call_check_idx() {
        let mut retval = ZvalSlot([0u8; 16]);
        let call = unsafe {
            NativeCall::new(
                std::ptr::null_mut(),
                2,
                &mut retval as *mut ZvalSlot as *mut c_void,
                None,
                None,
            )
        };
        assert!(call.check_idx(0).is_ok());
        assert!(call.check_idx(1).is_ok());
        assert!(call.check_idx(2).is_err());
        assert_eq!(call.argc(), 2);
        assert_eq!(call.object_id(), None);
    }

    #[test]
    fn test_native_call_with_object_id() {
        let mut retval = ZvalSlot([0u8; 16]);
        let call = unsafe {
            NativeCall::new(
                std::ptr::null_mut(),
                0,
                &mut retval as *mut ZvalSlot as *mut c_void,
                Some(42),
                None,
            )
        };
        assert_eq!(call.object_id(), Some(42));
    }

    #[test]
    fn test_zval_slot_alignment() {
        assert_eq!(std::mem::align_of::<ZvalSlot>(), 8);
        assert_eq!(std::mem::size_of::<ZvalSlot>(), 16);
    }

    #[test]
    fn test_owned_result_drops_without_panic() {
        let result = OwnedResult {
            slot: ZvalSlot([0u8; 16]),
        };
        // Verify val() works — ptr is computed from field address, not cached
        let _v = result.val();
        drop(result); // Should call zval_dtor (no-op in mock)
    }

    #[test]
    fn test_storage_returns_error_for_free_function() {
        let call =
            unsafe { NativeCall::new(std::ptr::null_mut(), 0, std::ptr::null_mut(), None, None) };
        let result = call.storage::<u32>();
        assert!(result.is_err());
    }

    #[test]
    fn test_storage_with_rust_data() {
        let value: u32 = 42;
        let ptr = Box::into_raw(Box::new(value)) as *mut std::ffi::c_void;
        let call = unsafe {
            NativeCall::new(
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                Some(1),
                Some(ptr),
            )
        };
        let result = call.storage::<u32>();
        assert!(result.is_ok());
        assert_eq!(*result.unwrap(), 42);
        unsafe {
            drop(Box::from_raw(ptr as *mut u32));
        }
    }

    #[test]
    fn test_storage_mut_with_rust_data() {
        let value: u32 = 42;
        let ptr = Box::into_raw(Box::new(value)) as *mut std::ffi::c_void;
        let mut call = unsafe {
            NativeCall::new(
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                Some(1),
                Some(ptr),
            )
        };
        {
            let s = call.storage_mut::<u32>().unwrap();
            *s = 100;
        }
        assert_eq!(*call.storage::<u32>().unwrap(), 100);
        unsafe {
            drop(Box::from_raw(ptr as *mut u32));
        }
    }

    #[test]
    fn test_raw_arg_ptr() {
        let mut args = [0u8; 64];
        let mut retval = [0u8; 16];
        let call = unsafe {
            NativeCall::new(
                args.as_mut_ptr() as *mut c_void,
                2,
                retval.as_mut_ptr() as *mut c_void,
                None,
                None,
            )
        };
        let ptr = unsafe { call.raw_arg_ptr(0) };
        assert_eq!(ptr, args.as_ptr() as *mut c_void);
    }

    #[test]
    fn test_retval_ptr() {
        let mut args = [0u8; 64];
        let mut retval = [0u8; 16];
        let call = unsafe {
            NativeCall::new(
                args.as_mut_ptr() as *mut c_void,
                1,
                retval.as_mut_ptr() as *mut c_void,
                None,
                None,
            )
        };
        assert_eq!(call.retval_ptr(), retval.as_ptr() as *mut c_void);
    }
}
