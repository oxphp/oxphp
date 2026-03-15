#[cfg(not(feature = "php"))]
mod async_tests {
    use oxphp::async_types::*;
    use std::ptr;

    #[test]
    fn test_async_task_send_trait() {
        fn assert_send<T: Send>() {}
        assert_send::<AsyncTask>();
        assert_send::<AsyncResult>();
        assert_send::<FrozenZval>();
        assert_send::<BorrowedZval>();
        assert_send::<PromiseCleanup>();
    }

    #[test]
    fn test_promise_cleanup_lifecycle() {
        let mut cleanup = PromiseCleanup::new();
        assert!(cleanup.frozen.is_empty());
        assert!(cleanup.borrowed.is_empty());

        cleanup.frozen.push(FrozenZval {
            zval_ptr: ptr::null_mut(),
            orig_refcount: 1,
            orig_gc_flags: 0,
            orig_type_flags: 0,
        });
        assert_eq!(cleanup.frozen.len(), 1);

        cleanup.borrowed.push(BorrowedZval {
            proxy_zval_ptr: ptr::null_mut(),
            original_zval_data: [0xAB; 16],
        });
        assert_eq!(cleanup.borrowed.len(), 1);
    }

    #[test]
    fn test_promise_cleanup_default() {
        let cleanup = PromiseCleanup::default();
        assert!(cleanup.frozen.is_empty());
        assert!(cleanup.borrowed.is_empty());
    }
}
