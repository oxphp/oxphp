# Decorator System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the PHP attribute-based decorator interception system — discovery, registry, observer hooks, and plugin API.

**Architecture:** PHP Observer API (`zend_observer_fcall_register`) intercepts decorated function calls. Two registration paths (PHP `oxphp_register_decorator()` and Rust `PluginContext::register_decorator()`) feed a shared `DecoratorRegistry`. The C observer dispatches to PHP `before()`/`after()` methods or Rust `on_begin()`/`on_end()` trait methods.

**Tech Stack:** Rust (types, registry, plugin API), C (observer hooks, bridge functions, PHP class registration), PHP 8.4 ZTS (attributes, reflection)

**Spec:** `docs/superpowers/specs/2026-03-21-php-attribute-decorators-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `Cargo.toml` | Modify | Add `bitflags` dependency |
| `src/lib.rs` | Modify:1 | Add `pub mod decorator` |
| `src/decorator/mod.rs` | Create | Module exports |
| `src/decorator/types.rs` | Create | `Decorator` trait, `DecoratorAction`, `DecoratorCallContext`, `DecoratorCallResult`, `AttributeTargets` |
| `src/decorator/registry.rs` | Create | `DecoratorRegistry`, `PhpDecoratorMeta`, `ResolvedDecorator`, resolution + cache |
| `src/plugin/context.rs` | Modify:15-48,126-141 | Add `decorators` field, `register_decorator()` method |
| `src/plugin/manager.rs` | Modify:13-31,43-64,136-140 | Add `decorators` field, wire through init, add `take_decorators()` |
| `src/plugin/mod.rs` | Modify:9-17 | Re-export decorator types |
| `src/main.rs` | Modify:43-49 | Wire decorator registry to bridge after plugin init |
| `src/decorator/dispatch.rs` | Create | Rust-side `unsafe extern "C" fn` dispatch callbacks for bridge |
| `src/bridge/ffi.rs` | Modify:65-68 | Add extern declarations for bridge functions |
| `src/bridge/mock.rs` | Modify | Add no-op stubs for new bridge functions |
| `ext/bridge/oxphp_bridge.h` | Modify | Add decorator typedefs, function declarations |
| `ext/bridge/oxphp_bridge.c` | Modify | Add static globals, setter/getter, TLS for reject reason and context stack |
| `ext/oxphp_sapi.c` | Modify | Register observer, PHP classes (`AttributeInterface`, `Context`), `oxphp_register_decorator()`, begin/end handlers, instance caching, PHP decorator dispatch |

---

### Task 1: Rust Types — `src/decorator/types.rs`

**Files:**
- Create: `src/decorator/types.rs`
- Create: `src/decorator/mod.rs`
- Modify: `Cargo.toml` (add bitflags)
- Modify: `src/lib.rs:1` (add module)

- [ ] **Step 1: Add `bitflags` dependency**

In `Cargo.toml`, add after line 47 (`dashmap = "6"`):
```toml
bitflags = "2"
```

- [ ] **Step 2: Write tests for types**

Create `src/decorator/types.rs`:

```rust
use std::sync::Arc;

/// Result of a decorator's on_begin() call.
#[derive(Debug, Clone, PartialEq)]
pub enum DecoratorAction {
    Continue,
    Reject(String),
}

bitflags::bitflags! {
    /// Which PHP attribute targets this decorator supports.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AttributeTargets: u32 {
        const FUNCTION = 0x01;
        const METHOD   = 0x02;
        const CLASS    = 0x04;
        const ALL      = Self::FUNCTION.bits() | Self::METHOD.bits() | Self::CLASS.bits();
    }
}

/// Context passed to decorator on_begin/on_end.
/// String fields use Arc<str> — allocated once during resolution, reused across calls.
#[derive(Debug, Clone)]
pub struct DecoratorCallContext {
    pub target: Arc<str>,
    pub class: Option<Arc<str>>,
    pub method: Option<Arc<str>>,
    pub function: Option<Arc<str>>,
    pub object_id: u64,
    pub request_id: String,
    pub trace_id: String,
    pub timestamp_ns: u64,
}

/// Result of the decorated function execution, passed to on_end.
#[derive(Debug, Clone)]
pub struct DecoratorCallResult {
    pub success: bool,
    pub elapsed_ns: u64,
    pub exception_class: Option<String>,
}

/// Trait for Rust-native decorators, registered by plugins.
pub trait Decorator: Send + Sync {
    /// Fully qualified PHP attribute class name (e.g. "App\\Profiler\\Timer").
    fn attribute_name(&self) -> &str;

    /// Which attribute targets this decorator supports (for registry optimization).
    fn targets(&self) -> AttributeTargets;

    /// Called before the decorated function executes.
    fn on_begin(&self, ctx: &DecoratorCallContext) -> DecoratorAction;

    /// Called after the decorated function executes.
    fn on_end(&self, ctx: &DecoratorCallContext, result: &DecoratorCallResult);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decorator_action_variants() {
        let action = DecoratorAction::Continue;
        assert_eq!(action, DecoratorAction::Continue);

        let action = DecoratorAction::Reject("circuit open".into());
        assert_eq!(action, DecoratorAction::Reject("circuit open".into()));
    }

    #[test]
    fn test_attribute_targets_bitflags() {
        let targets = AttributeTargets::FUNCTION | AttributeTargets::METHOD;
        assert!(targets.contains(AttributeTargets::FUNCTION));
        assert!(targets.contains(AttributeTargets::METHOD));
        assert!(!targets.contains(AttributeTargets::CLASS));

        let all = AttributeTargets::ALL;
        assert!(all.contains(AttributeTargets::FUNCTION));
        assert!(all.contains(AttributeTargets::METHOD));
        assert!(all.contains(AttributeTargets::CLASS));
    }

    #[test]
    fn test_call_context_arc_str_reuse() {
        let target: Arc<str> = Arc::from("App\\Service::method");
        let ctx1 = DecoratorCallContext {
            target: Arc::clone(&target),
            class: Some(Arc::from("App\\Service")),
            method: Some(Arc::from("method")),
            function: None,
            object_id: 42,
            request_id: "abc123".into(),
            trace_id: "trace-1".into(),
            timestamp_ns: 1000,
        };
        let ctx2 = DecoratorCallContext {
            target: Arc::clone(&target),
            class: ctx1.class.clone(),
            method: ctx1.method.clone(),
            function: None,
            object_id: 43,
            request_id: "def456".into(),
            trace_id: "trace-2".into(),
            timestamp_ns: 2000,
        };
        // Arc::ptr_eq proves same allocation reused
        assert!(Arc::ptr_eq(&ctx1.target, &ctx2.target));
    }

    #[test]
    fn test_call_result_fields() {
        let result = DecoratorCallResult {
            success: true,
            elapsed_ns: 5_000_000,
            exception_class: None,
        };
        assert!(result.success);
        assert_eq!(result.elapsed_ns, 5_000_000);
        assert!(result.exception_class.is_none());

        let result = DecoratorCallResult {
            success: false,
            elapsed_ns: 1_000,
            exception_class: Some("RuntimeException".into()),
        };
        assert!(!result.success);
        assert_eq!(result.exception_class.as_deref(), Some("RuntimeException"));
    }
}
```

- [ ] **Step 3: Create module file and wire into lib.rs**

Create `src/decorator/mod.rs`:

```rust
pub mod types;
pub mod registry;
pub mod dispatch;

pub use types::{
    AttributeTargets, Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult,
};
```

Note: `registry` re-exports are added in Task 2 after the module is implemented. `dispatch` re-exports are added in Task 5a. Create empty placeholders for both:

`src/decorator/registry.rs`: `// Implemented in Task 2.`
`src/decorator/dispatch.rs`: `// Implemented in Task 5a.`

Add to `src/lib.rs` after line 4 (`pub mod events;`):

```rust
pub mod decorator;
```

- [ ] **Step 4: Run tests**

Run: `cargo test --no-default-features --lib decorator::types`
Expected: all 4 tests pass. (Note: `registry` module doesn't exist yet — the `mod.rs` import will fail. Create an empty `src/decorator/registry.rs` placeholder first.)

Create empty `src/decorator/registry.rs`:
```rust
// Placeholder — implemented in Task 2.
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/lib.rs src/decorator/
git commit -m "feat(decorator): add core types — Decorator trait, DecoratorAction, AttributeTargets"
```

---

### Task 2: DecoratorRegistry — `src/decorator/registry.rs`

**Files:**
- Create: `src/decorator/registry.rs` (replace placeholder)

- [ ] **Step 1: Write tests for registry**

Replace `src/decorator/registry.rs` with:

```rust
use std::sync::Arc;

use dashmap::DashMap;

use super::types::{AttributeTargets, Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult};

/// Metadata for a PHP-side decorator registered via `oxphp_register_decorator()`.
#[derive(Debug, Clone)]
pub struct PhpDecoratorMeta {
    pub class_name: String,
    pub targets: AttributeTargets,
}

/// A resolved decorator for a specific function — either Rust-native or PHP.
#[derive(Clone)]
pub enum ResolvedDecorator {
    Rust(Arc<dyn Decorator>),
    /// cache_key is an opaque handle into the per-thread C-side instance cache.
    Php { class_name: String, cache_key: u64 },
}

/// Central registry for all decorator registrations.
/// Thread-safe via DashMap — accessed from multiple PHP worker threads.
pub struct DecoratorRegistry {
    /// Rust-native decorators: attribute_name → Decorator impl
    rust_decorators: DashMap<String, Arc<dyn Decorator>>,

    /// PHP decorators: attribute_name → metadata
    php_decorators: DashMap<String, PhpDecoratorMeta>,

    /// Resolution cache: fn_id → Vec<ResolvedDecorator>
    /// Populated by observer init on first call, reused thereafter.
    resolved: DashMap<usize, Vec<ResolvedDecorator>>,
}

impl DecoratorRegistry {
    pub fn new() -> Self {
        Self {
            rust_decorators: DashMap::new(),
            php_decorators: DashMap::new(),
            resolved: DashMap::new(),
        }
    }

    /// Register a Rust-native decorator (called by plugin init).
    pub fn register_rust(&self, decorator: Arc<dyn Decorator>) {
        let name = decorator.attribute_name().to_string();
        self.rust_decorators.insert(name, decorator);
    }

    /// Register a PHP-side decorator (called via bridge from oxphp_register_decorator).
    pub fn register_php(&self, class_name: String, targets: AttributeTargets) {
        self.php_decorators.insert(class_name.clone(), PhpDecoratorMeta {
            class_name,
            targets,
        });
    }

    /// Check if an attribute name is registered as a decorator (either Rust or PHP).
    pub fn is_registered(&self, attribute_name: &str) -> bool {
        self.rust_decorators.contains_key(attribute_name)
            || self.php_decorators.contains_key(attribute_name)
    }

    /// Resolve decorators for a function ID. Returns true if decorators were found.
    /// Called by observer init — caches the result for subsequent calls.
    /// `attr_names` is the list of attribute class names found on the function.
    pub fn resolve(&self, fn_id: usize, attr_names: &[String]) -> bool {
        let mut decorators = Vec::new();

        for (index, name) in attr_names.iter().enumerate() {
            if let Some(dec) = self.rust_decorators.get(name) {
                decorators.push(ResolvedDecorator::Rust(Arc::clone(dec.value())));
            } else if self.php_decorators.contains_key(name) {
                decorators.push(ResolvedDecorator::Php {
                    class_name: name.clone(),
                    cache_key: (fn_id as u64) << 16 | index as u64,
                });
            }
            // Unknown attributes (built-in PHP, unregistered) — silently skip.
        }

        let found = !decorators.is_empty();
        if found {
            self.resolved.insert(fn_id, decorators);
        }
        found
    }

    /// Get resolved decorators for a function ID.
    pub fn get_resolved(&self, fn_id: usize) -> Option<dashmap::mapref::one::Ref<'_, usize, Vec<ResolvedDecorator>>> {
        self.resolved.get(&fn_id)
    }

    /// Clear resolution cache (called at RSHUTDOWN in traditional mode).
    pub fn clear_cache(&self) {
        self.resolved.clear();
    }
}

impl Default for DecoratorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decorator::types::*;

    struct MockDecorator {
        name: &'static str,
        targets: AttributeTargets,
    }

    impl Decorator for MockDecorator {
        fn attribute_name(&self) -> &str { self.name }
        fn targets(&self) -> AttributeTargets { self.targets }
        fn on_begin(&self, _ctx: &DecoratorCallContext) -> DecoratorAction {
            DecoratorAction::Continue
        }
        fn on_end(&self, _ctx: &DecoratorCallContext, _result: &DecoratorCallResult) {}
    }

    #[test]
    fn test_register_rust_decorator() {
        let registry = DecoratorRegistry::new();
        let dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::ALL,
        });
        registry.register_rust(dec);
        assert!(registry.is_registered("App\\Timer"));
        assert!(!registry.is_registered("App\\Unknown"));
    }

    #[test]
    fn test_register_php_decorator() {
        let registry = DecoratorRegistry::new();
        registry.register_php("App\\Logger".into(), AttributeTargets::METHOD);
        assert!(registry.is_registered("App\\Logger"));
    }

    #[test]
    fn test_resolve_finds_registered_decorators() {
        let registry = DecoratorRegistry::new();
        let dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::ALL,
        });
        registry.register_rust(dec);
        registry.register_php("App\\Logger".into(), AttributeTargets::METHOD);

        let attrs = vec![
            "App\\Timer".to_string(),
            "Override".to_string(),          // PHP built-in — skipped
            "App\\Logger".to_string(),
        ];
        let found = registry.resolve(0x1234, &attrs);
        assert!(found);

        let resolved = registry.get_resolved(0x1234).unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(matches!(&resolved[0], ResolvedDecorator::Rust(_)));
        assert!(matches!(&resolved[1], ResolvedDecorator::Php { class_name, .. } if class_name == "App\\Logger"));
    }

    #[test]
    fn test_resolve_no_decorators_returns_false() {
        let registry = DecoratorRegistry::new();
        let attrs = vec!["Override".to_string(), "Deprecated".to_string()];
        let found = registry.resolve(0x5678, &attrs);
        assert!(!found);
        assert!(registry.get_resolved(0x5678).is_none());
    }

    #[test]
    fn test_resolve_caches_result() {
        let registry = DecoratorRegistry::new();
        let dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::ALL,
        });
        registry.register_rust(dec);

        let attrs = vec!["App\\Timer".to_string()];
        registry.resolve(0xABCD, &attrs);

        // Second call should find cached result
        assert!(registry.get_resolved(0xABCD).is_some());
    }

    #[test]
    fn test_clear_cache() {
        let registry = DecoratorRegistry::new();
        let dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::ALL,
        });
        registry.register_rust(dec);

        let attrs = vec!["App\\Timer".to_string()];
        registry.resolve(0x1111, &attrs);
        assert!(registry.get_resolved(0x1111).is_some());

        registry.clear_cache();
        assert!(registry.get_resolved(0x1111).is_none());

        // But registrations still exist
        assert!(registry.is_registered("App\\Timer"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --no-default-features --lib decorator::registry`
Expected: all 6 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/decorator/registry.rs
git commit -m "feat(decorator): add DecoratorRegistry with resolution cache and tests"
```

---

### Task 3: Plugin API Integration

**Files:**
- Modify: `src/plugin/context.rs:15-48,126-141`
- Modify: `src/plugin/manager.rs:13-31,43-64,136-140`
- Modify: `src/plugin/mod.rs:9-17`

- [ ] **Step 1: Write test for register_decorator**

In `src/plugin/context.rs`, first add to imports:

```rust
use crate::decorator::{Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult, AttributeTargets};
```

Define the storage struct (can go before `PluginContext`):

```rust
/// Definition collected during plugin init for a Rust-native decorator.
pub struct PluginDecoratorDef {
    pub decorator: Box<dyn Decorator>,
}
```

Add `decorators` field to `PluginContext`:

```rust
decorators: &'a mut Vec<PluginDecoratorDef>,
```

Add to `PluginContext::new()` parameter list and `Self` construction (mirrors `native_php_functions` pattern).

Add method:

```rust
/// Register a Rust-native decorator.
/// The decorator's `attribute_name()` is the fully qualified PHP attribute class name.
pub fn register_decorator(&mut self, decorator: impl Decorator + 'static) {
    self.decorators.push(PluginDecoratorDef {
        decorator: Box::new(decorator),
    });
}
```

Add test in the `#[cfg(test)]` module (use `make_context` pattern from existing tests — add `decorators: &'a mut Vec<PluginDecoratorDef>` param):

```rust
#[test]
fn test_register_decorator() {
    struct TestDecorator;
    impl Decorator for TestDecorator {
        fn attribute_name(&self) -> &str { "App\\TestDec" }
        fn targets(&self) -> AttributeTargets { AttributeTargets::ALL }
        fn on_begin(&self, _: &DecoratorCallContext) -> DecoratorAction { DecoratorAction::Continue }
        fn on_end(&self, _: &DecoratorCallContext, _: &DecoratorCallResult) {}
    }

    let mut dispatcher = EventDispatcher::new();
    let mut services = HashMap::new();
    let mut config = HashMap::new();
    let mut metrics = Vec::new();
    let mut routes = HashMap::new();
    let mut native_php = Vec::new();
    let mut decorators = Vec::new();

    let mut ctx = make_context(
        &mut dispatcher, &mut services, &mut config,
        &mut metrics, &mut routes, &mut native_php, &mut decorators,
    );

    ctx.register_decorator(TestDecorator);
    drop(ctx);
    assert_eq!(decorators.len(), 1);
    assert_eq!(decorators[0].decorator.attribute_name(), "App\\TestDec");
}
```

- [ ] **Step 2: Update PluginManager**

In `src/plugin/manager.rs`:

Add import: `use super::context::PluginDecoratorDef;`

Add field to `PluginManager`:
```rust
decorators: Vec<PluginDecoratorDef>,
```

Initialize in `new()`:
```rust
decorators: Vec::new(),
```

Pass to `PluginContext::new()` in `init_all()`:
```rust
&mut self.decorators,
```

Add accessor (after `take_native_php_functions` at line ~138):
```rust
/// Take decorator definitions (empties the internal vec).
/// Call after init_all(), before wrapping manager in Arc.
pub fn take_decorators(&mut self) -> Vec<PluginDecoratorDef> {
    std::mem::take(&mut self.decorators)
}
```

- [ ] **Step 3: Update plugin mod.rs re-exports**

In `src/plugin/mod.rs`, add to the `pub use context` line:
```rust
pub use context::{PluginContext, PluginDecoratorDef};
```

- [ ] **Step 4: Run tests**

Run: `cargo test --no-default-features --lib plugin::context`
Expected: all existing tests pass + new `test_register_decorator` passes.

Run: `cargo test --no-default-features --lib`
Expected: full suite passes (no regressions).

- [ ] **Step 5: Commit**

```bash
git add src/plugin/context.rs src/plugin/manager.rs src/plugin/mod.rs
git commit -m "feat(decorator): add PluginContext::register_decorator() and PluginManager wiring"
```

---

### Task 4: Bridge FFI — Rust Side

**Files:**
- Modify: `src/bridge/ffi.rs:65-68`
- Modify: `src/bridge/mock.rs`

- [ ] **Step 1: Add FFI declarations**

In `src/bridge/ffi.rs`, add after `oxphp_bridge_set_native_dispatch` (line ~68):

```rust
    // ── Decorator system ──
    pub fn oxphp_bridge_set_decorator_registry(ptr: *const c_void);

    pub fn oxphp_bridge_set_decorator_resolve(
        f: Option<unsafe extern "C" fn(
            fn_id: usize,
            attr_names: *const *const c_char,
            attr_count: u32,
        ) -> c_int>,
    );

    pub fn oxphp_bridge_set_decorator_begin(
        f: Option<unsafe extern "C" fn(
            fn_id: usize,
            target: *const c_char,
            class_name: *const c_char,  // NULL for functions
            object_id: u64,
            timestamp_ns: u64,
        ) -> c_int>,
    );

    pub fn oxphp_bridge_set_decorator_end(
        f: Option<unsafe extern "C" fn(
            fn_id: usize,
            elapsed_ns: u64,
            success: c_int,
            exception_class: *const c_char,  // NULL if no exception
        )>,
    );

    pub fn oxphp_bridge_get_decorator_reject_reason(
        out_len: *mut usize,
    ) -> *const c_char;

    pub fn oxphp_bridge_set_decorator_reject_reason(
        reason: *const c_char,
        len: usize,
    );

    pub fn oxphp_bridge_clear_decorator_reject_reason();

    /// Register a PHP-side decorator in the Rust registry.
    /// Called from oxphp_register_decorator() via bridge.
    pub fn oxphp_bridge_register_php_decorator(
        class_name: *const c_char,
        targets: u32,
    );
```

- [ ] **Step 2: Add mock stubs**

In `src/bridge/mock.rs`, add matching `pub unsafe fn` stubs that are no-ops (return 0 / null as appropriate). This allows `--no-default-features` compilation.

- [ ] **Step 3: Run compilation check**

Run: `cargo test --no-default-features --lib`
Expected: compiles and all tests pass. The FFI functions are declared but never called from Rust test code.

- [ ] **Step 4: Commit**

```bash
git add src/bridge/ffi.rs src/bridge/mock.rs
git commit -m "feat(decorator): add bridge FFI declarations for decorator system"
```

---

### Task 5: main.rs Wiring

**Files:**
- Modify: `src/main.rs:43-49`

- [ ] **Step 1: Wire decorator registry to bridge**

In `src/main.rs`, inside the `#[cfg(feature = "php")]` block (line 43), after the native_fns block (line 48), add:

```rust
        // Register decorator definitions with the decorator registry
        let decorator_defs = plugin_manager.take_decorators();
        let registry = std::sync::Arc::new(oxphp::decorator::DecoratorRegistry::new());
        for def in decorator_defs {
            registry.register_rust(std::sync::Arc::from(def.decorator));
        }
        // Install Rust dispatch callbacks and pass registry to bridge
        oxphp::decorator::dispatch::install_bridge_callbacks(std::sync::Arc::clone(&registry));
        // Leak the Arc so the raw pointer in the bridge is valid for the process lifetime
        std::mem::forget(registry);
```

- [ ] **Step 2: Run compilation check**

Run: `cargo clippy --no-default-features -- -D warnings`
Expected: compiles clean. The `#[cfg(feature = "php")]` block is not compiled without the `php` feature.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(decorator): wire decorator registry creation in main.rs startup"
```

---

### Task 5a: Rust Dispatch Callbacks — `src/decorator/dispatch.rs`

**Files:**
- Create: `src/decorator/dispatch.rs` (replace placeholder)
- Modify: `src/decorator/mod.rs` (add re-export)

This is the critical missing piece — the `unsafe extern "C" fn` functions that the C bridge calls into Rust. They look up resolved decorators in the registry and call `on_begin()`/`on_end()`.

- [ ] **Step 1: Implement dispatch callbacks**

Replace `src/decorator/dispatch.rs`:

```rust
//! Rust-side dispatch callbacks installed into the C bridge.
//! These are called from observer begin/end handlers on PHP worker threads.

use std::ffi::{c_char, c_int, CStr};
use std::sync::Arc;

use super::registry::DecoratorRegistry;
use super::types::{DecoratorAction, DecoratorCallContext, DecoratorCallResult};

/// Global registry pointer — set once at startup, read from worker threads.
static mut REGISTRY: Option<Arc<DecoratorRegistry>> = None;

/// Install all bridge callbacks and store the registry globally.
///
/// # Safety
/// Must be called exactly once, before any worker threads are spawned.
pub fn install_bridge_callbacks(registry: Arc<DecoratorRegistry>) {
    unsafe {
        REGISTRY = Some(registry);
        #[cfg(feature = "php")]
        {
            crate::bridge::ffi::oxphp_bridge_set_decorator_resolve(Some(resolve_callback));
            crate::bridge::ffi::oxphp_bridge_set_decorator_begin(Some(begin_callback));
            crate::bridge::ffi::oxphp_bridge_set_decorator_end(Some(end_callback));
        }
    }
}

fn get_registry() -> &'static DecoratorRegistry {
    unsafe { REGISTRY.as_ref().expect("decorator registry not initialized") }
}

/// Called by observer init — resolve which decorators apply to this function.
unsafe extern "C" fn resolve_callback(
    fn_id: usize,
    attr_names: *const *const c_char,
    attr_count: u32,
) -> c_int {
    let registry = get_registry();
    let mut names = Vec::with_capacity(attr_count as usize);
    for i in 0..attr_count as usize {
        let ptr = *attr_names.add(i);
        if let Ok(s) = CStr::from_ptr(ptr).to_str() {
            names.push(s.to_string());
        }
    }
    if registry.resolve(fn_id, &names) { 1 } else { 0 }
}

/// Called by observer begin — dispatch on_begin to Rust decorators (in order).
/// Returns 0=Continue, 1=Reject.
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

    let target_str: Arc<str> = Arc::from(
        CStr::from_ptr(target).to_str().unwrap_or("")
    );
    let class_str = if class_name.is_null() {
        None
    } else {
        Some(Arc::from(CStr::from_ptr(class_name).to_str().unwrap_or("")))
    };

    let ctx = DecoratorCallContext {
        target: Arc::clone(&target_str),
        class: class_str.clone(),
        method: if class_str.is_some() { Some(Arc::clone(&target_str)) } else { None },
        function: if class_str.is_none() { Some(Arc::clone(&target_str)) } else { None },
        object_id,
        request_id: String::new(), // TODO: read from bridge TLS
        trace_id: String::new(),   // TODO: read from $_SERVER
        timestamp_ns,
    };

    for dec in resolved.iter() {
        if let super::registry::ResolvedDecorator::Rust(ref decorator) = dec {
            match decorator.on_begin(&ctx) {
                DecoratorAction::Continue => {}
                DecoratorAction::Reject(reason) => {
                    // Store reject reason in TLS for C to read
                    #[cfg(feature = "php")]
                    {
                        let bytes = reason.as_bytes();
                        crate::bridge::ffi::oxphp_bridge_set_decorator_reject_reason(
                            bytes.as_ptr() as *const c_char,
                            bytes.len(),
                        );
                    }
                    return 1;
                }
            }
        }
    }
    0
}

/// Called by observer end — dispatch on_end to Rust decorators (in reverse order).
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
        CStr::from_ptr(exception_class).to_str().ok().map(String::from)
    };

    let result = DecoratorCallResult {
        success: success != 0,
        elapsed_ns,
        exception_class: exc,
    };

    // Build a minimal context (target/class come from the resolved cache in a real impl,
    // for now use empty — the begin_callback already gave full context)
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
        if let super::registry::ResolvedDecorator::Rust(ref decorator) = dec {
            decorator.on_end(&ctx, &result);
        }
    }
}

/// Register a PHP-side decorator from the bridge (called by oxphp_register_decorator).
#[cfg(feature = "php")]
pub unsafe extern "C" fn register_php_decorator_callback(
    class_name: *const c_char,
    targets: u32,
) {
    let registry = get_registry();
    if let Ok(name) = CStr::from_ptr(class_name).to_str() {
        use super::types::AttributeTargets;
        registry.register_php(name.to_string(), AttributeTargets::from_bits_truncate(targets));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decorator::types::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingDecorator {
        begin_count: AtomicU32,
        end_count: AtomicU32,
    }

    impl Decorator for CountingDecorator {
        fn attribute_name(&self) -> &str { "Test\\Counter" }
        fn targets(&self) -> AttributeTargets { AttributeTargets::ALL }
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
        assert!(registry.resolve(0x1, &attrs));

        // Simulate begin dispatch
        let resolved = registry.get_resolved(0x1).unwrap();
        let ctx = DecoratorCallContext {
            target: Arc::from("test_fn"),
            class: None, method: None, function: Some(Arc::from("test_fn")),
            object_id: 0, request_id: String::new(), trace_id: String::new(),
            timestamp_ns: 0,
        };
        for d in resolved.iter() {
            if let super::super::registry::ResolvedDecorator::Rust(ref decorator) = d {
                assert_eq!(decorator.on_begin(&ctx), DecoratorAction::Continue);
            }
        }
        assert_eq!(dec_ref.begin_count.load(Ordering::Relaxed), 1);

        // Simulate end dispatch (reverse order)
        let result = DecoratorCallResult { success: true, elapsed_ns: 100, exception_class: None };
        for d in resolved.iter().rev() {
            if let super::super::registry::ResolvedDecorator::Rust(ref decorator) = d {
                decorator.on_end(&ctx, &result);
            }
        }
        assert_eq!(dec_ref.end_count.load(Ordering::Relaxed), 1);
    }
}
```

- [ ] **Step 2: Update mod.rs re-exports**

Add to `src/decorator/mod.rs`:
```rust
pub use registry::{DecoratorRegistry, PhpDecoratorMeta, ResolvedDecorator};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --no-default-features --lib decorator::dispatch`
Expected: `test_registry_dispatch_round_trip` passes.

- [ ] **Step 4: Commit**

```bash
git add src/decorator/dispatch.rs src/decorator/mod.rs
git commit -m "feat(decorator): add Rust-side dispatch callbacks for bridge integration"
```

---

### Task 6: Bridge C Side — `oxphp_bridge.{h,c}`

**Files:**
- Modify: `ext/bridge/oxphp_bridge.h`
- Modify: `ext/bridge/oxphp_bridge.c`

- [ ] **Step 1: Add declarations to header**

In `ext/bridge/oxphp_bridge.h`, add after the native dispatch section (around line 244):

```c
/* ── Decorator system ── */

/* Callback: resolve decorator attributes for a function.
 * Returns 1 if decorators found, 0 otherwise. */
typedef int (*oxphp_decorator_resolve_fn_t)(
    uintptr_t fn_id,
    const char **attr_names,
    uint32_t attr_count
);

/* Callback: begin decorator dispatch. Returns 0=Continue, 1=Reject. */
typedef int (*oxphp_decorator_begin_fn_t)(
    uintptr_t fn_id,
    const char *target,
    const char *class_name,
    uint64_t object_id,
    uint64_t timestamp_ns
);

/* Callback: end decorator dispatch. */
typedef void (*oxphp_decorator_end_fn_t)(
    uintptr_t fn_id,
    uint64_t elapsed_ns,
    int success,
    const char *exception_class
);

void oxphp_bridge_set_decorator_registry(void *ptr);
void *oxphp_bridge_get_decorator_registry(void);

void oxphp_bridge_set_decorator_resolve(oxphp_decorator_resolve_fn_t fn);
oxphp_decorator_resolve_fn_t oxphp_bridge_get_decorator_resolve(void);

void oxphp_bridge_set_decorator_begin(oxphp_decorator_begin_fn_t fn);
oxphp_decorator_begin_fn_t oxphp_bridge_get_decorator_begin(void);

void oxphp_bridge_set_decorator_end(oxphp_decorator_end_fn_t fn);
oxphp_decorator_end_fn_t oxphp_bridge_get_decorator_end(void);

/* Reject reason — stored in __thread TLS, set by begin callback. */
void oxphp_bridge_set_decorator_reject_reason(const char *reason, size_t len);
const char *oxphp_bridge_get_decorator_reject_reason(size_t *out_len);
void oxphp_bridge_clear_decorator_reject_reason(void);

/* Decorator context stack (TLS) for nested decorated calls. */
#define OXPHP_DECORATOR_CTX_STACK_MAX 32

typedef struct {
    uintptr_t fn_id;
    const char *target;
    const char *class_name;
    uint64_t object_id;
    uint64_t timestamp_ns;
    void *execute_data;      /* zend_execute_data* for lazy getParams() */
    int decorator_count;     /* how many decorators' before() succeeded */
} oxphp_decorator_ctx_t;

oxphp_decorator_ctx_t *oxphp_decorator_ctx_push(void);
oxphp_decorator_ctx_t *oxphp_decorator_ctx_peek(void);
void oxphp_decorator_ctx_pop(void);
```

- [ ] **Step 2: Implement in bridge.c**

In `ext/bridge/oxphp_bridge.c`, add:

```c
/* ── Decorator system — global callbacks (set once before worker threads) ── */
static void *decorator_registry_ptr = NULL;
static oxphp_decorator_resolve_fn_t decorator_resolve_fn = NULL;
static oxphp_decorator_begin_fn_t decorator_begin_fn = NULL;
static oxphp_decorator_end_fn_t decorator_end_fn = NULL;

void oxphp_bridge_set_decorator_registry(void *ptr) { decorator_registry_ptr = ptr; }
void *oxphp_bridge_get_decorator_registry(void) { return decorator_registry_ptr; }

void oxphp_bridge_set_decorator_resolve(oxphp_decorator_resolve_fn_t fn) { decorator_resolve_fn = fn; }
oxphp_decorator_resolve_fn_t oxphp_bridge_get_decorator_resolve(void) { return decorator_resolve_fn; }

void oxphp_bridge_set_decorator_begin(oxphp_decorator_begin_fn_t fn) { decorator_begin_fn = fn; }
oxphp_decorator_begin_fn_t oxphp_bridge_get_decorator_begin(void) { return decorator_begin_fn; }

void oxphp_bridge_set_decorator_end(oxphp_decorator_end_fn_t fn) { decorator_end_fn = fn; }
oxphp_decorator_end_fn_t oxphp_bridge_get_decorator_end(void) { return decorator_end_fn; }

/* ── Reject reason — per-thread TLS ── */
static __thread char decorator_reject_buf[256];
static __thread size_t decorator_reject_len = 0;

void oxphp_bridge_set_decorator_reject_reason(const char *reason, size_t len) {
    if (len > sizeof(decorator_reject_buf) - 1) len = sizeof(decorator_reject_buf) - 1;
    memcpy(decorator_reject_buf, reason, len);
    decorator_reject_buf[len] = '\0';
    decorator_reject_len = len;
}
const char *oxphp_bridge_get_decorator_reject_reason(size_t *out_len) {
    if (out_len) *out_len = decorator_reject_len;
    return decorator_reject_buf;
}
void oxphp_bridge_clear_decorator_reject_reason(void) {
    decorator_reject_len = 0;
    decorator_reject_buf[0] = '\0';
}

/* ── Decorator context stack — per-thread TLS ── */
static __thread oxphp_decorator_ctx_t decorator_ctx_stack[OXPHP_DECORATOR_CTX_STACK_MAX];
static __thread int decorator_ctx_depth = 0;

oxphp_decorator_ctx_t *oxphp_decorator_ctx_push(void) {
    if (decorator_ctx_depth >= OXPHP_DECORATOR_CTX_STACK_MAX) {
        return &decorator_ctx_stack[OXPHP_DECORATOR_CTX_STACK_MAX - 1]; /* overflow safety */
    }
    return &decorator_ctx_stack[decorator_ctx_depth++];
}
oxphp_decorator_ctx_t *oxphp_decorator_ctx_peek(void) {
    if (decorator_ctx_depth <= 0) return NULL;
    return &decorator_ctx_stack[decorator_ctx_depth - 1];
}
void oxphp_decorator_ctx_pop(void) {
    if (decorator_ctx_depth > 0) decorator_ctx_depth--;
}
```

- [ ] **Step 3: Commit**

```bash
git add ext/bridge/oxphp_bridge.h ext/bridge/oxphp_bridge.c
git commit -m "feat(decorator): add bridge C functions for decorator registry, context stack, and callbacks"
```

---

### Task 7: PHP Classes and Registration — `ext/oxphp_sapi.c`

**Files:**
- Modify: `ext/oxphp_sapi.c`

This is the largest task. It adds: interface, context class, `oxphp_register_decorator()` function, observer hooks, and instance caching.

- [ ] **Step 1: Add global class entry pointers**

Near the top of `ext/oxphp_sapi.c` (where `oxphp_async_exception_ce` etc. are declared):

```c
/* Decorator system class entries */
static zend_class_entry *oxphp_decorator_interface_ce = NULL;
static zend_class_entry *oxphp_decorator_context_ce = NULL;
static zend_class_entry *oxphp_decorator_rejected_ce = NULL;

/* Decorator instance cache — per-thread */
#define OXPHP_DEC_CACHE_MAX 256
static __thread zval decorator_instance_cache[OXPHP_DEC_CACHE_MAX];
static __thread int decorator_instance_count = 0;
```

- [ ] **Step 2: Register PHP interface and classes in MINIT**

In `PHP_MINIT_FUNCTION(oxphp_sapi)` (line ~1016), after the BorrowedProxy class registration, add:

```c
    /* OxPHP\Decorator\AttributeInterface */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Decorator", "AttributeInterface",
            oxphp_decorator_interface_methods);
        oxphp_decorator_interface_ce = zend_register_internal_interface(&tmp_ce);
    }

    /* OxPHP\Decorator\Context */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Decorator", "Context",
            oxphp_decorator_context_methods);
        oxphp_decorator_context_ce = zend_register_internal_class(&tmp_ce);
        oxphp_decorator_context_ce->ce_flags |= ZEND_ACC_FINAL;

        /* Readonly properties */
        zend_declare_property_string(oxphp_decorator_context_ce, "target", sizeof("target")-1, "", ZEND_ACC_PUBLIC|ZEND_ACC_READONLY);
        zend_declare_property_string(oxphp_decorator_context_ce, "class", sizeof("class")-1, "", ZEND_ACC_PUBLIC|ZEND_ACC_READONLY);
        zend_declare_property_string(oxphp_decorator_context_ce, "method", sizeof("method")-1, "", ZEND_ACC_PUBLIC|ZEND_ACC_READONLY);
        zend_declare_property_string(oxphp_decorator_context_ce, "function", sizeof("function")-1, "", ZEND_ACC_PUBLIC|ZEND_ACC_READONLY);
        zend_declare_property_long(oxphp_decorator_context_ce, "objectId", sizeof("objectId")-1, 0, ZEND_ACC_PUBLIC|ZEND_ACC_READONLY);
        zend_declare_property_string(oxphp_decorator_context_ce, "requestId", sizeof("requestId")-1, "", ZEND_ACC_PUBLIC|ZEND_ACC_READONLY);
        zend_declare_property_string(oxphp_decorator_context_ce, "traceId", sizeof("traceId")-1, "", ZEND_ACC_PUBLIC|ZEND_ACC_READONLY);
    }

    /* OxPHP\Decorator\RejectedException */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Decorator", "RejectedException", NULL);
        oxphp_decorator_rejected_ce = zend_register_internal_class_ex(&tmp_ce, zend_ce_exception);
    }

    /* Register observer for decorator interception */
    zend_observer_fcall_register(oxphp_decorator_observer_init);
```

Define the interface methods array (before MINIT):

```c
/* AttributeInterface methods */
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_decorator_before, 0, 1, IS_VOID, 0)
    ZEND_ARG_OBJ_INFO(0, ctx, OxPHP\\Decorator\\Context, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_decorator_after, 0, 1, IS_VOID, 0)
    ZEND_ARG_OBJ_INFO(0, ctx, OxPHP\\Decorator\\Context, 0)
ZEND_END_ARG_INFO()

static const zend_function_entry oxphp_decorator_interface_methods[] = {
    ZEND_ABSTRACT_ME(AttributeInterface, before, arginfo_decorator_before)
    ZEND_ABSTRACT_ME(AttributeInterface, after, arginfo_decorator_after)
    PHP_FE_END
};
```

Define Context class methods (getParams, getResult, hasResult):

```c
/* Context methods: getParams(), getResult(), hasResult() */
ZEND_METHOD(OxPHP_Decorator_Context, getParams) { /* impl in Step 4 */ }
ZEND_METHOD(OxPHP_Decorator_Context, getResult) { /* impl in Step 4 */ }
ZEND_METHOD(OxPHP_Decorator_Context, hasResult) { /* impl in Step 4 */ }

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_ctx_getParams, 0, 0, IS_ARRAY, 0)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_ctx_getResult, 0, 0, IS_MIXED, 0)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_ctx_hasResult, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

static const zend_function_entry oxphp_decorator_context_methods[] = {
    ZEND_ME(OxPHP_Decorator_Context, getParams, arginfo_ctx_getParams, ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Decorator_Context, getResult, arginfo_ctx_getResult, ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Decorator_Context, hasResult, arginfo_ctx_hasResult, ZEND_ACC_PUBLIC)
    PHP_FE_END
};
```

- [ ] **Step 3: Implement `oxphp_register_decorator()` PHP function**

Add to `oxphp_sapi_functions[]` array:

```c
PHP_FE(oxphp_register_decorator, arginfo_oxphp_register_decorator)
```

Arginfo and implementation:

```c
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_register_decorator, 0, 1, _IS_BOOL, 0)
    ZEND_ARG_TYPE_INFO(0, class, IS_STRING, 0)
ZEND_END_ARG_INFO()

PHP_FUNCTION(oxphp_register_decorator) {
    zend_string *class_name;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(class_name)
    ZEND_PARSE_PARAMETERS_END();

    /* Look up the class */
    zend_class_entry *ce = zend_lookup_class(class_name);
    if (!ce) {
        php_error_docref(NULL, E_WARNING, "Class '%s' not found", ZSTR_VAL(class_name));
        RETURN_FALSE;
    }

    /* Verify implements AttributeInterface */
    if (!instanceof_function(ce, oxphp_decorator_interface_ce)) {
        php_error_docref(NULL, E_WARNING,
            "Class '%s' does not implement OxPHP\\Decorator\\AttributeInterface",
            ZSTR_VAL(class_name));
        RETURN_FALSE;
    }

    /* Verify class has #[Attribute] */
    if (!ce->attributes) {
        php_error_docref(NULL, E_WARNING,
            "Class '%s' is not marked with #[Attribute]", ZSTR_VAL(class_name));
        RETURN_FALSE;
    }

    /* Check for Attribute attribute on the class */
    zend_attribute *attr = zend_get_attribute_str(
        ce->attributes, "Attribute", sizeof("Attribute")-1);
    if (!attr) {
        php_error_docref(NULL, E_WARNING,
            "Class '%s' is not marked with #[Attribute]", ZSTR_VAL(class_name));
        RETURN_FALSE;
    }

    /* Read target flags from Attribute constructor arg (default: TARGET_ALL) */
    uint32_t targets = 0x3F; /* ZEND_ATTRIBUTE_TARGET_ALL */
    if (attr->argc > 0) {
        zval tmp;
        if (SUCCESS == zend_get_attribute_value(&tmp, attr, 0, ce)) {
            targets = (uint32_t)zval_get_long(&tmp);
            zval_ptr_dtor(&tmp);
        }
    }

    /* Convert PHP targets to our bitflags and send to Rust registry */
    oxphp_decorator_resolve_fn_t resolve_fn = oxphp_bridge_get_decorator_resolve();
    if (!resolve_fn) {
        php_error_docref(NULL, E_WARNING, "Decorator system not initialized");
        RETURN_FALSE;
    }

    /* Register via bridge — Rust side stores in DecoratorRegistry.php_decorators */
    const char *name = ZSTR_VAL(class_name);
    /* For now: directly register the class name. The resolve callback
     * recognizes it as a PHP decorator when observer init queries. */
    /* TODO: proper bridge function for PHP decorator registration */

    RETURN_TRUE;
}
```

- [ ] **Step 4: Implement observer hooks**

```c
/* ── Observer hooks ── */

static zend_observer_fcall_handlers oxphp_decorator_observer_init(
    zend_execute_data *execute_data
) {
    zend_function *func = execute_data->func;
    if (!func || func->type != ZEND_USER_FUNCTION) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    oxphp_decorator_resolve_fn_t resolve = oxphp_bridge_get_decorator_resolve();
    if (!resolve) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    /* Collect attribute names from function/method and class */
    const char *attr_names[64];
    uint32_t attr_count = 0;

    /* Function/method attributes */
    if (func->common.attributes) {
        ZEND_HASH_PACKED_FOREACH_PTR(func->common.attributes, zend_attribute *a) {
            if (attr_count < 64) {
                attr_names[attr_count++] = ZSTR_VAL(a->name);
            }
        } ZEND_HASH_FOREACH_END();
    }

    /* Class attributes (for TARGET_CLASS) */
    if (func->common.scope && func->common.scope->attributes) {
        ZEND_HASH_PACKED_FOREACH_PTR(func->common.scope->attributes, zend_attribute *a) {
            if (attr_count < 64) {
                attr_names[attr_count++] = ZSTR_VAL(a->name);
            }
        } ZEND_HASH_FOREACH_END();
    }

    if (attr_count == 0) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    uintptr_t fn_id = (uintptr_t)func;
    int found = resolve(fn_id, attr_names, attr_count);
    if (!found) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    return (zend_observer_fcall_handlers){
        oxphp_decorator_begin,
        oxphp_decorator_end
    };
}

static void oxphp_decorator_begin(zend_execute_data *execute_data) {
    oxphp_decorator_ctx_t *ctx = oxphp_decorator_ctx_push();
    zend_function *func = execute_data->func;

    ctx->fn_id = (uintptr_t)func;
    ctx->target = func->common.function_name
        ? ZSTR_VAL(func->common.function_name) : "";
    ctx->class_name = (func->common.scope)
        ? ZSTR_VAL(func->common.scope->name) : NULL;
    ctx->object_id = (Z_TYPE(execute_data->This) == IS_OBJECT)
        ? Z_OBJ(execute_data->This)->handle : 0;
    ctx->execute_data = execute_data;
    ctx->decorator_count = 0;

    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    ctx->timestamp_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;

    /* Dispatch to Rust decorators */
    oxphp_decorator_begin_fn_t begin_fn = oxphp_bridge_get_decorator_begin();
    if (begin_fn) {
        int action = begin_fn(ctx->fn_id, ctx->target, ctx->class_name,
                              ctx->object_id, ctx->timestamp_ns);
        if (action != 0) { /* Reject */
            size_t reason_len;
            const char *reason = oxphp_bridge_get_decorator_reject_reason(&reason_len);
            zend_throw_exception(oxphp_decorator_rejected_ce,
                reason_len > 0 ? reason : "Decorator rejected", 0);
            oxphp_bridge_clear_decorator_reject_reason();
            return;
        }
    }

    /* TODO: Dispatch to PHP decorators (cached instances) —
     * iterate resolved PHP decorators, call $dec->before($ctx).
     * Track ctx->decorator_count for cleanup on exception. */
}

static void oxphp_decorator_end(zend_execute_data *execute_data, zval *retval) {
    oxphp_decorator_ctx_t *ctx = oxphp_decorator_ctx_peek();
    if (!ctx) return;

    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uint64_t now_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
    uint64_t elapsed_ns = now_ns - ctx->timestamp_ns;
    int success = !EG(exception);

    /* TODO: Dispatch to PHP decorators in reverse order —
     * iterate resolved PHP decorators reversed, call $dec->after($ctx).
     * Pass retval for lazy getResult(). */

    /* Dispatch to Rust decorators */
    oxphp_decorator_end_fn_t end_fn = oxphp_bridge_get_decorator_end();
    if (end_fn) {
        const char *exc_class = NULL;
        if (!success && EG(exception)) {
            exc_class = ZSTR_VAL(EG(exception)->ce->name);
        }
        end_fn(ctx->fn_id, elapsed_ns, success, exc_class);
    }

    oxphp_decorator_ctx_pop();
}
```

- [ ] **Step 5: Add RSHUTDOWN cleanup for instance cache**

In `PHP_RSHUTDOWN_FUNCTION(oxphp_sapi)` (find existing or add), add:

```c
    /* Clear decorator instance cache */
    for (int i = 0; i < decorator_instance_count; i++) {
        zval_ptr_dtor(&decorator_instance_cache[i]);
        ZVAL_UNDEF(&decorator_instance_cache[i]);
    }
    decorator_instance_count = 0;
```

- [ ] **Step 6: Commit**

```bash
git add ext/oxphp_sapi.c
git commit -m "feat(decorator): add PHP interface, context class, observer hooks, and registration"
```

---

### Task 8: Docker Build Verification

**Files:** None modified — verification only.

- [ ] **Step 1: Run host tests**

```bash
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features
```
Expected: all pass, no warnings.

- [ ] **Step 2: Docker build**

```bash
docker compose build
```
Expected: compiles with `--features php`. C bridge and extension compile without errors.

- [ ] **Step 3: Docker smoke test**

Create `www/public/test_decorator.php`:

```php
<?php
// Verify classes exist
var_dump(interface_exists('OxPHP\Decorator\AttributeInterface'));
var_dump(class_exists('OxPHP\Decorator\Context'));
var_dump(class_exists('OxPHP\Decorator\RejectedException'));
var_dump(function_exists('oxphp_register_decorator'));

// Verify interface methods
$ref = new ReflectionClass('OxPHP\Decorator\AttributeInterface');
var_dump($ref->hasMethod('before'));
var_dump($ref->hasMethod('after'));
echo "DECORATOR_SYSTEM_OK\n";
```

```bash
docker compose up -d && sleep 2
curl -s http://localhost:8080/test_decorator.php
```
Expected output:
```
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
DECORATOR_SYSTEM_OK
```

- [ ] **Step 4: Commit test file and cleanup**

```bash
git add www/public/test_decorator.php
git commit -m "test(decorator): add PHP smoke test for decorator system classes"
```

---

### Task 9: Integration Test — Full Decorator Flow

**Files:**
- Create: `www/public/test_decorator_flow.php`

- [ ] **Step 1: Write end-to-end test**

Create `www/public/test_decorator_flow.php`:

```php
<?php
#[Attribute(Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD | Attribute::TARGET_CLASS)]
class DebugDecorator implements OxPHP\Decorator\AttributeInterface {
    private string $tag;

    public function __construct(public readonly string $label = 'default') {
        $this->tag = '';
    }

    public function before(OxPHP\Decorator\Context $ctx): void {
        $this->tag = $this->label . ':' . $ctx->target;
        echo "BEFORE:{$this->tag}\n";
    }

    public function after(OxPHP\Decorator\Context $ctx): void {
        echo "AFTER:{$this->tag}\n";
    }
}

oxphp_register_decorator(DebugDecorator::class);

#[DebugDecorator(label: 'fn')]
function decorated_function(): string {
    echo "EXEC:decorated_function\n";
    return "ok";
}

class MyService {
    #[DebugDecorator(label: 'method')]
    public function doWork(): void {
        echo "EXEC:doWork\n";
    }
}

// Test 1: decorated function
echo "--- Test 1: Function ---\n";
decorated_function();

// Test 2: decorated method
echo "--- Test 2: Method ---\n";
$svc = new MyService();
$svc->doWork();

echo "ALL_TESTS_PASSED\n";
```

- [ ] **Step 2: Test via Docker**

```bash
docker compose build && docker compose up -d && sleep 2
curl -s http://localhost:8080/test_decorator_flow.php
```

Expected:
```
--- Test 1: Function ---
BEFORE:fn:decorated_function
EXEC:decorated_function
AFTER:fn:decorated_function
--- Test 2: Method ---
BEFORE:method:doWork
EXEC:doWork
AFTER:method:doWork
ALL_TESTS_PASSED
```

- [ ] **Step 3: Commit**

```bash
git add www/public/test_decorator_flow.php
git commit -m "test(decorator): add end-to-end decorator flow test"
```

---

## Notes for Implementation

1. **The C code in Tasks 6-7 is guide-level, not copy-paste.** Verify all Zend API calls against PHP 8.4 headers (`Zend/zend_attributes.h`, `Zend/zend_observer.h`). Key things to check:
   - Use `ZEND_HASH_FOREACH_PTR` (not `ZEND_HASH_PACKED_FOREACH_PTR`) for attribute hash tables
   - Use `zend_declare_typed_property()` with `ZEND_ACC_READONLY` instead of `zend_declare_property_string()` — the latter with `ZEND_ACC_READONLY` causes a fatal error in PHP 8.2+

2. **PHP decorator `before()`/`after()` dispatch** (TODO comments in observer begin/end) is the most complex C code. Implementation steps:
   - Create `OxPHP\Decorator\Context` instance via `object_init_ex()`
   - Populate readonly properties via internal setter (before `ZEND_ACC_READONLY` takes effect)
   - Store `execute_data` pointer as internal property for lazy `getParams()`
   - Call `zend_call_method_with_1_params()` for `before()` / `after()`
   - Track `ctx->decorator_count` — if `before()` throws, call `after()` on previously-succeeded decorators in reverse (cleanup semantics)
   - In `after()`: store `retval` pointer before calling PHP methods so `getResult()`/`hasResult()` work

3. **`oxphp_register_decorator()` completion** — the TODO in Task 7 Step 3 needs to call `register_php_decorator_callback` from `dispatch.rs` through the bridge. Add a C bridge function `oxphp_bridge_register_php_decorator()` that calls the Rust callback.

4. **`Context.getParams()` lazy method**: store `execute_data` pointer in an internal zval property (invisible to PHP). On first `getParams()` call, iterate `ZEND_CALL_ARG(execute_data, n)` for `n = 1..ZEND_NUM_ARGS()` and build array. Cache the array for subsequent calls.

5. **`Context.getResult()` / `hasResult()`**: the `retval` pointer is only available in end handler. Before calling PHP `after()` methods, store `retval` as internal property on Context. `getResult()` returns it; `hasResult()` returns `retval != NULL && !EG(exception)`.

6. **RSHUTDOWN resolution cache**: add a bridge call `oxphp_bridge_decorator_clear_cache()` in `PHP_RSHUTDOWN_FUNCTION(oxphp_sapi)` that calls `DecoratorRegistry::clear_cache()`. This prevents unbounded cache growth in traditional (non-worker) mode.

7. **Repeatable attributes**: the current `resolve()` implementation handles these naturally — each `zend_attribute` in the hash table is a separate entry, and the cache key `(fn_id, decorator_index)` distinguishes instances. Verify this works during integration testing.

8. **`cache_key` in `ResolvedDecorator::Php`**: use `index as u64` alone (scoped to the fn_id's resolved vec), not a composite key with fn_id shift — avoids overflow on 64-bit pointers.
