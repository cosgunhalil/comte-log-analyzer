//! # comte-core
//!
//! Engine-agnostic building blocks for Comte Logger:
//!
//! - [`model`]: entries, stack frames, timeline sections, facts and metrics.
//! - [`parser`]: the streaming [`LogParser`] trait and the [`ParseEvent`]s it emits.
//! - [`document`]: [`LogDocument`], which turns parse events into a navigable document.
//! - [`engine`]: the [`EngineSupport`] / [`LogLocator`] traits each engine crate implements.
//! - [`source`]: descriptions of log files on disk ([`LogSource`]).
//! - [`reader`]: lossy, line-oriented file reading that feeds a parser.
//! - [`fingerprint`]: stable hashes used to collapse duplicate entries.
//!
//! Line numbers are **1-based** everywhere; line ranges are half-open (`start..end`).

pub mod document;
pub mod engine;
pub mod fingerprint;
pub mod model;
pub mod parser;
pub mod reader;
pub mod source;

pub use document::LogDocument;
pub use engine::{EngineSupport, LogLocator};
pub use model::{
    Category, Fact, FactKey, FrameKind, LogEntry, Metric, MetricKind, MetricValue, Section,
    SectionKind, Severity, SeverityCounts, SourceRef, StackFrame,
};
pub use parser::{LogParser, ParseEvent, PlainTextParser, SectionStart};
pub use reader::{LineReader, parse_file, parse_reader};
pub use source::{EngineId, LogKind, LogSource, SourceRole};
