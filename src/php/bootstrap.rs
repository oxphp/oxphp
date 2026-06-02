//! Shared pre-MINIT engine bootstrap, called by every execution frontend.
//!
//! Both the HTTP `serve` path (`main.rs`) and the one-shot `oxphp run` path
//! (`frontend::cli_oneshot`) must hand the same set of plugin-contributed PHP
//! artifacts to Zend *before* `php_module_startup` (MINIT), so OPcache sees
//! them at compile time. Keeping that ordered list in one place means adding a
//! new `take_*()` artifact kind propagates to every frontend at once — instead
//! of silently reaching only whichever path the author happened to edit.

use std::sync::Arc;

use crate::decorator::{dispatch, DecoratorRegistry};
use crate::php::{bindings, sapi};
use crate::plugin::PluginManager;

/// Register all plugin-contributed PHP artifacts with Zend and set the
/// superglobals flag — the full ordered sequence that MUST run after
/// `PluginManager::init_all` and before `php_module_startup` (MINIT).
///
/// Drains `plugin_manager` of its native functions, PHP definitions
/// (classes/interfaces/enums/attributes/functions) and decorators, registering
/// each kind with the SAPI, then installs the decorator dispatch bridge.
///
/// Returns the freshly built [`DecoratorRegistry`] so a caller can inspect the
/// registered count; the bridge already retains its own clone via
/// [`dispatch::install_bridge_callbacks`].
///
/// HTTP request accessors are deliberately **not** registered here — they are
/// an HTTP-frontend concern, not a plugin artifact, and the CLI frontend never
/// serves requests. The `serve` path installs them separately.
pub fn register_plugin_artifacts(
    plugin_manager: &mut PluginManager,
    superglobals_enabled: bool,
) -> Arc<DecoratorRegistry> {
    // Set the superglobals flag before MINIT (read during MINIT and request
    // handling). The CLI frontend forces `true`; serve passes its config value.
    unsafe {
        bindings::oxphp_bridge_set_superglobals_enabled(superglobals_enabled);
    }

    let native_fns = plugin_manager.take_native_php_functions();
    if !native_fns.is_empty() {
        sapi::register_native_plugin_functions(native_fns);
    }

    // Register plugin PHP definitions (classes, interfaces, enums, attributes,
    // functions).
    let php_defs = plugin_manager.take_php_definitions();
    if !php_defs.classes.is_empty()
        || !php_defs.interfaces.is_empty()
        || !php_defs.enums.is_empty()
        || !php_defs.attributes.is_empty()
        || !php_defs.functions.is_empty()
    {
        sapi::register_php_definitions(php_defs);
    }

    // Create the decorator registry — always, even without Rust plugins,
    // because PHP decorators register at runtime via oxphp_register_decorator().
    let registry = Arc::new(DecoratorRegistry::new());
    for def in plugin_manager.take_decorators() {
        registry.register_rust(Arc::from(def.decorator));
    }
    dispatch::install_bridge_callbacks(Arc::clone(&registry));
    registry
}
