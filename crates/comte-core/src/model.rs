//! Core data model shared by every engine and by the UI.

use std::fmt;
use std::ops::Range;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

/// How bad an entry is. Ordered from least to most severe, so filters such as
/// "severity >= Warning" are a plain comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Severity {
    #[default]
    Info,
    Warning,
    Error,
    Exception,
    Crash,
}

impl Severity {
    pub const ALL: [Severity; 5] = [
        Severity::Info,
        Severity::Warning,
        Severity::Error,
        Severity::Exception,
        Severity::Crash,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "Info",
            Severity::Warning => "Warning",
            Severity::Error => "Error",
            Severity::Exception => "Exception",
            Severity::Crash => "Crash",
        }
    }

    /// Error, Exception and Crash all count as "errors" in summaries.
    pub fn is_error_like(self) -> bool {
        self >= Severity::Error
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

// ---------------------------------------------------------------------------
// Category
// ---------------------------------------------------------------------------

/// What produced an entry. Drives filtering and the noise policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    UserLog,
    Exception,
    Compiler,
    Crash,
    AssetPipeline,
    DomainReload,
    PlayMode,
    SceneLoad,
    Memory,
    PackageManager,
    Licensing,
    Build,
    Shader,
    Graphics,
    Input,
    System,
    Other,
}

impl Category {
    pub const ALL: [Category; 17] = [
        Category::UserLog,
        Category::Exception,
        Category::Compiler,
        Category::Crash,
        Category::AssetPipeline,
        Category::DomainReload,
        Category::PlayMode,
        Category::SceneLoad,
        Category::Memory,
        Category::PackageManager,
        Category::Licensing,
        Category::Build,
        Category::Shader,
        Category::Graphics,
        Category::Input,
        Category::System,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::UserLog => "User log",
            Category::Exception => "Exception",
            Category::Compiler => "Compiler",
            Category::Crash => "Crash",
            Category::AssetPipeline => "Asset pipeline",
            Category::DomainReload => "Domain reload",
            Category::PlayMode => "Play mode",
            Category::SceneLoad => "Scene load",
            Category::Memory => "Memory",
            Category::PackageManager => "Package Manager",
            Category::Licensing => "Licensing",
            Category::Build => "Build",
            Category::Shader => "Shader",
            Category::Graphics => "Graphics",
            Category::Input => "Input",
            Category::System => "System",
            Category::Other => "Other",
        }
    }

    /// Engine chatter that is hidden by default ("Show engine noise" reveals it).
    pub fn is_noise(self) -> bool {
        matches!(
            self,
            Category::Licensing | Category::PackageManager | Category::Input | Category::Memory
        )
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

// ---------------------------------------------------------------------------
// Source locations and stack frames
// ---------------------------------------------------------------------------

/// A location in source code, e.g. `Assets/Game/Player.cs:42:13`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceRef {
    /// Path as written in the log (often project-relative, e.g. `Assets/...`).
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl SourceRef {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line: None,
            column: None,
        }
    }

    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    pub fn with_column(mut self, column: u32) -> Self {
        self.column = Some(column);
        self
    }

    /// The file name without directories, for compact display (`Player.cs:42`).
    pub fn file_name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }

    /// Compact `File.cs:42` form used on entry rows.
    pub fn short(&self) -> String {
        match self.line {
            Some(line) => format!("{}:{line}", self.file_name()),
            None => self.file_name().to_owned(),
        }
    }
}

impl fmt::Display for SourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path)?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
            if let Some(column) = self.column {
                write!(f, ":{column}")?;
            }
        }
        Ok(())
    }
}

/// Who owns the code in a stack frame. Decides how the UI renders it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FrameKind {
    /// The game's own code (e.g. under `Assets/`). Rendered bold.
    User,
    /// Third-party packages (e.g. `Packages/`, `Library/PackageCache`).
    Package,
    /// Engine and runtime frames (`UnityEngine.*`, `System.*`). Folded by default.
    #[default]
    Engine,
    /// Native frames from crash dumps.
    Native,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackFrame {
    /// Cleaned, human-readable signature, e.g. `GameInstaller.Report(TextData)`.
    pub text: String,
    /// Full original signature (namespaces, raw argument types) for tooltips.
    pub raw: String,
    pub location: Option<SourceRef>,
    pub kind: FrameKind,
}

impl StackFrame {
    pub fn new(text: impl Into<String>, kind: FrameKind) -> Self {
        let text = text.into();
        Self {
            raw: text.clone(),
            text,
            location: None,
            kind,
        }
    }

    pub fn with_raw(mut self, raw: impl Into<String>) -> Self {
        self.raw = raw.into();
        self
    }

    pub fn with_location(mut self, location: SourceRef) -> Self {
        self.location = Some(location);
        self
    }
}

// ---------------------------------------------------------------------------
// Entries
// ---------------------------------------------------------------------------

/// One logical log entry. A single entry may span many raw lines
/// (message + details + stack trace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Assigned by [`crate::LogDocument`]; parsers can leave it at 0.
    pub id: u64,
    /// Raw lines this entry came from (1-based, half-open).
    pub line_range: Range<usize>,
    /// Innermost timeline section; assigned by [`crate::LogDocument`].
    pub section_id: Option<u32>,
    pub severity: Severity,
    pub category: Category,
    /// First line of the message, cleaned up.
    pub message: String,
    /// Additional message lines (multi-line logs, indented continuation lines).
    pub details: Vec<String>,
    pub stack: Vec<StackFrame>,
    /// Where the log claims it came from, e.g. `(Filename: X Line: N)`.
    pub source: Option<SourceRef>,
    /// Hash used to collapse duplicates; assigned by [`crate::LogDocument`].
    pub fingerprint: u64,
    /// The parser wrote this message for humans ("Domain reload took 1.30 s",
    /// "Entering play mode (#1)"). Summaries stay visible when engine chatter
    /// is hidden.
    pub summary: bool,
    /// When it was logged, for logs with timestamps (milliseconds since the
    /// Unix epoch, UTC). See [`parse_utc_millis`].
    pub time: Option<i64>,
}

impl LogEntry {
    pub fn new(
        severity: Severity,
        category: Category,
        message: impl Into<String>,
        line_range: Range<usize>,
    ) -> Self {
        Self {
            id: 0,
            line_range,
            section_id: None,
            severity,
            category,
            message: message.into(),
            details: Vec::new(),
            stack: Vec::new(),
            source: None,
            fingerprint: 0,
            summary: false,
            time: None,
        }
    }

    /// Marks this entry as a human-written summary.
    pub fn as_summary(mut self) -> Self {
        self.summary = true;
        self
    }

    /// Engine output a reader rarely needs: Info entries that are neither the
    /// game's own logs nor summaries, plus everything in a noise category.
    /// Hidden unless "Show engine noise" is on.
    pub fn is_engine_chatter(&self) -> bool {
        if self.category.is_noise() {
            return self.severity < Severity::Warning;
        }
        self.severity == Severity::Info && !self.summary && self.category != Category::UserLog
    }

    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }

    pub fn with_stack(mut self, stack: Vec<StackFrame>) -> Self {
        self.stack = stack;
        self
    }

    pub fn with_source(mut self, source: SourceRef) -> Self {
        self.source = Some(source);
        self
    }

    /// The most useful location to show on the entry row: the first frame in
    /// the game's own code, falling back to the location the log reported.
    pub fn culprit(&self) -> Option<&SourceRef> {
        self.stack
            .iter()
            .find(|frame| frame.kind == FrameKind::User && frame.location.is_some())
            .and_then(|frame| frame.location.as_ref())
            .or(self.source.as_ref())
    }

    pub fn user_frames(&self) -> impl Iterator<Item = &StackFrame> {
        self.stack.iter().filter(|f| f.kind == FrameKind::User)
    }

    /// Short "where" label: `File.cs:42`, or for release builds without paths
    /// the first frame of the game's own code (`Gun.StopShooting()`).
    pub fn culprit_label(&self) -> Option<String> {
        self.culprit()
            .map(SourceRef::short)
            .or_else(|| self.user_frames().next().map(|f| f.text.clone()))
    }

    /// A short headline: the exception type for exceptions
    /// (`NullReferenceException`), otherwise the message cut to `max` chars.
    pub fn headline(&self, max: usize) -> String {
        // Rich-text tags (`<color=cyan>`) are styling, not content.
        let message = crate::markup::plain(&self.message);
        if let Some((name, _)) = message.split_once(": ") {
            let short = name.rsplit('.').next().unwrap_or(name);
            // A specific type says what happened; a bare `Exception` doesn't,
            // so it keeps its message.
            if short.ends_with("Exception") && short != "Exception" && !short.contains(' ') {
                return short.to_owned();
            }
        }
        let message = message.trim();
        if message.chars().count() <= max {
            message.to_owned()
        } else {
            let cut: String = message.chars().take(max.saturating_sub(1)).collect();
            format!("{}…", cut.trim_end())
        }
    }

    /// Number of raw lines this entry spans.
    pub fn line_count(&self) -> usize {
        self.line_range.len()
    }
}

// ---------------------------------------------------------------------------
// Timeline sections
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionKind {
    Startup,
    Compile,
    DomainReload,
    AssetRefresh,
    PlayMode,
    SceneLoad,
    Build,
    Shutdown,
    Crash,
}

impl SectionKind {
    pub fn label(self) -> &'static str {
        match self {
            SectionKind::Startup => "Startup",
            SectionKind::Compile => "Script compilation",
            SectionKind::DomainReload => "Domain reload",
            SectionKind::AssetRefresh => "Asset refresh",
            SectionKind::PlayMode => "Play mode",
            SectionKind::SceneLoad => "Scene load",
            SectionKind::Build => "Build",
            SectionKind::Shutdown => "Shutdown",
            SectionKind::Crash => "Crash",
        }
    }
}

/// Per-severity counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SeverityCounts {
    pub info: u32,
    pub warning: u32,
    pub error: u32,
    pub exception: u32,
    pub crash: u32,
}

impl SeverityCounts {
    pub fn add(&mut self, severity: Severity) {
        *self.slot(severity) += 1;
    }

    pub fn get(&self, severity: Severity) -> u32 {
        match severity {
            Severity::Info => self.info,
            Severity::Warning => self.warning,
            Severity::Error => self.error,
            Severity::Exception => self.exception,
            Severity::Crash => self.crash,
        }
    }

    /// Errors + exceptions + crashes.
    pub fn errors(&self) -> u32 {
        self.error + self.exception + self.crash
    }

    pub fn total(&self) -> u32 {
        self.info + self.warning + self.errors()
    }

    pub fn merge(&mut self, other: &SeverityCounts) {
        for severity in Severity::ALL {
            *self.slot(severity) += other.get(severity);
        }
    }

    fn slot(&mut self, severity: Severity) -> &mut u32 {
        match severity {
            Severity::Info => &mut self.info,
            Severity::Warning => &mut self.warning,
            Severity::Error => &mut self.error,
            Severity::Exception => &mut self.exception,
            Severity::Crash => &mut self.crash,
        }
    }
}

/// A collapsible block of the timeline ("Play mode #2", "Domain reload", ...).
/// Sections can nest: a domain reload inside a play mode session, for example.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub id: u32,
    pub parent: Option<u32>,
    /// 0 for top-level sections.
    pub depth: u16,
    pub kind: SectionKind,
    pub title: String,
    /// Raw lines covered (1-based, half-open). Grows while the section is open.
    pub line_range: Range<usize>,
    pub duration: Option<Duration>,
    /// Counts of all entries inside this section, including nested sections.
    pub counts: SeverityCounts,
    pub closed: bool,
}

// ---------------------------------------------------------------------------
// Facts and metrics
// ---------------------------------------------------------------------------

/// Well-known facts shown on the info card.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FactKey {
    EngineVersion,
    Os,
    PhysicalMemory,
    Cpu,
    Gpu,
    GraphicsApi,
    Vram,
    GpuDriver,
    ProjectPath,
    /// Install folder of a built game (Player logs).
    GamePath,
    StartDate,
    /// Folder where Unity's crash handler wrote a crash report.
    CrashReport,
    /// A mod or plugin a mod loader loaded (one fact per mod).
    Mod,
    Other(String),
}

impl FactKey {
    pub fn label(&self) -> &str {
        match self {
            FactKey::EngineVersion => "Engine version",
            FactKey::Os => "OS",
            FactKey::PhysicalMemory => "Memory",
            FactKey::Cpu => "CPU",
            FactKey::Gpu => "GPU",
            FactKey::GraphicsApi => "Graphics API",
            FactKey::Vram => "VRAM",
            FactKey::GpuDriver => "GPU driver",
            FactKey::ProjectPath => "Project",
            FactKey::GamePath => "Game",
            FactKey::StartDate => "Started",
            FactKey::CrashReport => "Crash report",
            FactKey::Mod => "Mods",
            FactKey::Other(name) => name,
        }
    }
}

/// A piece of environment information found in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub key: FactKey,
    pub value: String,
    pub line: usize,
}

impl Fact {
    pub fn new(key: FactKey, value: impl Into<String>, line: usize) -> Self {
        Self {
            key,
            value: value.into(),
            line,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetricKind {
    DomainReload,
    AssetRefresh,
    Compile,
    Startup,
    SceneLoad,
    SceneUnload,
    UnusedAssetsUnloaded,
    LoadedObjects,
}

impl MetricKind {
    pub fn label(self) -> &'static str {
        match self {
            MetricKind::DomainReload => "Domain reload",
            MetricKind::AssetRefresh => "Asset refresh",
            MetricKind::Compile => "Script compilation",
            MetricKind::Startup => "Startup",
            MetricKind::SceneLoad => "Scene load",
            MetricKind::SceneUnload => "Scene unload",
            MetricKind::UnusedAssetsUnloaded => "Unused assets unloaded",
            MetricKind::LoadedObjects => "Loaded objects",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricValue {
    Duration(Duration),
    Count(u64),
}

/// A measurement extracted from the log (timings, object counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metric {
    pub kind: MetricKind,
    pub value: MetricValue,
    pub line: usize,
}

impl Metric {
    pub fn duration(kind: MetricKind, duration: Duration, line: usize) -> Self {
        Self {
            kind,
            value: MetricValue::Duration(duration),
            line,
        }
    }

    pub fn count(kind: MetricKind, count: u64, line: usize) -> Self {
        Self {
            kind,
            value: MetricValue::Count(count),
            line,
        }
    }

    pub fn as_duration(&self) -> Option<Duration> {
        match self.value {
            MetricValue::Duration(d) => Some(d),
            MetricValue::Count(_) => None,
        }
    }
}

/// `2022-04-26T08:59:29.580Z` → milliseconds since the Unix epoch (UTC).
/// Fractions of any length are accepted; anything else is `None`.
pub fn parse_utc_millis(text: &str) -> Option<i64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (d.next()??, d.next()??, d.next()??);
    let (hms, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut t = hms.split(':').map(|p| p.parse::<i64>().ok());
    let (hour, minute, second) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let millis = match fraction.len() {
        0 => 0,
        _ => {
            let digits: String = fraction.chars().chain("000".chars()).take(3).collect();
            digits.parse::<i64>().ok()?
        }
    };
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis)
}

/// Human-friendly duration: `850 ms`, `1.30 s`, `2 min 05 s`.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 1.0 {
        let ms = secs * 1000.0;
        if ms < 10.0 && ms.fract() != 0.0 {
            format!("{ms:.1} ms")
        } else {
            format!("{} ms", ms.round() as u64)
        }
    } else if secs < 60.0 {
        format!("{secs:.2} s")
    } else {
        let total = secs.round() as u64;
        format!("{} min {:02} s", total / 60, total % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_utc_timestamps() {
        assert_eq!(parse_utc_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_utc_millis("2022-04-26T08:59:29.580Z"),
            Some(1_650_963_569_580)
        );
        assert_eq!(
            parse_utc_millis("2000-03-01T00:00:00.5Z"),
            Some(951_868_800_500)
        );
        assert_eq!(parse_utc_millis("2022-04-26T08:59:29"), None, "needs the Z");
        assert_eq!(parse_utc_millis("2022-13-26T08:59:29Z"), None);
    }

    #[test]
    fn formats_durations_for_humans() {
        assert_eq!(format_duration(Duration::from_micros(1592)), "1.6 ms");
        assert_eq!(format_duration(Duration::from_millis(205)), "205 ms");
        assert_eq!(format_duration(Duration::from_millis(1304)), "1.30 s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2 min 05 s");
    }

    #[test]
    fn severity_is_ordered_for_threshold_filters() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert!(Severity::Error < Severity::Exception);
        assert!(Severity::Exception < Severity::Crash);
        assert!(!Severity::Warning.is_error_like());
        assert!(Severity::Exception.is_error_like());
    }

    #[test]
    fn noise_categories() {
        assert!(Category::Licensing.is_noise());
        assert!(Category::PackageManager.is_noise());
        assert!(!Category::UserLog.is_noise());
        assert!(!Category::Compiler.is_noise());
        assert_eq!(
            Category::ALL.iter().filter(|c| c.is_noise()).count(),
            4,
            "noise policy changed; update docs/DESIGN.md §3"
        );
    }

    #[test]
    fn source_ref_formatting() {
        let r = SourceRef::new("Assets/Game/Player.cs")
            .with_line(42)
            .with_column(13);
        assert_eq!(r.to_string(), "Assets/Game/Player.cs:42:13");
        assert_eq!(r.short(), "Player.cs:42");
        assert_eq!(SourceRef::new(r"C:\a\b\Debug.h").file_name(), "Debug.h");
    }

    #[test]
    fn culprit_prefers_first_user_frame_over_reported_source() {
        let entry = LogEntry::new(Severity::Error, Category::UserLog, "boom", 10..15)
            .with_source(SourceRef::new("Runtime/Debug.bindings.h").with_line(35))
            .with_stack(vec![
                StackFrame::new("UnityEngine.Debug.LogError(object)", FrameKind::Engine),
                StackFrame::new("Enemy.Spawn()", FrameKind::User)
                    .with_location(SourceRef::new("Assets/Enemy.cs").with_line(57)),
                StackFrame::new("Spawner.Tick()", FrameKind::User)
                    .with_location(SourceRef::new("Assets/Spawner.cs").with_line(9)),
            ]);
        assert_eq!(entry.culprit().unwrap().short(), "Enemy.cs:57");
        assert_eq!(entry.user_frames().count(), 2);
        assert_eq!(entry.line_count(), 5);
    }

    #[test]
    fn engine_chatter_rules() {
        let chatter = LogEntry::new(Severity::Info, Category::DomainReload, "Mono: x", 1..2);
        assert!(chatter.is_engine_chatter());
        assert!(!chatter.clone().as_summary().is_engine_chatter());
        let user = LogEntry::new(Severity::Info, Category::UserLog, "hi", 1..2);
        assert!(!user.is_engine_chatter());
        let warn = LogEntry::new(Severity::Warning, Category::Other, "careful", 1..2);
        assert!(!warn.is_engine_chatter());
        let licensing_summary =
            LogEntry::new(Severity::Info, Category::Memory, "Unloaded 3 assets", 1..2).as_summary();
        assert!(
            licensing_summary.is_engine_chatter(),
            "noise categories stay hidden"
        );
        let noisy_error = LogEntry::new(
            Severity::Error,
            Category::Licensing,
            "license expired",
            1..2,
        );
        assert!(!noisy_error.is_engine_chatter(), "problems are never noise");
    }

    #[test]
    fn culprit_falls_back_to_reported_source() {
        let entry = LogEntry::new(Severity::Info, Category::UserLog, "hi", 1..2)
            .with_source(SourceRef::new("Assets/Flow.cs").with_line(26));
        assert_eq!(entry.culprit().unwrap().short(), "Flow.cs:26");
    }

    #[test]
    fn severity_counts() {
        let mut a = SeverityCounts::default();
        a.add(Severity::Warning);
        a.add(Severity::Error);
        a.add(Severity::Crash);
        let mut b = SeverityCounts::default();
        b.add(Severity::Exception);
        b.merge(&a);
        assert_eq!(b.errors(), 3);
        assert_eq!(b.total(), 4);
        assert_eq!(b.get(Severity::Warning), 1);
    }
}
