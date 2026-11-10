//! # comte-core
//!
//! Engine-agnostic building blocks for Comte Log Analyzer:
//!
//! - [`model`]: entries, stack frames, timeline sections, facts and metrics.
//! - [`parser`]: the streaming [`LogParser`] trait and the [`ParseEvent`]s it emits.
//! - [`document`]: [`LogDocument`], which turns parse events into a navigable document.
//! - [`engine`]: the [`EngineSupport`] / [`LogLocator`] traits each engine crate implements.
//! - [`source`]: descriptions of log files on disk ([`LogSource`]).
//! - [`reader`]: lossy, line-oriented file reading that feeds a parser.
//! - [`fingerprint`]: stable hashes used to collapse duplicate entries.
//! - [`markup`]: Unity rich text (`<color=…>`, `<b>`, `<i>`) to styled spans or plain text.
//! - [`insights`]: verdict, most repeated problems, slowest operations.
//! - [`compare`]: what changed between two logs (new/gone problems, timings).
//! - [`session`]: the saved-session file format (header with note and tags).
//! - [`report`]: a pasteable Markdown or Slack report of a log.
//! - [`redact`]: masks user names, tokens, ids, … before saving or sharing.
//! - [`tail`]: incremental parsing for live tail ([`TailDocument`]).
//!
//! Line numbers are **1-based** everywhere; line ranges are half-open (`start..end`).

pub mod activity;
pub mod compare;
pub mod document;
pub mod engine;
pub mod fingerprint;
pub mod insights;
pub mod markup;
pub mod model;
pub mod parser;
pub mod reader;
pub mod redact;
pub mod report;
pub mod session;
pub mod source;
pub mod tail;

pub use activity::{
    AssetImport, BuildReport, Hint, ReloadPhase, ReloadProfile, SizeRow, Unfinished, format_bytes,
    parse_size,
};
pub use compare::{Comparison, FactDiff, ProblemDiff, TimingDiff};
pub use document::LogDocument;
pub use engine::{Companion, DETECT_LINES, EngineSupport, LogLocator, detect_engine};
pub use insights::{Insights, PhaseTotal, Repeated, Timed, TimingTotal, Verdict, VerdictKind};
pub use model::{
    Category, Fact, FactKey, FrameKind, LogEntry, Metric, MetricKind, MetricValue, Section,
    SectionKind, Severity, SeverityCounts, SourceRef, StackFrame, format_duration,
    parse_utc_millis,
};
pub use parser::{LogParser, ParseEvent, PlainTextParser, SectionStart};
pub use reader::{LineReader, parse_file, parse_reader, read_head};
pub use redact::{RedactionReport, Redactor, redact_lines};
pub use report::{ReportInput, ReportStyle, render_report};
pub use session::{
    Pin, SessionContent, SessionHeader, kind_token, parse_header, parse_kind_token, parse_tags,
    write_session,
};
pub use source::{EngineId, LogKind, LogSource, SourceRole};
pub use tail::TailDocument;
