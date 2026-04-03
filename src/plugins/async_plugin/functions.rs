use crate::plugin::{PluginContext, PluginError};

/// Register async plugin PHP functions.
///
/// This is a stub — actual function implementations are added in the next task.
pub fn register_functions(_ctx: &mut PluginContext, _enabled: bool) -> Result<(), PluginError> {
    Ok(())
}
