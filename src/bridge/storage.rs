use std::ffi::c_void;
use std::sync::OnceLock;

/// Metadata for a plugin-registered PHP class with Rust storage.
pub struct ClassMeta {
    pub fqn: String,
    pub factory: Box<dyn Fn() -> *mut c_void + Send + Sync>,
    pub drop_fn: Box<dyn Fn(*mut c_void) + Send + Sync>,
    pub clone_fn: Option<Box<dyn Fn(*mut c_void) -> *mut c_void + Send + Sync>>,
}

/// Global class metadata table. Set once before MINIT, read during runtime.
pub static CLASS_META: OnceLock<Vec<ClassMeta>> = OnceLock::new();

/// FFI callback: create storage for a new object instance.
pub unsafe extern "C" fn storage_create_callback(class_index: u32) -> *mut c_void {
    let meta = CLASS_META.get().expect("CLASS_META not initialized");
    if let Some(m) = meta.get(class_index as usize) {
        (m.factory)()
    } else {
        std::ptr::null_mut()
    }
}

/// FFI callback: drop storage when object is freed.
pub unsafe extern "C" fn storage_drop_callback(class_index: u32, rust_data: *mut c_void) {
    if rust_data.is_null() {
        return;
    }
    let meta = CLASS_META.get().expect("CLASS_META not initialized");
    if let Some(m) = meta.get(class_index as usize) {
        (m.drop_fn)(rust_data);
    }
}

/// FFI callback: clone storage when object is cloned.
pub unsafe extern "C" fn storage_clone_callback(class_index: u32, rust_data: *mut c_void) -> *mut c_void {
    let meta = CLASS_META.get().expect("CLASS_META not initialized");
    if let Some(m) = meta.get(class_index as usize) {
        match &m.clone_fn {
            Some(clone) => clone(rust_data),
            None => (m.factory)(), // fallback: fresh instance
        }
    } else {
        std::ptr::null_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_class_meta_create_and_drop() {
        let meta = ClassMeta {
            fqn: "Test\\MyClass".into(),
            factory: Box::new(|| Box::into_raw(Box::new(42u32)) as *mut c_void),
            drop_fn: Box::new(|ptr| unsafe { drop(Box::from_raw(ptr as *mut u32)) }),
            clone_fn: None,
        };
        let ptr = (meta.factory)();
        assert!(!ptr.is_null());
        let val = unsafe { *(ptr as *const u32) };
        assert_eq!(val, 42);
        (meta.drop_fn)(ptr);
    }

    #[test]
    fn test_class_meta_clone() {
        let meta = ClassMeta {
            fqn: "Test\\Cloneable".into(),
            factory: Box::new(|| Box::into_raw(Box::new(0u32)) as *mut c_void),
            drop_fn: Box::new(|ptr| unsafe { drop(Box::from_raw(ptr as *mut u32)) }),
            clone_fn: Some(Box::new(|ptr| {
                let val = unsafe { *(ptr as *const u32) };
                Box::into_raw(Box::new(val)) as *mut c_void
            })),
        };
        let ptr = (meta.factory)();
        unsafe { *(ptr as *mut u32) = 99; }
        let clone_ptr = (meta.clone_fn.as_ref().unwrap())(ptr);
        assert_eq!(unsafe { *(clone_ptr as *const u32) }, 99);
        (meta.drop_fn)(ptr);
        (meta.drop_fn)(clone_ptr);
    }
}
