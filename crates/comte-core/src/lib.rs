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
//! - [`session`]: the saved-session file format (header with note and tags).
//! - [`redact`]: masks user names, tokens, ids, … before saving or sharing.
//!
//! Line numbers are **1-based** everywhere; line ranges are half-open (`start..end`).

pub mod document;
pub mod engine;
pub mod fingerprint;
pub mod model;
pub mod parser;
pub mod reader;
pub mod redact;
pub mod session;
pub mod source;

pub use document::LogDocument;
pub use engine::{DETECT_LINES, EngineSupport, LogLocator, detect_engine};
pub use model::{
    Category, Fact, FactKey, FrameKind, LogEntry, Metric, MetricKind, MetricValue, Section,
    SectionKind, Severity, SeverityCounts, SourceRef, StackFrame, format_duration,
};
pub use parser::{LogParser, ParseEvent, PlainTextParser, SectionStart};
pub use reader::{LineReader, parse_file, parse_reader, read_head};
pub use redact::{RedactionReport, Redactor, redact_lines};
pub use session::{
    SessionContent, SessionHeader, kind_token, parse_header, parse_kind_token, parse_tags,
    write_session,
};
pub use source::{EngineId, LogKind, LogSource, SourceRole};
