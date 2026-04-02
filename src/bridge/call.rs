use super::ffi;
use super::types::ValType;
use crate::plugin::PhpError;
use std::marker::PhantomData;
use std::os::raw::c_void;

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
    _marker: PhantomData<&'a ()>,
}

impl<'a> NativeCall<'a> {
    /// Create from raw pointers (called from dispatch callback).
    ///
    /// # Safety
    /// `args` and `retval` must be valid zval pointers for the duration of `'a`.
    /// `rust_data`, if `Some`, must point to a valid object of the expected type
    /// for the duration of `'a`.
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
            _marker: PhantomData,
        }
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

    /// Get typed immutable reference to Rust storage for current object.
    pub fn storage<T: std::any::Any + Send + Sync>(&self) -> Result<&T, PhpError> {
        let ptr = self.rust_data.ok_or_else(|| {
            PhpError::Custom(
                "storage() called outside method context or no storage".into(),
            )
        })?;
        if ptr.is_null() {
            return Err(PhpError::Custom("object storage not initialized".into()));
        }
        Ok(unsafe { &*(ptr as *const T) })
    }

    /// Get typed mutable reference to Rust storage for current object.
    pub fn storage_mut<T: std::any::Any + Send + Sync>(&mut self) -> Result<&mut T, PhpError> {
        let ptr = self.rust_data.ok_or_else(|| {
            PhpError::Custom(
                "storage_mut() called outside method context or no storage".into(),
            )
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

        // Stack-allocate for <= 8 args, heap (via global allocator / mimalloc) for more.
        // ZvalSlot has align(8) matching zval's alignment requirement.
        let mut stack_args: [std::mem::MaybeUninit<ZvalSlot>; 8] =
            [const { std::mem::MaybeUninit::uninit() }; 8];
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

        // Build arguments
        if argc > 0 {
            let mut builder = ArgBuilder {
                args: args_ptr,
                idx: 0,
            };
            build_args(&mut builder);
        }

        // Call — result is placed in an aligned, owned buffer.
        let mut result_slot = std::mem::MaybeUninit::<ZvalSlot>::uninit();
        let result_ptr = result_slot.as_mut_ptr() as *mut c_void;
        let rc = unsafe { ffi::oxphp_call_php_native(c_name, args_ptr, argc, result_ptr) };

        // heap_args dropped here automatically (Vec destructor via mimalloc)

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

    fn next(&mut self) -> *mut c_void {
        let ptr =
            unsafe { (self.args as *mut u8).add(self.idx as usize * ZVAL_SIZE) as *mut c_void };
        self.idx += 1;
        ptr
    }
}

/// Sizeof(zval) — 16 on all 64-bit PHP 8.x builds. Verified at runtime via
/// `oxphp_zval_size()` in debug builds (see `debug_assert_zval_size`).
const ZVAL_SIZE: usize = 16;

/// Owned result from `call_php` — calls `zval_ptr_dtor` on drop to prevent
/// refcounted value leaks (strings, arrays, objects).
/// Pointer is computed from `slot` field address on each access rather than
/// cached as a raw pointer — avoids use-after-move UB.
pub struct OwnedResult {
    slot: ZvalSlot,
}

impl OwnedResult {
    /// Borrow as Val for reading.
    /// Pointer is computed from field address on each call — safe after moves.
    pub fn val(&self) -> Val<'_> {
        Val {
            ptr: &self.slot as *const ZvalSlot as *mut c_void,
            _marker: PhantomData,
        }
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
        let call = unsafe {
            NativeCall::new(std::ptr::null_mut(), 0, std::ptr::null_mut(), None, None)
        };
        let result = call.storage::<u32>();
        assert!(result.is_err());
    }

    #[test]
    fn test_storage_with_rust_data() {
        let value: u32 = 42;
        let ptr = Box::into_raw(Box::new(value)) as *mut std::ffi::c_void;
        let call = unsafe {
            NativeCall::new(std::ptr::null_mut(), 0, std::ptr::null_mut(), Some(1), Some(ptr))
        };
        let result = call.storage::<u32>();
        assert!(result.is_ok());
        assert_eq!(*result.unwrap(), 42);
        unsafe { drop(Box::from_raw(ptr as *mut u32)); }
    }

    #[test]
    fn test_storage_mut_with_rust_data() {
        let value: u32 = 42;
        let ptr = Box::into_raw(Box::new(value)) as *mut std::ffi::c_void;
        let mut call = unsafe {
            NativeCall::new(std::ptr::null_mut(), 0, std::ptr::null_mut(), Some(1), Some(ptr))
        };
        {
            let s = call.storage_mut::<u32>().unwrap();
            *s = 100;
        }
        assert_eq!(*call.storage::<u32>().unwrap(), 100);
        unsafe { drop(Box::from_raw(ptr as *mut u32)); }
    }
}
