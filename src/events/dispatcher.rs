use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use super::{Event, EventHandler, Priority, Propagation};

/// Identity hasher for TypeId — avoids SipHash overhead.
/// TypeId hashes via `write_u128()`; we take the lower 64 bits directly.
#[derive(Default)]
struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // Fallback: FNV-like fold for unexpected write calls
        for &b in bytes {
            self.0 = self.0.wrapping_mul(0x100000001b3).wrapping_add(b as u64);
        }
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = i;
    }

    #[inline]
    fn write_u128(&mut self, i: u128) {
        // TypeId hashes via write_u128 — take lower 64 bits
        self.0 = i as u64;
    }
}

// TypeId hashes via write_u64 (8 bytes) or write_u128 (16 bytes).
// Our hasher handles both, but verify TypeId is still a known size.
const _: () = assert!(
    std::mem::size_of::<TypeId>() == 8 || std::mem::size_of::<TypeId>() == 16,
    "TypeId size changed — update TypeIdHasher"
);

type TypeIdMap<V> = HashMap<TypeId, V, BuildHasherDefault<TypeIdHasher>>;

/// Type-erased synchronous handler function.
type ErasedFn = Box<dyn Fn(&mut dyn Any) -> Propagation + Send + Sync>;

/// Typed event dispatcher with safe type erasure.
///
/// Handlers are registered during startup via `on()`, then `freeze()` sorts
/// them by priority. After freezing, only `dispatch()` is available.
pub struct EventDispatcher {
    handlers: TypeIdMap<Vec<(Priority, ErasedFn)>>,
    frozen: bool,
}

impl EventDispatcher {
    /// Create an empty dispatcher in mutable (unfrozen) state.
    pub fn new() -> Self {
        Self {
            handlers: TypeIdMap::default(),
            frozen: false,
        }
    }

    /// Register a synchronous handler for event type `E`.
    ///
    /// # Panics
    ///
    /// Panics if the dispatcher has been frozen.
    pub fn on<E: Event>(&mut self, handler: impl EventHandler<E> + 'static) {
        assert!(!self.frozen, "cannot register handlers after freeze()");

        let priority = handler.priority();
        let f: ErasedFn = Box::new(move |event: &mut dyn Any| {
            handler.handle(event.downcast_mut::<E>().expect("event type mismatch"))
        });

        self.handlers
            .entry(TypeId::of::<E>())
            .or_default()
            .push((priority, f));
    }

    /// Freeze the dispatcher: sort handlers by priority (ascending) and prevent
    /// further registration.
    pub fn freeze(&mut self) {
        self.frozen = true;
        for handlers in self.handlers.values_mut() {
            handlers.sort_by_key(|(priority, _)| *priority);
        }
    }

    /// Dispatch an event to all registered handlers in priority order.
    ///
    /// Returns `Propagation::Stop` if any handler stopped propagation,
    /// otherwise `Propagation::Continue`.
    #[inline]
    pub fn dispatch<E: Event>(&self, event: &mut E) -> Propagation {
        let type_id = TypeId::of::<E>();
        let Some(handlers) = self.handlers.get(&type_id) else {
            return Propagation::Continue;
        };

        for (_, handler_fn) in handlers {
            if handler_fn(event) == Propagation::Stop {
                return Propagation::Stop;
            }
        }

        Propagation::Continue
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, EventHandler, Propagation};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // Test event
    struct TestEvent {
        value: i32,
    }

    impl Event for TestEvent {
        fn name(&self) -> &'static str {
            "test"
        }
    }

    // Another test event (to test type isolation)
    struct OtherEvent;

    impl Event for OtherEvent {
        fn name(&self) -> &'static str {
            "other"
        }
    }

    // Simple handler that increments event value
    struct IncrementHandler {
        amount: i32,
        prio: Priority,
    }

    impl EventHandler<TestEvent> for IncrementHandler {
        fn handle(&self, event: &mut TestEvent) -> Propagation {
            event.value += self.amount;
            Propagation::Continue
        }

        fn priority(&self) -> Priority {
            self.prio
        }
    }

    // Handler that stops propagation
    struct StopHandler;

    impl EventHandler<TestEvent> for StopHandler {
        fn handle(&self, _event: &mut TestEvent) -> Propagation {
            Propagation::Stop
        }

        fn priority(&self) -> Priority {
            0
        }
    }

    // Handler that records call order
    struct OrderTracker {
        call_order: Arc<AtomicUsize>,
        my_order: Arc<AtomicUsize>,
        prio: Priority,
    }

    impl EventHandler<TestEvent> for OrderTracker {
        fn handle(&self, _event: &mut TestEvent) -> Propagation {
            let order = self.call_order.fetch_add(1, Ordering::SeqCst);
            self.my_order.store(order, Ordering::SeqCst);
            Propagation::Continue
        }

        fn priority(&self) -> Priority {
            self.prio
        }
    }

    #[test]
    fn test_dispatch_empty() {
        let mut dispatcher = EventDispatcher::new();
        dispatcher.freeze();
        let mut event = TestEvent { value: 0 };
        let result = dispatcher.dispatch(&mut event);
        assert_eq!(result, Propagation::Continue);
        assert_eq!(event.value, 0);
    }

    #[test]
    fn test_dispatch_single_handler() {
        let mut dispatcher = EventDispatcher::new();
        dispatcher.on(IncrementHandler {
            amount: 10,
            prio: 0,
        });
        dispatcher.freeze();

        let mut event = TestEvent { value: 0 };
        dispatcher.dispatch(&mut event);
        assert_eq!(event.value, 10);
    }

    #[test]
    fn test_dispatch_multiple_handlers() {
        let mut dispatcher = EventDispatcher::new();
        dispatcher.on(IncrementHandler {
            amount: 10,
            prio: 0,
        });
        dispatcher.on(IncrementHandler { amount: 5, prio: 0 });
        dispatcher.freeze();

        let mut event = TestEvent { value: 0 };
        dispatcher.dispatch(&mut event);
        assert_eq!(event.value, 15);
    }

    #[test]
    fn test_dispatch_priority_order() {
        let call_counter = Arc::new(AtomicUsize::new(0));
        let order_a = Arc::new(AtomicUsize::new(999));
        let order_b = Arc::new(AtomicUsize::new(999));
        let order_c = Arc::new(AtomicUsize::new(999));

        let mut dispatcher = EventDispatcher::new();
        // Register in reverse priority order to verify sorting
        dispatcher.on(OrderTracker {
            call_order: Arc::clone(&call_counter),
            my_order: Arc::clone(&order_c),
            prio: 100,
        });
        dispatcher.on(OrderTracker {
            call_order: Arc::clone(&call_counter),
            my_order: Arc::clone(&order_a),
            prio: -50,
        });
        dispatcher.on(OrderTracker {
            call_order: Arc::clone(&call_counter),
            my_order: Arc::clone(&order_b),
            prio: 0,
        });
        dispatcher.freeze();

        let mut event = TestEvent { value: 0 };
        dispatcher.dispatch(&mut event);

        // a (prio -50) should be called first (order 0)
        assert_eq!(order_a.load(Ordering::SeqCst), 0);
        // b (prio 0) should be called second (order 1)
        assert_eq!(order_b.load(Ordering::SeqCst), 1);
        // c (prio 100) should be called third (order 2)
        assert_eq!(order_c.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_dispatch_propagation_stop() {
        let mut dispatcher = EventDispatcher::new();
        dispatcher.on(IncrementHandler {
            amount: 10,
            prio: -10,
        });
        dispatcher.on(StopHandler);
        dispatcher.on(IncrementHandler {
            amount: 100,
            prio: 10,
        });
        dispatcher.freeze();

        let mut event = TestEvent { value: 0 };
        let result = dispatcher.dispatch(&mut event);
        assert_eq!(result, Propagation::Stop);
        // First handler ran, third did not (stopped by second)
        assert_eq!(event.value, 10);
    }

    #[test]
    #[should_panic(expected = "cannot register handlers after freeze()")]
    fn test_register_after_freeze_panics() {
        let mut dispatcher = EventDispatcher::new();
        dispatcher.freeze();
        dispatcher.on(IncrementHandler { amount: 1, prio: 0 });
    }

    #[test]
    fn test_type_isolation() {
        let mut dispatcher = EventDispatcher::new();
        dispatcher.on(IncrementHandler {
            amount: 42,
            prio: 0,
        });
        dispatcher.freeze();

        // Dispatching OtherEvent should not trigger TestEvent handlers
        let mut other = OtherEvent;
        let result = dispatcher.dispatch(&mut other);
        assert_eq!(result, Propagation::Continue);
    }

    #[test]
    fn test_dispatch_without_freeze() {
        // Dispatching before freeze should work (handlers just aren't sorted)
        let mut dispatcher = EventDispatcher::new();
        dispatcher.on(IncrementHandler { amount: 5, prio: 0 });

        let mut event = TestEvent { value: 0 };
        dispatcher.dispatch(&mut event);
        assert_eq!(event.value, 5);
    }
}
