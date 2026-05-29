pub mod dispatch;
pub mod registry;
pub mod types;

pub use registry::{DecoratorRegistry, PhpDecoratorMeta, ResolvedDecorator};
pub use types::{
    AttrArg, AttrArgs, AttributeTargets, Decorator, DecoratorAction, DecoratorCallContext,
    DecoratorCallResult,
};
