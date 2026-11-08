//! # comte-unity
//!
//! Unity support for Comte Logger.
//!
//! - [`detect`]: tells an Editor log from a Player log by its first lines.
//! - [`frames`]: parses, classifies and cleans up stack frames (both Unity's
//!   `Type:Method (args) (at file:line)` form and Mono's `at Type.Method () in file:line`).
//! - [`editor`]: the streaming [`EditorParser`] for `Editor.log` / `Editor-prev.log`.
//!
//! Player parser: M3. Log locator: M4 (see `docs/DESIGN.md` §4 and §11).

/// Declares a lazily compiled, module-private regex.
macro_rules! regex {
    ($name:ident = $pattern:expr) => {
        static $name: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new($pattern).expect("valid regex"));
    };
}

mod common;
pub mod detect;
pub mod editor;
pub mod frames;

pub use detect::detect_kind;
pub use editor::EditorParser;
