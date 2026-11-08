//! # comte-unity
//!
//! Unity support for Comte Logger.
//!
//! - [`detect`]: tells an Editor log from a Player log by its first lines.
//! - Editor parser: M2. Player parser: M3. Log locator: M4
//!   (see `docs/DESIGN.md` §4 and §11).

pub mod detect;

pub use detect::detect_kind;
