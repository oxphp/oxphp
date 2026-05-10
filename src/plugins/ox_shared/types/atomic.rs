//! Shared\Atomic — generic atomic int64 primitive (load, store, swap, CAS,
//! fetch-arithmetic, fetch-bitwise) with explicit memory ordering control.

use std::sync::atomic::{AtomicI64, Ordering};

pub struct AtomicInner {
    value: AtomicI64,
}

impl AtomicInner {
    pub fn new(initial: i64) -> Self {
        Self {
            value: AtomicI64::new(initial),
        }
    }

    pub fn load(&self, order: Ordering) -> i64 {
        self.value.load(order)
    }

    pub fn store(&self, v: i64, order: Ordering) {
        self.value.store(v, order);
    }

    pub fn swap(&self, v: i64, order: Ordering) -> i64 {
        self.value.swap(v, order)
    }

    pub fn compare_and_set(
        &self,
        expect: i64,
        new: i64,
        success: Ordering,
        failure: Ordering,
    ) -> bool {
        self.value
            .compare_exchange(expect, new, success, failure)
            .is_ok()
    }

    pub fn fetch_add(&self, delta: i64, order: Ordering) -> i64 {
        self.value.fetch_add(delta, order)
    }

    pub fn fetch_sub(&self, delta: i64, order: Ordering) -> i64 {
        self.value.fetch_sub(delta, order)
    }

    pub fn fetch_and(&self, mask: i64, order: Ordering) -> i64 {
        self.value.fetch_and(mask, order)
    }

    pub fn fetch_or(&self, mask: i64, order: Ordering) -> i64 {
        self.value.fetch_or(mask, order)
    }

    pub fn fetch_xor(&self, mask: i64, order: Ordering) -> i64 {
        self.value.fetch_xor(mask, order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_store_swap_baseline() {
        let a = AtomicInner::new(0);
        a.store(42, Ordering::SeqCst);
        assert_eq!(a.load(Ordering::SeqCst), 42);
        assert_eq!(a.swap(7, Ordering::SeqCst), 42);
        assert_eq!(a.load(Ordering::Acquire), 7);
    }

    #[test]
    fn cas_success_and_failure_paths() {
        let a = AtomicInner::new(10);
        assert!(a.compare_and_set(10, 20, Ordering::SeqCst, Ordering::SeqCst));
        assert!(!a.compare_and_set(10, 30, Ordering::SeqCst, Ordering::Acquire));
        assert_eq!(a.load(Ordering::SeqCst), 20);
    }

    #[test]
    fn fetch_add_returns_prev() {
        let a = AtomicInner::new(5);
        assert_eq!(a.fetch_add(3, Ordering::SeqCst), 5);
        assert_eq!(a.load(Ordering::SeqCst), 8);
    }

    #[test]
    fn fetch_sub_overflow_wraps() {
        let a = AtomicInner::new(i64::MIN);
        let prev = a.fetch_sub(1, Ordering::SeqCst);
        assert_eq!(prev, i64::MIN);
        assert_eq!(a.load(Ordering::SeqCst), i64::MAX);
    }

    #[test]
    fn fetch_bitwise_known_masks() {
        let a = AtomicInner::new(0b1010);
        assert_eq!(a.fetch_and(0b1100, Ordering::SeqCst), 0b1010);
        assert_eq!(a.load(Ordering::SeqCst), 0b1000);
        assert_eq!(a.fetch_or(0b0011, Ordering::SeqCst), 0b1000);
        assert_eq!(a.load(Ordering::SeqCst), 0b1011);
        assert_eq!(a.fetch_xor(0b1111, Ordering::SeqCst), 0b1011);
        assert_eq!(a.load(Ordering::SeqCst), 0b0100);
    }
}
