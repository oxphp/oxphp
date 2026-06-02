//! Rust-side dispatch callbacks installed into the C bridge.
//! These are called from observer begin/end handlers on PHP worker threads.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::sync::{Arc, OnceLock};

use super::registry::{DecoratorRegistry, ResolvedDecorator};
use super::types::{AttributeTargets, DecoratorAction, DecoratorCallContext, DecoratorCallResult};

/// Global registry — set once at startup, read from worker threads.
static REGISTRY: OnceLock<Arc<DecoratorRegistry>> = OnceLock::new();

fn get_registry() -> &'static DecoratorRegistry {
    REGISTRY
        .get()
        .expect("decorator registry not initialized")
        .as_ref()
}

/// Install all bridge callbacks and store the registry globally.
///
/// Must be called exactly once; subsequent calls panic.
pub fn install_bridge_callbacks(registry: Arc<DecoratorRegistry>) {
    if REGISTRY.set(registry).is_err() {
        panic!("decorator registry already initialized");
    }
    #[cfg(feature = "php")]
    unsafe {
        crate::bridge::ffi::oxphp_bridge_set_decorator_resolve(Some(resolve_callback));
        crate::bridge::ffi::oxphp_bridge_set_decorator_begin(Some(begin_callback));
        crate::bridge::ffi::oxphp_bridge_set_decorator_end(Some(end_callback));
        crate::bridge::ffi::oxphp_bridge_set_php_decorator_count(Some(
            php_decorator_count_callback,
        ));
        crate::bridge::ffi::oxphp_bridge_set_php_decorator_class(Some(
            php_decorator_class_callback,
        ));
        crate::bridge::ffi::oxphp_bridge_set_php_decorator_cache_key(Some(
            php_decorator_cache_key_callback,
        ));
        crate::bridge::ffi::oxphp_bridge_set_decorator_register_php(Some(
            register_php_decorator_callback,
        ));
    }
}

/// Collect a C array of `count` C-string pointers into owned `String`s,
/// skipping any that aren't valid UTF-8. Null base pointer (the C side
/// passes NULL when the count is 0) yields an empty vec.
///
/// # Safety
/// `names` must point to `count` valid `*const c_char` entries, each a
/// valid null-terminated C string, or be null when `count` is 0.
unsafe fn collect_names(names: *const *const c_char, count: u32) -> Vec<String> {
    if names.is_null() || count == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let ptr = *names.add(i);
        if let Ok(s) = CStr::from_ptr(ptr).to_str() {
            out.push(s.to_string());
        }
    }
    out
}

/// Called by observer init — resolve which decorators apply to this function.
/// Returns 1 if decorators found, 0 otherwise.
///
/// Function/method and class attribute names arrive in separate arrays so the
/// registry can count occurrences independently per scope. `attr_ctx` is the
/// C-side attribute resolver context (an `ox_attr_resolver_ctx_t*`), forwarded
/// to the registry so each matched decorator can read its attribute's
/// constructor arguments.
#[allow(dead_code)]
unsafe extern "C" fn resolve_callback(
    fn_id: usize,
    fn_attr_names: *const *const c_char,
    fn_attr_count: u32,
    class_attr_names: *const *const c_char,
    class_attr_count: u32,
    attr_ctx: *mut std::os::raw::c_void,
) -> c_int {
    let registry = get_registry();
    let fn_names = collect_names(fn_attr_names, fn_attr_count);
    let class_names = collect_names(class_attr_names, class_attr_count);
    if registry.resolve(fn_id, &fn_names, &class_names, attr_ctx) {
        1
    } else {
        0
    }
}

/// Called by observer begin — dispatch on_begin to Rust decorators (in attribute order).
/// Returns 0=Continue, 1=Reject.
#[allow(dead_code)]
unsafe extern "C" fn begin_callback(
    fn_id: usize,
    target: *const c_char,
    class_name: *const c_char,
    object_id: u64,
    timestamp_ns: u64,
) -> c_int {
    let registry = get_registry();
    let resolved = match registry.get_resolved(fn_id) {
        Some(r) => r,
        None => return 0,
    };

    let target_str: Arc<str> = Arc::from(CStr::from_ptr(target).to_str().unwrap_or(""));
    let class_str = if class_name.is_null() {
        None
    } else {
        Some(Arc::<str>::from(
            CStr::from_ptr(class_name).to_str().unwrap_or(""),
        ))
    };

    let ctx = DecoratorCallContext {
        target: Arc::clone(&target_str),
        class: class_str.clone(),
        method: if class_str.is_some() {
            Some(Arc::clone(&target_str))
        } else {
            None
        },
        function: if class_str.is_none() {
            Some(Arc::clone(&target_str))
        } else {
            None
        },
        object_id,
        request_id: String::new(),
        trace_id: String::new(),
        timestamp_ns,
    };

    for dec in resolved.iter() {
        if let ResolvedDecorator::Rust(ref decorator) = dec {
            match decorator.on_begin(&ctx) {
                DecoratorAction::Continue => {}
                DecoratorAction::Reject(reason) => {
                    let bytes = reason.as_bytes();
                    crate::bridge::ffi::oxphp_bridge_set_decorator_reject_reason(
                        bytes.as_ptr() as *const c_char,
                        bytes.len(),
                    );
                    return 1;
                }
            }
        }
    }
    0
}

/// Called by observer end — dispatch on_end to Rust decorators (in reverse order).
#[allow(dead_code)]
unsafe extern "C" fn end_callback(
    fn_id: usize,
    elapsed_ns: u64,
    success: c_int,
    exception_class: *const c_char,
) {
    let registry = get_registry();
    let resolved = match registry.get_resolved(fn_id) {
        Some(r) => r,
        None => return,
    };

    let exc = if exception_class.is_null() {
        None
    } else {
        CStr::from_ptr(exception_class)
            .to_str()
            .ok()
            .map(String::from)
    };

    let result = DecoratorCallResult {
        success: success != 0,
        elapsed_ns,
        exception_class: exc,
    };

    let ctx = DecoratorCallContext {
        target: Arc::from(""),
        class: None,
        method: None,
        function: None,
        object_id: 0,
        request_id: String::new(),
        trace_id: String::new(),
        timestamp_ns: 0,
    };

    // Reverse order — stack semantics
    for dec in resolved.iter().rev() {
        if let ResolvedDecorator::Rust(ref decorator) = dec {
            decorator.on_end(&ctx, &result);
        }
    }
}

/// Returns the number of PHP decorators for fn_id.
#[allow(dead_code)]
unsafe extern "C" fn php_decorator_count_callback(fn_id: usize) -> u32 {
    get_registry().php_decorator_count(fn_id) as u32
}

/// Returns the class name of the PHP decorator at php_index.
/// Writes to a __thread buffer in the bridge and returns a pointer.
#[allow(dead_code)]
unsafe extern "C" fn php_decorator_class_callback(fn_id: usize, php_index: u32) -> *const c_char {
    match get_registry().php_decorator_at(fn_id, php_index as usize) {
        Some((name, _)) => {
            crate::bridge::ffi::oxphp_bridge_set_decorator_class_buf(
                name.as_ptr() as *const c_char,
                name.len(),
            );
            crate::bridge::ffi::oxphp_bridge_get_decorator_class_buf()
        }
        None => std::ptr::null(),
    }
}

/// Returns the cache key for the PHP decorator at php_index.
#[allow(dead_code)]
unsafe extern "C" fn php_decorator_cache_key_callback(fn_id: usize, php_index: u32) -> u64 {
    match get_registry().php_decorator_at(fn_id, php_index as usize) {
        Some((_, key)) => key,
        None => u64::MAX,
    }
}

/// Register a PHP-side decorator from the bridge (called by oxphp_register_decorator).
///
/// # Safety
/// `class_name` must be a valid non-null null-terminated C string.
pub unsafe extern "C" fn register_php_decorator_callback(class_name: *const c_char, targets: u32) {
    let registry = get_registry();
    if let Ok(name) = CStr::from_ptr(class_name).to_str() {
        registry.register_php(
            name.to_string(),
            AttributeTargets::from_bits_truncate(targets),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decorator::registry::DecoratorRegistry;
    use crate::decorator::types::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingDecorator {
        begin_count: AtomicU32,
        end_count: AtomicU32,
    }

    impl Decorator for CountingDecorator {
        fn attribute_name(&self) -> &str {
            "Test\\Counter"
        }
        fn targets(&self) -> AttributeTargets {
            AttributeTargets::ALL
        }
        fn on_begin(&self, _ctx: &DecoratorCallContext) -> DecoratorAction {
            self.begin_count.fetch_add(1, Ordering::Relaxed);
            DecoratorAction::Continue
        }
        fn on_end(&self, _ctx: &DecoratorCallContext, _result: &DecoratorCallResult) {
            self.end_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn test_registry_dispatch_round_trip() {
        let registry = DecoratorRegistry::new();
        let dec = Arc::new(CountingDecorator {
            begin_count: AtomicU32::new(0),
            end_count: AtomicU32::new(0),
        });
        let dec_ref = Arc::clone(&dec);
        registry.register_rust(dec);

        let attrs = vec!["Test\\Counter".to_string()];
        assert!(registry.resolve(0x1, &attrs, &[], std::ptr::null_mut()));

        let resolved = registry.get_resolved(0x1).unwrap();
        let ctx = DecoratorCallContext {
            target: Arc::from("test_fn"),
            class: None,
            method: None,
            function: Some(Arc::from("test_fn")),
            object_id: 0,
            request_id: String::new(),
            trace_id: String::new(),
            timestamp_ns: 0,
        };

        // Simulate begin dispatch
        for d in resolved.iter() {
            if let ResolvedDecorator::Rust(ref decorator) = d {
                assert_eq!(decorator.on_begin(&ctx), DecoratorAction::Continue);
            }
        }
        assert_eq!(dec_ref.begin_count.load(Ordering::Relaxed), 1);

        // Simulate end dispatch (reverse order)
        let result = DecoratorCallResult {
            success: true,
            elapsed_ns: 100,
            exception_class: None,
        };
        for d in resolved.iter().rev() {
            if let ResolvedDecorator::Rust(ref decorator) = d {
                decorator.on_end(&ctx, &result);
            }
        }
        assert_eq!(dec_ref.end_count.load(Ordering::Relaxed), 1);
    }
}
