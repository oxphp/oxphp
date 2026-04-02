use crate::plugin::php::{PhpError, PluginNativeFunction};
use crate::plugin::types::{MagicMethod, Modifiers, PhpType, PhpValue, Visibility};
use crate::plugin::PluginError;

// ─── PhpParamDef ─────────────────────────────────────────────────────────────

/// Parameter definition for a PHP function or method.
#[derive(Debug, Clone)]
pub struct PhpParamDef {
    pub name: String,
    pub php_type: PhpType,
    pub required: bool,
    pub is_variadic: bool,
    pub default: Option<PhpValue>,
}

impl PhpParamDef {
    /// Create a required parameter.
    pub fn required(name: impl Into<String>, php_type: PhpType) -> Self {
        Self {
            name: name.into(),
            php_type,
            required: true,
            is_variadic: false,
            default: None,
        }
    }

    /// Create an optional parameter with a default value.
    pub fn optional(name: impl Into<String>, php_type: PhpType, default: PhpValue) -> Self {
        Self {
            name: name.into(),
            php_type,
            required: false,
            is_variadic: false,
            default: Some(default),
        }
    }

    /// Create a variadic parameter.
    pub fn variadic(name: impl Into<String>, php_type: PhpType) -> Self {
        Self {
            name: name.into(),
            php_type,
            required: false,
            is_variadic: true,
            default: None,
        }
    }
}

// ─── PhpPropertyDef ──────────────────────────────────────────────────────────

/// Property definition for a PHP class.
#[derive(Debug, Clone)]
pub struct PhpPropertyDef {
    pub name: String,
    pub php_type: PhpType,
    pub visibility: Visibility,
    pub modifiers: Modifiers,
    pub default: Option<PhpValue>,
}

// ─── PhpConstantDef ──────────────────────────────────────────────────────────

/// Constant definition for a PHP class or interface.
#[derive(Debug, Clone)]
pub struct PhpConstantDef {
    pub name: String,
    pub value: PhpValue,
    pub visibility: Visibility,
}

// ─── PhpMethodDef ────────────────────────────────────────────────────────────

/// Method definition for a PHP class or interface.
pub struct PhpMethodDef {
    pub name: String,
    pub visibility: Visibility,
    pub modifiers: Modifiers,
    pub is_async: bool,
    pub is_variadic: bool,
    pub params: Vec<PhpParamDef>,
    pub return_type: Option<PhpType>,
    pub handler: Option<Box<dyn PluginNativeFunction>>,
}

impl PhpMethodDef {
    /// Create a new method definition with defaults:
    /// Public visibility, empty modifiers, not async, not variadic, no params, no return type, no handler.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            visibility: Visibility::Public,
            modifiers: Modifiers::empty(),
            is_async: false,
            is_variadic: false,
            params: Vec::new(),
            return_type: None,
            handler: None,
        }
    }

    /// Count of required parameters.
    pub fn required_params(&self) -> usize {
        self.params.iter().filter(|p| p.required).count()
    }

    /// Total number of parameters.
    pub fn total_params(&self) -> usize {
        self.params.len()
    }
}

// ─── MagicHandler ────────────────────────────────────────────────────────────

/// Handler for a PHP magic method invocation.
pub type MagicHandler =
    Box<dyn Fn(&mut crate::bridge::call::NativeCall) -> Result<(), PhpError> + Send + Sync>;

// ─── PhpClassDef ─────────────────────────────────────────────────────────────

/// Class definition registered by a plugin.
pub struct PhpClassDef {
    pub fqn: String,
    pub plugin_name: String,
    pub parent: Option<String>,
    pub interfaces: Vec<String>,
    pub modifiers: Modifiers,
    pub properties: Vec<PhpPropertyDef>,
    pub constants: Vec<PhpConstantDef>,
    pub methods: Vec<PhpMethodDef>,
    pub magic_handlers: [Option<MagicHandler>; MagicMethod::COUNT],
    pub has_custom_storage: bool,
    pub storage_factory: Option<Box<dyn Fn() -> *mut std::ffi::c_void + Send + Sync>>,
    pub storage_drop: Option<Box<dyn Fn(*mut std::ffi::c_void) + Send + Sync>>,
    pub storage_clone:
        Option<Box<dyn Fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void + Send + Sync>>,
}

impl PhpClassDef {
    /// Create a new class definition with all fields defaulted/empty.
    pub fn new(fqn: impl Into<String>) -> Self {
        Self {
            fqn: fqn.into(),
            plugin_name: String::new(),
            parent: None,
            interfaces: Vec::new(),
            modifiers: Modifiers::empty(),
            properties: Vec::new(),
            constants: Vec::new(),
            methods: Vec::new(),
            magic_handlers: core::array::from_fn(|_| None),
            has_custom_storage: false,
            storage_factory: None,
            storage_drop: None,
            storage_clone: None,
        }
    }
}

// ─── PhpInterfaceDef ─────────────────────────────────────────────────────────

/// Interface definition registered by a plugin.
pub struct PhpInterfaceDef {
    pub fqn: String,
    pub plugin_name: String,
    pub parent: Option<String>,
    pub constants: Vec<PhpConstantDef>,
    pub methods: Vec<PhpMethodDef>,
}

impl PhpInterfaceDef {
    /// Create a new interface definition with all fields defaulted/empty.
    pub fn new(fqn: impl Into<String>) -> Self {
        Self {
            fqn: fqn.into(),
            plugin_name: String::new(),
            parent: None,
            constants: Vec::new(),
            methods: Vec::new(),
        }
    }
}

// ─── PhpEnumCaseDef ──────────────────────────────────────────────────────────

/// Enum case definition.
#[derive(Debug, Clone)]
pub struct PhpEnumCaseDef {
    pub name: String,
    pub value: Option<PhpValue>,
}

// ─── PhpEnumDef ──────────────────────────────────────────────────────────────

/// Enum definition registered by a plugin.
pub struct PhpEnumDef {
    pub fqn: String,
    pub plugin_name: String,
    pub backing_type: Option<PhpType>,
    pub interfaces: Vec<String>,
    pub cases: Vec<PhpEnumCaseDef>,
    pub constants: Vec<PhpConstantDef>,
    pub methods: Vec<PhpMethodDef>,
}

impl PhpEnumDef {
    /// Create a new enum definition with all fields defaulted/empty.
    pub fn new(fqn: impl Into<String>) -> Self {
        Self {
            fqn: fqn.into(),
            plugin_name: String::new(),
            backing_type: None,
            interfaces: Vec::new(),
            cases: Vec::new(),
            constants: Vec::new(),
            methods: Vec::new(),
        }
    }
}

// ─── PhpAttributeDef ─────────────────────────────────────────────────────────

/// Attribute definition registered by a plugin.
pub struct PhpAttributeDef {
    pub fqn: String,
    pub plugin_name: String,
    /// Bitmask of valid targets. Defaults to `0x3F` (ALL targets).
    pub targets: u32,
    pub repeatable: bool,
    pub params: Vec<PhpParamDef>,
    pub properties: Vec<PhpPropertyDef>,
}

impl PhpAttributeDef {
    /// Create a new attribute definition. `targets` defaults to `0x3F` (ALL).
    pub fn new(fqn: impl Into<String>) -> Self {
        Self {
            fqn: fqn.into(),
            plugin_name: String::new(),
            targets: 0x3F,
            repeatable: false,
            params: Vec::new(),
            properties: Vec::new(),
        }
    }
}

// ─── PhpFunctionDef ──────────────────────────────────────────────────────────

/// Free function definition registered by a plugin.
pub struct PhpFunctionDef {
    pub fqn: String,
    pub plugin_name: String,
    pub params: Vec<PhpParamDef>,
    pub return_type: Option<PhpType>,
    pub is_variadic: bool,
    pub handler: Option<Box<dyn PluginNativeFunction>>,
}

impl PhpFunctionDef {
    /// Create a new function definition with all fields defaulted/empty.
    pub fn new(fqn: impl Into<String>) -> Self {
        Self {
            fqn: fqn.into(),
            plugin_name: String::new(),
            params: Vec::new(),
            return_type: None,
            is_variadic: false,
            handler: None,
        }
    }

    /// Count of required parameters.
    pub fn required_params(&self) -> usize {
        self.params.iter().filter(|p| p.required).count()
    }

    /// Total number of parameters.
    pub fn total_params(&self) -> usize {
        self.params.len()
    }
}

// ─── PhpDefinitions ──────────────────────────────────────────────────────────

/// Aggregate of all PHP definitions contributed by a plugin.
#[derive(Default)]
pub struct PhpDefinitions {
    pub classes: Vec<PhpClassDef>,
    pub interfaces: Vec<PhpInterfaceDef>,
    pub enums: Vec<PhpEnumDef>,
    pub attributes: Vec<PhpAttributeDef>,
    pub functions: Vec<PhpFunctionDef>,
}


// ─── Topological Sort ────────────────────────────────────────────────────────

/// Sort class definitions topologically by parent dependency.
///
/// Uses Kahn's algorithm. External parents (not in `classes`) are ignored.
/// Returns the indices of `classes` in topological order (parents before children).
/// Returns `Err` if a cycle is detected.
pub fn topological_sort_classes(classes: &[PhpClassDef]) -> Result<Vec<usize>, PluginError> {
    let n = classes.len();

    // Map fqn → index for fast parent lookup.
    let mut fqn_to_idx: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::with_capacity(n);
    for (i, cls) in classes.iter().enumerate() {
        fqn_to_idx.insert(cls.fqn.as_str(), i);
    }

    // Build in-degree and adjacency list (parent → children).
    let mut in_degree = vec![0usize; n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];

    for (i, cls) in classes.iter().enumerate() {
        if let Some(parent_fqn) = &cls.parent {
            if let Some(&parent_idx) = fqn_to_idx.get(parent_fqn.as_str()) {
                // parent_idx must be processed before i
                children[parent_idx].push(i);
                in_degree[i] += 1;
            }
            // External parents are ignored — no dependency added.
        }
    }

    // Kahn's BFS: start with all nodes that have no internal parent dependency.
    let mut queue: std::collections::VecDeque<usize> = (0..n)
        .filter(|&i| in_degree[i] == 0)
        .collect();

    let mut result = Vec::with_capacity(n);
    while let Some(idx) = queue.pop_front() {
        result.push(idx);
        for &child in &children[idx] {
            in_degree[child] -= 1;
            if in_degree[child] == 0 {
                queue.push_back(child);
            }
        }
    }

    if result.len() != n {
        return Err(PluginError::Config(
            "cycle detected in class parent hierarchy".to_string(),
        ));
    }

    Ok(result)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── PhpDefinitions ──

    #[test]
    fn test_php_definitions_empty() {
        let defs = PhpDefinitions::default();
        assert!(defs.classes.is_empty());
        assert!(defs.interfaces.is_empty());
        assert!(defs.enums.is_empty());
        assert!(defs.attributes.is_empty());
        assert!(defs.functions.is_empty());
    }

    // ── PhpClassDef ──

    #[test]
    fn test_php_class_def_defaults() {
        let cls = PhpClassDef::new("MyPlugin\\MyClass");
        assert_eq!(cls.fqn, "MyPlugin\\MyClass");
        assert_eq!(cls.plugin_name, "");
        assert!(cls.parent.is_none());
        assert!(cls.interfaces.is_empty());
        assert_eq!(cls.modifiers, Modifiers::empty());
        assert!(cls.properties.is_empty());
        assert!(cls.constants.is_empty());
        assert!(cls.methods.is_empty());
        assert!(!cls.has_custom_storage);
        assert!(cls.storage_factory.is_none());
        assert!(cls.storage_drop.is_none());
        assert!(cls.storage_clone.is_none());
        // All magic handlers are None
        for i in 0..MagicMethod::COUNT {
            assert!(cls.magic_handlers[i].is_none());
        }
    }

    // ── PhpInterfaceDef ──

    #[test]
    fn test_php_interface_def_defaults() {
        let iface = PhpInterfaceDef::new("MyPlugin\\Countable");
        assert_eq!(iface.fqn, "MyPlugin\\Countable");
        assert_eq!(iface.plugin_name, "");
        assert!(iface.parent.is_none());
        assert!(iface.constants.is_empty());
        assert!(iface.methods.is_empty());
    }

    // ── PhpEnumDef ──

    #[test]
    fn test_php_enum_def_defaults() {
        let e = PhpEnumDef::new("MyPlugin\\Status");
        assert_eq!(e.fqn, "MyPlugin\\Status");
        assert_eq!(e.plugin_name, "");
        assert!(e.backing_type.is_none());
        assert!(e.interfaces.is_empty());
        assert!(e.cases.is_empty());
        assert!(e.constants.is_empty());
        assert!(e.methods.is_empty());
    }

    // ── PhpAttributeDef ──

    #[test]
    fn test_php_attribute_def_defaults() {
        let attr = PhpAttributeDef::new("MyPlugin\\Route");
        assert_eq!(attr.fqn, "MyPlugin\\Route");
        assert_eq!(attr.plugin_name, "");
        assert_eq!(attr.targets, 0x3F);
        assert!(!attr.repeatable);
        assert!(attr.params.is_empty());
        assert!(attr.properties.is_empty());
    }

    // ── PhpFunctionDef ──

    #[test]
    fn test_php_function_def_defaults() {
        let f = PhpFunctionDef::new("oxphp_my_plugin_hello");
        assert_eq!(f.fqn, "oxphp_my_plugin_hello");
        assert_eq!(f.plugin_name, "");
        assert!(f.params.is_empty());
        assert!(f.return_type.is_none());
        assert!(!f.is_variadic);
        assert!(f.handler.is_none());
        assert_eq!(f.required_params(), 0);
        assert_eq!(f.total_params(), 0);
    }

    // ── PhpMethodDef ──

    #[test]
    fn test_php_method_def_defaults() {
        let m = PhpMethodDef::new("doSomething");
        assert_eq!(m.name, "doSomething");
        assert_eq!(m.visibility, Visibility::Public);
        assert_eq!(m.modifiers, Modifiers::empty());
        assert!(!m.is_async);
        assert!(!m.is_variadic);
        assert!(m.params.is_empty());
        assert!(m.return_type.is_none());
        assert!(m.handler.is_none());
        assert_eq!(m.required_params(), 0);
        assert_eq!(m.total_params(), 0);
    }

    #[test]
    fn test_php_method_def_param_counts() {
        let mut m = PhpMethodDef::new("greet");
        m.params.push(PhpParamDef::required("name", PhpType::String));
        m.params.push(PhpParamDef::optional("title", PhpType::String, PhpValue::Null));
        m.params.push(PhpParamDef::variadic("extras", PhpType::Mixed));
        assert_eq!(m.required_params(), 1);
        assert_eq!(m.total_params(), 3);
    }

    // ── PhpPropertyDef ──

    #[test]
    fn test_php_property_def() {
        let p = PhpPropertyDef {
            name: "count".to_string(),
            php_type: PhpType::Int,
            visibility: Visibility::Protected,
            modifiers: Modifiers::READONLY,
            default: Some(PhpValue::Int(0)),
        };
        assert_eq!(p.name, "count");
        assert_eq!(p.php_type, PhpType::Int);
        assert_eq!(p.visibility, Visibility::Protected);
        assert!(p.modifiers.contains(Modifiers::READONLY));
        assert_eq!(p.default, Some(PhpValue::Int(0)));
    }

    // ── PhpConstantDef ──

    #[test]
    fn test_php_constant_def() {
        let c = PhpConstantDef {
            name: "VERSION".to_string(),
            value: PhpValue::String("1.0.0".to_string()),
            visibility: Visibility::Public,
        };
        assert_eq!(c.name, "VERSION");
        assert_eq!(c.value, PhpValue::String("1.0.0".to_string()));
        assert_eq!(c.visibility, Visibility::Public);
    }

    // ── PhpParamDef ──

    #[test]
    fn test_php_param_def_required() {
        let p = PhpParamDef::required("id", PhpType::Int);
        assert_eq!(p.name, "id");
        assert_eq!(p.php_type, PhpType::Int);
        assert!(p.required);
        assert!(!p.is_variadic);
        assert!(p.default.is_none());
    }

    #[test]
    fn test_php_param_def_optional() {
        let p = PhpParamDef::optional("name", PhpType::String, PhpValue::Null);
        assert_eq!(p.name, "name");
        assert!(!p.required);
        assert!(!p.is_variadic);
        assert_eq!(p.default, Some(PhpValue::Null));
    }

    #[test]
    fn test_php_param_def_variadic() {
        let p = PhpParamDef::variadic("args", PhpType::Mixed);
        assert_eq!(p.name, "args");
        assert!(!p.required);
        assert!(p.is_variadic);
        assert!(p.default.is_none());
    }

    // ── PhpEnumCaseDef ──

    #[test]
    fn test_enum_case_def() {
        let pure = PhpEnumCaseDef {
            name: "Active".to_string(),
            value: None,
        };
        assert_eq!(pure.name, "Active");
        assert!(pure.value.is_none());

        let backed = PhpEnumCaseDef {
            name: "Inactive".to_string(),
            value: Some(PhpValue::Int(0)),
        };
        assert_eq!(backed.name, "Inactive");
        assert_eq!(backed.value, Some(PhpValue::Int(0)));
    }

    // ── topological_sort_classes ──

    fn make_class(fqn: &str, parent: Option<&str>) -> PhpClassDef {
        let mut cls = PhpClassDef::new(fqn);
        cls.parent = parent.map(|s| s.to_string());
        cls
    }

    #[test]
    fn test_topo_sort_no_deps() {
        // Three independent classes — any order is valid, but all must appear.
        let classes = vec![
            make_class("A", None),
            make_class("B", None),
            make_class("C", None),
        ];
        let order = topological_sort_classes(&classes).unwrap();
        assert_eq!(order.len(), 3);
        // All indices present
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2]);
    }

    #[test]
    fn test_topo_sort_linear_chain() {
        // C extends B extends A — must appear as A, B, C (indices 0, 1, 2).
        let classes = vec![
            make_class("A", None),
            make_class("B", Some("A")),
            make_class("C", Some("B")),
        ];
        let order = topological_sort_classes(&classes).unwrap();
        assert_eq!(order.len(), 3);

        let pos: Vec<usize> = order
            .iter()
            .map(|&idx| {
                // position in result for class index idx
                order.iter().position(|&x| x == idx).unwrap()
            })
            .collect();
        // idx 0 (A) must appear before idx 1 (B) which must appear before idx 2 (C)
        let pos_a = order.iter().position(|&x| x == 0).unwrap();
        let pos_b = order.iter().position(|&x| x == 1).unwrap();
        let pos_c = order.iter().position(|&x| x == 2).unwrap();
        let _ = pos; // suppress unused warning
        assert!(pos_a < pos_b, "A must come before B");
        assert!(pos_b < pos_c, "B must come before C");
    }

    #[test]
    fn test_topo_sort_external_parent_ok() {
        // D extends \Exception (external) — treated as root, no error.
        let classes = vec![make_class("D", Some("\\Exception"))];
        let order = topological_sort_classes(&classes).unwrap();
        assert_eq!(order, vec![0]);
    }

    #[test]
    fn test_topo_sort_cycle_error() {
        // A extends B, B extends A — cycle.
        let classes = vec![
            make_class("A", Some("B")),
            make_class("B", Some("A")),
        ];
        let result = topological_sort_classes(&classes);
        assert!(result.is_err(), "Expected cycle error");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }
}
