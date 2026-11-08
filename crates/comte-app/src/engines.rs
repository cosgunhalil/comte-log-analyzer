//! The engines Comte Logger understands.

use comte_core::{EngineSupport, LogSource};
use comte_unity::UnitySupport;

/// Every supported engine. Detection tries them in this order.
pub fn engines() -> Vec<Box<dyn EngineSupport>> {
    vec![Box::new(UnitySupport)]
}

/// All logs every engine can find on this machine.
pub fn discover_sources() -> Vec<LogSource> {
    engines()
        .iter()
        .flat_map(|engine| engine.locator().discover())
        .collect()
}
