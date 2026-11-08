//! Streaming parser for Unity `Editor.log` (DESIGN §4.1).
//!
//! The parser keeps **one pending entry** open. Following lines either extend
//! it (indented continuation lines, lines of the same "fold" group, stack
//! frames, `(Filename: …)`) or flush it and start something new. Holding one
//! entry back is what lets a `Debug.Log` message line become a user log once
//! the stack trace below it shows up.
//!
//! Timeline sections are tracked on a small stack so nested sections (a domain
//! reload inside a play mode session) are always closed innermost-first.

use std::time::Duration;

use comte_core::{
    Category, Fact, FactKey, LogEntry, LogParser, Metric, MetricKind, ParseEvent, SectionKind,
    SectionStart, Severity, SourceRef, format_duration,
};

use crate::common::{
    file_name, graphics_fact, is_exception_header, millis, parse_filename_line, parse_unloading,
    secs,
};
use crate::frames::{self, ParsedFrame};

regex!(
    COMPILER = r"^(?P<path>.+?\.cs)\((?P<line>\d+),(?P<col>\d+)\): (?P<level>error|warning) (?P<code>CS\d+): (?P<msg>.*)$"
);
regex!(BUILT_FROM = r"^Built from '[^']*' branch; Version is '(?P<version>[^ ']+)");
regex!(OS_LINE = r"^OS: '(?P<os>[^']*)'(?:.*?Physical Memory: (?P<mem>\d+ ?[KMG]B))?");
regex!(DATE = r"^Date: (?P<date>\d{4}-\d{2}-\d{2}T\S+)$");
regex!(PROJECT_PATH = r"^Successfully changed project path to: (?P<path>.+)$");
regex!(DOMAIN_PROFILING = r"^Domain Reload Profiling: (?P<ms>\d+)ms");
regex!(
    ASSET_REFRESH = r"^Asset Pipeline Refresh \(id=[0-9a-fA-F]+\): Total: (?P<secs>[\d.]+) seconds - Initiated by (?P<by>.+)$"
);
regex!(
    TUNDRA = r"^\*\*\* Tundra (?P<what>build success|build failed|requires additional run) \((?P<secs>[\d.]+) seconds\)"
);
regex!(BEE_PROGRESS = r"^\[\s*(?:\d+/\d+|BUSY)\s+\d+s\] ");
regex!(
    COMPILE_REQUESTED =
        r"^\[ScriptCompilation\] Requested script compilation because: (?P<why>.+)$"
);
regex!(PROJECT_LOADED = r"^\[Project\] Loading completed in (?P<secs>[\d.]+) seconds");
regex!(SCENE_OPENING = r"^Opening scene '(?P<scene>[^']+)'");
regex!(SCENE_LOADED = r"^Loaded scene '(?P<scene>[^']+)'");
regex!(SCENE_TOTAL = r"^\s*Total Operation Time:\s+(?P<ms>[\d.]+) ms");
regex!(TAG = r"^\[(?P<tag>[A-Za-z][\w .-]*?)(?:::\w+)?\] ");

/// Line prefixes → (category, fold group). Consecutive lines of the same fold
/// group merge into one entry, so e.g. 40 licensing lines read as one row.
const PREFIXES: &[(&str, Category, &str)] = &[
    ("[Licensing::", Category::Licensing, "licensing"),
    ("[Package Manager]", Category::PackageManager, "upm"),
    ("Package Manager log level", Category::PackageManager, "upm"),
    ("[UnityMemory]", Category::Memory, "unitymemory"),
    ("Unloading ", Category::Memory, "unload"),
    ("Memory consumption went from", Category::Memory, "unload"),
    ("Total: ", Category::Memory, "unload"),
    ("Request Asset Garbage Collect", Category::Memory, "unload"),
    ("<RI>", Category::Input, "input"),
    ("Input System", Category::Input, "input"),
    ("This project uses Input Manager", Category::Input, "input"),
    ("[Physics::", Category::System, "physics"),
    ("[LAYOUT]", Category::System, "layout"),
    ("[MODES]", Category::System, "modes"),
    ("[Subsystems]", Category::System, "subsystems"),
    ("Player connection [", Category::System, "playerconn"),
    ("info: ", Category::System, "aspnet"),
    ("##utp:", Category::System, "utp"),
    ("BatchMode:", Category::System, "header"),
    ("System  architecture", Category::System, "header"),
    ("System architecture", Category::System, "header"),
    ("Process architecture", Category::System, "header"),
    ("Built from '", Category::System, "header"),
    ("OS: '", Category::System, "header"),
    ("Date: ", Category::System, "header"),
    ("Initialize engine version", Category::System, "engine"),
    ("Android Extension -", Category::System, "devices"),
    ("Scanning for USB devices", Category::System, "devices"),
    ("Killing ADB server", Category::System, "devices"),
    ("[D3D12 Device Filter]", Category::Graphics, "gfx"),
    ("GfxDevice:", Category::Graphics, "gfx"),
    ("d3d11:", Category::Graphics, "gfx"),
    ("d3d12:", Category::Graphics, "gfx"),
    ("Mono path[", Category::DomainReload, "mono"),
    ("Mono config path", Category::DomainReload, "mono"),
    ("Initialize mono", Category::DomainReload, "mono"),
    ("Using monoOptions", Category::DomainReload, "mono"),
    ("Mono: ", Category::DomainReload, "mono"),
    ("- Loaded All Assemblies", Category::DomainReload, "mono"),
    ("- Finished resetting", Category::DomainReload, "mono"),
    ("- Completed reload", Category::DomainReload, "mono"),
    ("Registering precompiled", Category::DomainReload, "mono"),
    (
        "Register platform support module",
        Category::DomainReload,
        "mono",
    ),
    ("Registered in ", Category::DomainReload, "mono"),
    ("Native extension for", Category::DomainReload, "mono"),
    ("Symbol file ", Category::DomainReload, "mono"),
    ("Refreshing native plugins", Category::DomainReload, "mono"),
    ("Preloading ", Category::DomainReload, "mono"),
    (
        "ScheduleIndexationOnStartup",
        Category::DomainReload,
        "mono",
    ),
    ("Reloading assemblies after", Category::DomainReload, "mono"),
    ("WorkingDir:", Category::Build, "bee"),
    ("ExitCode:", Category::Build, "bee"),
    ("Rebuilding DAG", Category::Build, "bee"),
    ("Finished compiling graph", Category::Build, "bee"),
    (
        "AssetDatabase: script compilation time",
        Category::Build,
        "bee",
    ),
    ("Compiling shader", Category::Shader, "shader"),
    (
        "Application.AssetDatabase",
        Category::AssetPipeline,
        "import",
    ),
    ("Start importing ", Category::AssetPipeline, "import"),
    ("Starting new worker", Category::AssetPipeline, "workers"),
    ("Worker ready:", Category::AssetPipeline, "workers"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Accepts indented continuation lines and same-fold lines.
    Plain,
    /// A user log or exception collecting stack frames.
    /// `blank` = a blank line was seen; only `(Filename: …)` may follow.
    Stack { frames: bool, blank: bool },
    /// The `COMMAND LINE ARGUMENTS:` block (arguments are not indented).
    CommandLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    None,
    Graphics,
    SceneLoad,
}

#[derive(Debug)]
struct Pending {
    entry: LogEntry,
    mode: Mode,
    fold: Option<String>,
    /// Section to close once this entry is emitted (e.g. the domain reload
    /// section ends after its profiling summary).
    close: Option<(SectionKind, Option<Duration>)>,
    block: Block,
    gfx_api: Option<String>,
    gfx_gpu: Option<String>,
}

impl Pending {
    fn new(entry: LogEntry) -> Self {
        Self {
            entry,
            mode: Mode::Plain,
            fold: None,
            close: None,
            block: Block::None,
            gfx_api: None,
            gfx_gpu: None,
        }
    }

    fn fold(mut self, key: impl Into<String>) -> Self {
        self.fold = Some(key.into());
        self
    }

    fn mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    fn closing(mut self, kind: SectionKind, duration: Option<Duration>) -> Self {
        self.close = Some((kind, duration));
        self
    }

    fn block(mut self, block: Block) -> Self {
        self.block = block;
        self
    }

    fn absorb(&mut self, line_no: usize) {
        self.entry.line_range.end = self.entry.line_range.end.max(line_no + 1);
    }
}

fn entry(severity: Severity, category: Category, message: impl Into<String>, n: usize) -> LogEntry {
    LogEntry::new(severity, category, message, n..n + 1)
}

fn info(category: Category, message: impl Into<String>, n: usize) -> LogEntry {
    entry(Severity::Info, category, message, n)
}

/// Streaming parser for Unity Editor logs. Feed it with
/// [`comte_core::parse_file`] or line by line for live tail.
#[derive(Debug, Default)]
pub struct EditorParser {
    pending: Option<Pending>,
    open: Vec<SectionKind>,
    play_mode_count: u32,
    /// The enter-time backup scene load of the current play session was seen.
    play_scene_loaded: bool,
    started: bool,
}

impl EditorParser {
    pub fn new() -> Self {
        Self::default()
    }

    // -- sections -----------------------------------------------------------

    fn open_section(
        &mut self,
        kind: SectionKind,
        title: String,
        line: usize,
        out: &mut Vec<ParseEvent>,
    ) {
        out.push(ParseEvent::SectionStart(SectionStart { kind, title, line }));
        self.open.push(kind);
    }

    /// Closes `kind` and everything nested inside it. No-op if not open.
    fn close_section(
        &mut self,
        kind: SectionKind,
        line: usize,
        duration: Option<Duration>,
        out: &mut Vec<ParseEvent>,
    ) {
        if !self.open.contains(&kind) {
            return;
        }
        while let Some(open) = self.open.pop() {
            let done = open == kind;
            out.push(ParseEvent::SectionEnd {
                line,
                duration: if done { duration } else { None },
            });
            if done {
                break;
            }
        }
    }

    fn close_all(&mut self, line: usize, out: &mut Vec<ParseEvent>) {
        while self.open.pop().is_some() {
            out.push(ParseEvent::SectionEnd {
                line,
                duration: None,
            });
        }
    }

    fn is_open(&self, kind: SectionKind) -> bool {
        self.open.contains(&kind)
    }

    /// Closes any previous play session (and startup) and opens the next one.
    fn begin_play_mode(&mut self, n: usize, out: &mut Vec<ParseEvent>) {
        let before = n.saturating_sub(1);
        self.close_section(SectionKind::PlayMode, before, None, out);
        self.close_section(SectionKind::Startup, before, None, out);
        self.play_mode_count += 1;
        self.play_scene_loaded = false;
        let title = format!("Play mode #{}", self.play_mode_count);
        self.open_section(SectionKind::PlayMode, title, n, out);
    }

    // -- pending entry ------------------------------------------------------

    fn flush(&mut self, out: &mut Vec<ParseEvent>) {
        if let Some(p) = self.pending.take() {
            let last_line = p.entry.line_range.end.saturating_sub(1);
            out.push(ParseEvent::Entry(p.entry));
            if let Some((kind, duration)) = p.close {
                self.close_section(kind, last_line, duration, out);
            }
        }
    }

    fn start(&mut self, pending: Pending, out: &mut Vec<ParseEvent>) {
        self.flush(out);
        self.pending = Some(pending);
    }

    fn emit(&mut self, entry: LogEntry, out: &mut Vec<ParseEvent>) {
        self.start(Pending::new(entry), out);
    }

    /// For a stack trace starting at line `n`: takes the message line right
    /// above it out of the pending entry (if that entry is unclassified text).
    fn take_message_line(
        &mut self,
        n: usize,
        out: &mut Vec<ParseEvent>,
    ) -> Option<(String, usize)> {
        let p = self.pending.as_mut()?;
        let adjacent = p.entry.line_range.end == n;
        if !(adjacent && p.mode == Mode::Plain && p.entry.category == Category::Other) {
            self.flush(out);
            return None;
        }
        if let Some(last) = p.entry.details.pop() {
            p.entry.line_range.end -= 1;
            self.flush(out);
            return Some((last, n - 1));
        }
        let p = self.pending.take()?;
        Some((p.entry.message, p.entry.line_range.start))
    }

    /// Tries to extend the pending entry with line `n`. Returns `true` if consumed.
    fn continue_pending(&mut self, n: usize, text: &str, out: &mut Vec<ParseEvent>) -> bool {
        let Some(p) = self.pending.as_mut() else {
            return false;
        };

        if let Some(source) = parse_filename_line(text) {
            if p.entry.source.is_none() {
                p.entry.source = source;
            }
            p.absorb(n);
            self.flush(out);
            return true;
        }

        match p.mode {
            Mode::Stack { frames, blank } => {
                if blank {
                    return false;
                }
                if let Some(frame) = frames::parse_frame(text) {
                    apply_frame(&mut p.entry, frame);
                    p.mode = Mode::Stack {
                        frames: true,
                        blank: false,
                    };
                    p.absorb(n);
                    return true;
                }
                if text.starts_with("Rethrow as ") {
                    p.entry.details.push(text.trim().to_owned());
                    p.absorb(n);
                    return true;
                }
                // Exception messages can span lines before the first frame.
                if !frames && p.entry.category == Category::Exception && p.entry.details.len() < 50
                {
                    p.entry.details.push(text.trim().to_owned());
                    p.absorb(n);
                    return true;
                }
                false
            }
            Mode::CommandLine => {
                if text.starts_with("Successfully changed project path to:")
                    || text.starts_with('[')
                    || p.entry.details.len() >= 64
                {
                    return false;
                }
                p.entry.details.push(text.to_owned());
                p.absorb(n);
                true
            }
            Mode::Plain => {
                if text.starts_with([' ', '\t']) {
                    match p.block {
                        Block::Graphics => {
                            if let Some(fact) = graphics_fact(text, n) {
                                match fact.key {
                                    FactKey::GraphicsApi => p.gfx_api = Some(fact.value.clone()),
                                    FactKey::Gpu => p.gfx_gpu = Some(fact.value.clone()),
                                    _ => {}
                                }
                                p.entry.message =
                                    graphics_message(p.gfx_api.as_deref(), p.gfx_gpu.as_deref());
                                out.push(ParseEvent::Fact(fact));
                            }
                        }
                        Block::SceneLoad => {
                            if let Some(d) =
                                SCENE_TOTAL.captures(text).and_then(|c| millis(&c["ms"]))
                            {
                                out.push(ParseEvent::Metric(Metric::duration(
                                    MetricKind::SceneLoad,
                                    d,
                                    n,
                                )));
                                p.entry
                                    .message
                                    .push_str(&format!(" in {}", format_duration(d)));
                            }
                        }
                        Block::None => {}
                    }
                    p.entry.details.push(text.trim().to_owned());
                    p.absorb(n);
                    return true;
                }
                // Lines with their own meaning (metrics, rewritten messages) are
                // never swallowed by a fold group.
                let structural = parse_unloading(text, n).is_some();
                let same_fold = !structural
                    && p.fold
                        .as_deref()
                        .is_some_and(|fold| classify(text).1.as_deref() == Some(fold));
                if same_fold {
                    p.entry.details.push(text.to_owned());
                    p.absorb(n);
                    return true;
                }
                false
            }
        }
    }

    fn on_blank(&mut self, n: usize, out: &mut Vec<ParseEvent>) {
        match self.pending.as_mut() {
            Some(p) if matches!(p.mode, Mode::Stack { .. }) => {
                if let Mode::Stack { frames, .. } = p.mode {
                    p.mode = Mode::Stack {
                        frames,
                        blank: true,
                    };
                }
                p.absorb(n);
            }
            Some(_) => self.flush(out),
            None => {}
        }
    }

    /// A line the pending entry did not take.
    fn on_line(&mut self, n: usize, text: &str, out: &mut Vec<ParseEvent>) {
        // --- stack traces and errors ---------------------------------------
        if let Some(frame) = frames::parse_unity_frame(text) {
            let (message, start) = self
                .take_message_line(n, out)
                .unwrap_or_else(|| ("(no message)".to_owned(), n));
            let mut e = LogEntry::new(Severity::Info, Category::UserLog, message, start..n + 1);
            if is_exception_header(&e.message) {
                e.severity = Severity::Exception;
                e.category = Category::Exception;
            }
            apply_frame(&mut e, frame);
            let p = Pending::new(e).mode(Mode::Stack {
                frames: true,
                blank: false,
            });
            self.start(p, out);
            return;
        }

        if is_exception_header(text) {
            let e = entry(Severity::Exception, Category::Exception, text, n);
            self.start(
                Pending::new(e).mode(Mode::Stack {
                    frames: false,
                    blank: false,
                }),
                out,
            );
            return;
        }

        if let Some(c) = COMPILER.captures(text) {
            let severity = if &c["level"] == "error" {
                Severity::Error
            } else {
                Severity::Warning
            };
            let mut source = SourceRef::new(frames::display_path(&c["path"]).unwrap_or_default());
            if let Ok(line) = c["line"].parse() {
                source = source.with_line(line);
            }
            if let Ok(col) = c["col"].parse() {
                source = source.with_column(col);
            }
            let message = format!("{}: {}", &c["code"], &c["msg"]);
            self.emit(
                entry(severity, Category::Compiler, message, n).with_source(source),
                out,
            );
            return;
        }

        if text.starts_with("Crash!!!")
            || text.starts_with("Received signal")
            || text.starts_with("========== OUTPUTTING STACK TRACE")
        {
            self.flush(out);
            if !self.is_open(SectionKind::Crash) {
                self.open_section(SectionKind::Crash, "Crash".into(), n, out);
            }
            self.emit(entry(Severity::Crash, Category::Crash, text, n), out);
            return;
        }

        // --- timeline markers ---------------------------------------------
        if text.starts_with("Begin MonoManager ReloadAssembly") {
            self.flush(out);
            self.close_section(SectionKind::DomainReload, n.saturating_sub(1), None, out);
            self.open_section(SectionKind::DomainReload, "Domain reload".into(), n, out);
            return;
        }

        if let Some(d) = DOMAIN_PROFILING
            .captures(text)
            .and_then(|c| c["ms"].parse::<u64>().ok())
            .map(Duration::from_millis)
        {
            self.flush(out);
            out.push(ParseEvent::Metric(Metric::duration(
                MetricKind::DomainReload,
                d,
                n,
            )));
            let e = info(
                Category::DomainReload,
                format!("Domain reload took {}", format_duration(d)),
                n,
            );
            self.start(
                Pending::new(e).closing(SectionKind::DomainReload, Some(d)),
                out,
            );
            return;
        }

        if text.starts_with("Reloading assemblies for play mode.") {
            self.flush(out);
            self.begin_play_mode(n, out);
            let k = self.play_mode_count;
            self.emit(
                info(Category::PlayMode, format!("Entering play mode (#{k})"), n),
                out,
            );
            return;
        }

        if let Some(c) = SCENE_LOADED.captures(text) {
            let scene = &c["scene"];
            // Unity reloads the backed-up editor scene twice per play session:
            // right after entering play mode and again when exiting it.
            let p = if scene.starts_with("Temp/__Backupscenes/") {
                self.flush(out);
                if self.is_open(SectionKind::PlayMode) && self.play_scene_loaded {
                    let k = self.play_mode_count;
                    let e = info(
                        Category::PlayMode,
                        format!("Exited play mode (#{k}), restored editor scenes"),
                        n,
                    );
                    Pending::new(e).closing(SectionKind::PlayMode, None)
                } else {
                    if !self.is_open(SectionKind::PlayMode) {
                        // Enter Play Mode Options without domain reload: no
                        // "Reloading assemblies" marker, this load is the start.
                        self.begin_play_mode(n, out);
                    }
                    self.play_scene_loaded = true;
                    let k = self.play_mode_count;
                    Pending::new(info(
                        Category::PlayMode,
                        format!("Play mode #{k} scene loaded"),
                        n,
                    ))
                }
            } else {
                Pending::new(info(
                    Category::SceneLoad,
                    format!("Loaded scene {}", file_name(scene)),
                    n,
                ))
            };
            self.start(p.block(Block::SceneLoad), out);
            return;
        }

        if let Some(c) = SCENE_OPENING.captures(text) {
            let e = info(
                Category::SceneLoad,
                format!("Opening scene {}", file_name(&c["scene"])),
                n,
            );
            self.emit(e, out);
            return;
        }

        if let Some(c) = COMPILE_REQUESTED.captures(text) {
            self.flush(out);
            if !self.is_open(SectionKind::Compile) {
                self.open_section(
                    SectionKind::Compile,
                    SectionKind::Compile.label().into(),
                    n,
                    out,
                );
            }
            let e = info(
                Category::Build,
                format!("Script compilation requested: {}", &c["why"]),
                n,
            );
            self.start(Pending::new(e).fold("bee"), out);
            return;
        }

        if text.starts_with("Starting: ")
            && (text.contains("bee_backend") || text.contains("ScriptCompilationBuildProgram"))
        {
            self.flush(out);
            if !self.is_open(SectionKind::Compile) {
                self.open_section(
                    SectionKind::Compile,
                    SectionKind::Compile.label().into(),
                    n,
                    out,
                );
            }
            // Keep merging into a running compile block instead of one row per step.
            let e = info(Category::Build, text, n);
            self.start(Pending::new(e).fold("bee"), out);
            return;
        }

        if let Some(c) = TUNDRA.captures(text) {
            let d = secs(&c["secs"]);
            match &c["what"] {
                "build success" | "build failed" => {
                    self.flush(out);
                    let failed = &c["what"] == "build failed";
                    if let Some(d) = d {
                        out.push(ParseEvent::Metric(Metric::duration(
                            MetricKind::Compile,
                            d,
                            n,
                        )));
                    }
                    let took = d.map(format_duration).unwrap_or_default();
                    let e = if failed {
                        entry(
                            Severity::Error,
                            Category::Build,
                            format!("Script compilation failed after {took}"),
                            n,
                        )
                    } else {
                        info(
                            Category::Build,
                            format!("Script compilation finished in {took}"),
                            n,
                        )
                    };
                    self.start(Pending::new(e).closing(SectionKind::Compile, d), out);
                }
                _ => {
                    // "requires additional run": part of the running compile block.
                    if !self.extend_fold(n, text, "bee") {
                        self.start(
                            Pending::new(info(Category::Build, text, n)).fold("bee"),
                            out,
                        );
                    }
                }
            }
            return;
        }

        if BEE_PROGRESS.is_match(text) {
            if !self.extend_fold(n, text, "bee") {
                self.start(
                    Pending::new(info(Category::Build, text, n)).fold("bee"),
                    out,
                );
            }
            return;
        }

        if let Some(c) = ASSET_REFRESH.captures(text) {
            let Some(d) = secs(&c["secs"]) else {
                return self.generic(n, text, out);
            };
            self.flush(out);
            out.push(ParseEvent::Metric(Metric::duration(
                MetricKind::AssetRefresh,
                d,
                n,
            )));
            let message = format!(
                "Asset refresh took {} (initiated by {})",
                format_duration(d),
                &c["by"]
            );
            self.emit(info(Category::AssetPipeline, message, n), out);
            return;
        }

        if let Some(u) = parse_unloading(text, n) {
            self.flush(out);
            for metric in u.metrics {
                out.push(ParseEvent::Metric(metric));
            }
            self.start(
                Pending::new(info(Category::Memory, u.message, n)).fold("unload"),
                out,
            );
            return;
        }

        if let Some(d) = PROJECT_LOADED.captures(text).and_then(|c| secs(&c["secs"])) {
            self.flush(out);
            out.push(ParseEvent::Metric(Metric::duration(
                MetricKind::Startup,
                d,
                n,
            )));
            let e = info(
                Category::System,
                format!("Project loaded in {}", format_duration(d)),
                n,
            );
            self.start(Pending::new(e).closing(SectionKind::Startup, Some(d)), out);
            return;
        }

        if text.starts_with("Cleanup mono")
            || text.starts_with("Input System module state changed to: ShutdownInProgress")
        {
            self.flush(out);
            if !self.is_open(SectionKind::Shutdown) {
                self.close_all(n.saturating_sub(1), out);
                self.open_section(
                    SectionKind::Shutdown,
                    SectionKind::Shutdown.label().into(),
                    n,
                    out,
                );
            }
            self.emit(info(Category::System, text, n), out);
            return;
        }

        // --- special blocks ------------------------------------------------
        if PROJECT_PATH.is_match(text) {
            self.emit(info(Category::System, text, n), out);
            return;
        } else if text == "COMMAND LINE ARGUMENTS:" {
            let e = info(Category::System, "Command line arguments", n);
            self.start(Pending::new(e).mode(Mode::CommandLine), out);
            return;
        } else if text == "Direct3D:" {
            let e = info(Category::Graphics, "Graphics device", n);
            self.start(Pending::new(e).block(Block::Graphics), out);
            return;
        }

        self.generic(n, text, out);
    }

    /// Facts are read from every line, independent of how the line is grouped
    /// into entries (header lines fold together, but each carries a fact).
    fn scan_facts(&mut self, n: usize, text: &str, out: &mut Vec<ParseEvent>) {
        if n > 200 {
            return; // header facts only appear at the top of the log
        }
        if let Some(c) = BUILT_FROM.captures(text) {
            out.push(ParseEvent::Fact(Fact::new(
                FactKey::EngineVersion,
                &c["version"],
                n,
            )));
        } else if let Some(c) = OS_LINE.captures(text) {
            let os = c["os"].split_whitespace().collect::<Vec<_>>().join(" ");
            out.push(ParseEvent::Fact(Fact::new(FactKey::Os, os, n)));
            if let Some(mem) = c.name("mem") {
                out.push(ParseEvent::Fact(Fact::new(
                    FactKey::PhysicalMemory,
                    mem.as_str(),
                    n,
                )));
            }
        } else if let Some(c) = DATE.captures(text) {
            out.push(ParseEvent::Fact(Fact::new(
                FactKey::StartDate,
                &c["date"],
                n,
            )));
        } else if let Some(c) = PROJECT_PATH.captures(text) {
            out.push(ParseEvent::Fact(Fact::new(
                FactKey::ProjectPath,
                c["path"].trim(),
                n,
            )));
        }
    }

    /// Appends to the pending entry if it belongs to fold group `fold`.
    fn extend_fold(&mut self, n: usize, text: &str, fold: &str) -> bool {
        match self.pending.as_mut() {
            Some(p) if p.mode == Mode::Plain && p.fold.as_deref() == Some(fold) => {
                p.entry.details.push(text.to_owned());
                p.absorb(n);
                true
            }
            _ => false,
        }
    }

    fn generic(&mut self, n: usize, text: &str, out: &mut Vec<ParseEvent>) {
        let (category, fold) = classify(text);
        let mut p = Pending::new(info(category, text, n));
        p.fold = fold;
        self.start(p, out);
    }
}

impl LogParser for EditorParser {
    fn feed_line(&mut self, line_no: usize, line: &str, out: &mut Vec<ParseEvent>) {
        if !self.started {
            self.started = true;
            self.open_section(
                SectionKind::Startup,
                SectionKind::Startup.label().into(),
                line_no,
                out,
            );
        }
        let text = line.trim_end();
        if text.is_empty() {
            self.on_blank(line_no, out);
            return;
        }
        self.scan_facts(line_no, text, out);
        if self.continue_pending(line_no, text, out) {
            return;
        }
        self.on_line(line_no, text, out);
    }

    fn finish(&mut self, out: &mut Vec<ParseEvent>) {
        self.flush(out);
        // Sections still open are closed at the last line by `LogDocument::finish`.
    }
}

/// Adds a parsed frame to an entry, or reads the severity from logging
/// plumbing frames (`UnityEngine.Debug:LogWarning`) and drops them.
fn apply_frame(entry: &mut LogEntry, frame: ParsedFrame) {
    if frames::is_plumbing(&frame) {
        if entry.category == Category::UserLog
            && let Some(severity) = frames::log_severity(&frame)
        {
            entry.severity = severity;
            if severity == Severity::Exception {
                entry.category = Category::Exception;
            }
        }
        return;
    }
    entry.stack.push(frame.frame);
}

fn classify(text: &str) -> (Category, Option<String>) {
    if let Some((_, category, fold)) = PREFIXES
        .iter()
        .find(|(prefix, ..)| text.starts_with(prefix))
    {
        return (*category, Some((*fold).to_owned()));
    }
    if let Some(c) = TAG.captures(text) {
        return (
            Category::Other,
            Some(format!("tag:{}", c["tag"].to_lowercase())),
        );
    }
    (Category::Other, None)
}

fn graphics_message(api: Option<&str>, gpu: Option<&str>) -> String {
    match (api, gpu) {
        (Some(api), Some(gpu)) => format!("Graphics: {api} on {gpu}"),
        (Some(api), None) => format!("Graphics: {api}"),
        (None, Some(gpu)) => format!("Graphics: {gpu}"),
        (None, None) => "Graphics device".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use comte_core::{FrameKind, LogDocument, parse_reader};

    use super::*;

    fn parse(text: &str) -> LogDocument {
        parse_reader(Cursor::new(text.to_owned()), &mut EditorParser::new()).unwrap()
    }

    #[test]
    fn debug_log_with_stack_becomes_user_warning() {
        let doc = parse(
            "[Text] Draft text is allowed: 558 unreviewed fragments are in use. Development only.
UnityEngine.Debug:ExtractStackTraceNoAlloc (byte*,int,string)
UnityEngine.StackTraceUtility:ExtractStackTrace ()
UnityEngine.DebugLogHandler:Internal_Log (UnityEngine.LogType,UnityEngine.LogOption,string,UnityEngine.Object)
UnityEngine.DebugLogHandler:LogFormat (UnityEngine.LogType,UnityEngine.Object,string,object[])
UnityEngine.Logger:Log (UnityEngine.LogType,object)
UnityEngine.Debug:LogWarning (object)
SpiritCabinet.Installers.GameInstaller:Report (SpiritCabinet.Text.Import.TextData) (at Assets/SpiritCabinet/Scripts/Installers/GameInstaller.cs:84)
SpiritCabinet.Installers.GameInstaller:InstallBindings () (at Assets/SpiritCabinet/Scripts/Installers/GameInstaller.cs:31)

(Filename: Assets/SpiritCabinet/Scripts/Installers/GameInstaller.cs Line: 84)

Next line
",
        );
        let e = &doc.entries()[0];
        assert_eq!(e.severity, Severity::Warning);
        assert_eq!(e.category, Category::UserLog);
        assert!(e.message.starts_with("[Text] Draft text is allowed"));
        assert_eq!(e.stack.len(), 2, "plumbing frames dropped");
        assert_eq!(e.stack[0].text, "GameInstaller.Report(TextData)");
        assert_eq!(e.culprit().unwrap().short(), "GameInstaller.cs:84");
        assert_eq!(e.line_range, 1..12);
        assert_eq!(doc.entries()[1].message, "Next line");
    }

    #[test]
    fn message_is_split_from_preceding_fold_group() {
        let doc = parse(
            "[Flow] one
[Flow] two
[Flow] three
UnityEngine.Debug:Log (object)
Game.Flow:Tick () (at Assets/Flow.cs:9)

(Filename: Assets/Flow.cs Line: 9)
",
        );
        let entries = doc.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "[Flow] one");
        assert_eq!(entries[0].details, ["[Flow] two"]);
        assert_eq!(entries[0].line_range, 1..3);
        assert_eq!(entries[1].message, "[Flow] three");
        assert_eq!(entries[1].category, Category::UserLog);
        assert_eq!(entries[1].line_range.start, 3);
    }

    #[test]
    fn mono_exception_with_rethrow_and_filename() {
        let doc = parse(
            r"MissingReferenceException: The object of type 'UnityEngine.GameObject' has been destroyed but you are still trying to access it.
Your script should either check if it is null or you should not destroy the object.
  at UnityEngine.GameObject.SetActive (System.Boolean value) [0x00006] in <1b6535aa091d41cf8b3ca222ed9ceef0>:0
  at Game.UI.TurnView.HideFile () [0x00001] in C:\Users\dev\Projects\Game\Assets\Scripts\UI\TurnView.cs:98
  at Zenject.DisposableManager.Dispose () [0x000d7] in C:\Users\dev\Projects\Game\Assets\Plugins\Zenject\DisposableManager.cs:122
Rethrow as ZenjectException: Error occurred while disposing IDisposable with type 'GameRunner'
  at Zenject.MonoKernel.OnDestroy () [0x0004f] in C:\Users\dev\Projects\Game\Assets\Plugins\Zenject\MonoKernel.cs:122

(Filename: Assets/Scripts/UI/TurnView.cs Line: 98)

",
        );
        assert_eq!(doc.entries().len(), 1);
        let e = &doc.entries()[0];
        assert_eq!(e.severity, Severity::Exception);
        assert_eq!(e.category, Category::Exception);
        assert_eq!(e.details.len(), 2);
        assert!(e.details[1].starts_with("Rethrow as ZenjectException"));
        assert_eq!(e.stack.len(), 4);
        assert_eq!(e.stack[1].kind, FrameKind::User);
        assert_eq!(e.culprit().unwrap().short(), "TurnView.cs:98");
    }

    #[test]
    fn exception_without_filename_ends_at_next_line() {
        let doc = parse(
            "UnityConnectWebRequestException: Token Exchange failed due a failure with the web request.
  at UnityEditor.Connect.TokenExchange.VerifyTokenExchangeResponse (UnityEngine.Networking.UnityWebRequest exchangeRequest) [0x00019] in <2e4eaf245b924ed1b8c17467295ae042>:0

Asset Pipeline Refresh (id=35bd790774014b14f9af60ee35eaea95): Total: 0.006 seconds - Initiated by RefreshV2(AllowForceSynchronousImport)
",
        );
        assert_eq!(doc.entries().len(), 2);
        assert_eq!(doc.entries()[0].line_range, 1..4);
        assert_eq!(doc.entries()[1].category, Category::AssetPipeline);
        assert_eq!(
            doc.entries()[1].message,
            "Asset refresh took 6 ms (initiated by RefreshV2(AllowForceSynchronousImport))"
        );
    }

    #[test]
    fn compiler_errors_and_warnings() {
        let doc = parse(
            "Assets/Scripts/Player.cs(12,5): error CS0103: The name 'speed' does not exist in the current context
Assets/Scripts/Old.cs(3,20): warning CS0618: 'WWW' is obsolete
",
        );
        let [error, warning] = doc.entries() else {
            panic!("two entries")
        };
        assert_eq!(error.severity, Severity::Error);
        assert_eq!(error.category, Category::Compiler);
        assert_eq!(
            error.message,
            "CS0103: The name 'speed' does not exist in the current context"
        );
        assert_eq!(
            error.source.as_ref().unwrap().to_string(),
            "Assets/Scripts/Player.cs:12:5"
        );
        assert_eq!(warning.severity, Severity::Warning);
    }

    #[test]
    fn domain_reload_section_with_profiling_tree() {
        let doc = parse(
            "Begin MonoManager ReloadAssembly
- Loaded All Assemblies, in  0.629 seconds
Mono: successfully reloaded assembly
Domain Reload Profiling: 1304ms
\tBeginReloadAssembly (193ms)
\t\tExecutionOrderSort (0ms)
\tFinalizeReload (681ms)
After
",
        );
        let reload = doc
            .sections()
            .iter()
            .find(|s| s.kind == SectionKind::DomainReload)
            .unwrap();
        assert_eq!(reload.duration, Some(Duration::from_millis(1304)));
        assert_eq!(reload.line_range, 1..8);
        let summary = doc
            .entries()
            .iter()
            .find(|e| e.message.starts_with("Domain reload took"))
            .unwrap();
        assert_eq!(summary.message, "Domain reload took 1.30 s");
        assert_eq!(summary.details.len(), 3);
        assert_eq!(summary.section_id, Some(reload.id));
        let after = doc.entries().last().unwrap();
        assert_eq!(after.message, "After");
        assert_ne!(after.section_id, Some(reload.id));
        assert_eq!(doc.metrics()[0].kind, MetricKind::DomainReload);
    }

    #[test]
    fn play_mode_sessions_nest_reloads_and_close_on_scene_restore() {
        let doc = parse(
            "[Project] Loading completed in 9.877 seconds
Reloading assemblies for play mode.
Begin MonoManager ReloadAssembly
Domain Reload Profiling: 2503ms
Loaded scene 'Temp/__Backupscenes/0.backup'
\tTotal Operation Time:   412.494 ms
Hello from game
Unloading 3 Unused Serialized files (Serialized files now loaded: 0)
Loaded scene 'Temp/__Backupscenes/0.backup'
\tTotal Operation Time:   23.449 ms
Reloading assemblies for play mode.
",
        );
        let titles: Vec<_> = doc
            .sections()
            .iter()
            .map(|s| (s.title.as_str(), s.depth))
            .collect();
        assert_eq!(
            titles,
            [
                ("Startup", 0),
                ("Play mode #1", 0),
                ("Domain reload", 1),
                ("Play mode #2", 0)
            ]
        );
        let startup = &doc.sections()[0];
        assert_eq!(startup.duration, Some(Duration::from_millis(9877)));
        let play1 = &doc.sections()[1];
        assert_eq!(play1.line_range, 2..11);
        let entered = doc
            .entries()
            .iter()
            .find(|e| e.message.starts_with("Play mode #1 scene loaded"))
            .unwrap();
        assert_eq!(entered.section_id, Some(play1.id));
        let hello = doc
            .entries()
            .iter()
            .find(|e| e.message == "Hello from game")
            .unwrap();
        assert_eq!(
            hello.section_id,
            Some(play1.id),
            "game logs stay inside the play session"
        );
        let exit = doc
            .entries()
            .iter()
            .find(|e| e.message.starts_with("Exited play mode"))
            .unwrap();
        assert_eq!(
            exit.message,
            "Exited play mode (#1), restored editor scenes in 23 ms"
        );
        assert_eq!(exit.section_id, Some(play1.id));
    }

    #[test]
    fn play_mode_without_domain_reload_starts_on_backup_scene_load() {
        let doc = parse(
            "Loaded scene 'Temp/__Backupscenes/0.backup'
game log
Loaded scene 'Temp/__Backupscenes/0.backup'
editor again
",
        );
        let play = doc
            .sections()
            .iter()
            .find(|s| s.kind == SectionKind::PlayMode)
            .unwrap();
        assert_eq!(play.title, "Play mode #1");
        assert_eq!(play.line_range, 1..4);
        let editor_again = doc.entries().last().unwrap();
        assert_ne!(editor_again.section_id, Some(play.id));
    }

    #[test]
    fn compile_section_from_bee_to_tundra() {
        let doc = parse(
            "[ScriptCompilation] Requested script compilation because: Assetdatabase observed changes in script compilation related files
AssetDatabase: script compilation time: 0.001592s
Starting: C:\\Unity\\Editor\\Data\\Tools\\BuildPipeline\\bee_backend.exe --ipc
WorkingDir: C:/Projects/Game
ExitCode: 0 Duration: 14s
[384/405    2s] Csc Library/Bee/artifacts/1900b0aEDbg.dag/Game.Core.dll (+2 others)
[BUSY       6s] Csc Library/Bee/artifacts/1900b0aEDbg.dag/Game.Flow.dll (+2 others)
*** Tundra build success (14.60 seconds), 21 items updated, 405 evaluated
",
        );
        let compile = doc
            .sections()
            .iter()
            .find(|s| s.kind == SectionKind::Compile)
            .unwrap();
        assert_eq!(compile.duration, Some(Duration::from_millis(14600)));
        assert!(compile.closed);
        let in_compile: Vec<_> = doc
            .entries_in(Some(compile.id))
            .map(|e| e.message.as_str())
            .collect();
        assert_eq!(in_compile.len(), 3, "{in_compile:?}");
        assert_eq!(in_compile[2], "Script compilation finished in 14.60 s");
    }

    #[test]
    fn tundra_failure_is_an_error() {
        let doc = parse(
            "Starting: bee_backend.exe\n*** Tundra build failed (2.10 seconds), 3 items updated, 405 evaluated\n",
        );
        let last = doc.entries().last().unwrap();
        assert_eq!(last.severity, Severity::Error);
        assert_eq!(last.message, "Script compilation failed after 2.10 s");
    }

    #[test]
    fn header_facts_and_graphics_block() {
        let doc = parse(
            "[Licensing::Module] Trying to connect to existing licensing client channel...
Built from '6000.3/staging' branch; Version is '6000.3.11f1 (3000ef702840) revision 3145967'; Using compiler version '194234433'; Build Type 'Release'
OS: 'Windows 11  (10.0.26200) CoreSingleLanguage' Language: 'en' Physical Memory: 32600 MB
Date: 2026-11-05T17:59:24Z
COMMAND LINE ARGUMENTS:
C:\\Unity\\Editor\\Unity.exe
-projectpath
C:\\Projects\\Game
Successfully changed project path to: C:\\Projects\\Game
Direct3D:
    Version:         Direct3D 12 [level 12.2]
    Renderer:        NVIDIA GeForce RTX 2070 Super (ID=0x1e91)
    Vendor:          NVIDIA
    VRAM:            7989 MB
    Driver:          32.0.16.1692
Initialize mono
",
        );
        let fact = |k: FactKey| doc.fact(&k).map(str::to_owned);
        assert_eq!(fact(FactKey::EngineVersion).as_deref(), Some("6000.3.11f1"));
        assert_eq!(
            fact(FactKey::Os).as_deref(),
            Some("Windows 11 (10.0.26200) CoreSingleLanguage")
        );
        assert_eq!(fact(FactKey::PhysicalMemory).as_deref(), Some("32600 MB"));
        assert_eq!(
            fact(FactKey::StartDate).as_deref(),
            Some("2026-11-05T17:59:24Z")
        );
        assert_eq!(
            fact(FactKey::ProjectPath).as_deref(),
            Some("C:\\Projects\\Game")
        );
        assert_eq!(
            fact(FactKey::Gpu).as_deref(),
            Some("NVIDIA GeForce RTX 2070 Super")
        );
        assert_eq!(fact(FactKey::Vram).as_deref(), Some("7989 MB"));
        assert_eq!(fact(FactKey::GpuDriver).as_deref(), Some("32.0.16.1692"));

        let messages: Vec<_> = doc.entries().iter().map(|e| e.message.as_str()).collect();
        assert!(messages.contains(&"Command line arguments"));
        assert!(
            messages
                .contains(&"Graphics: Direct3D 12 [level 12.2] on NVIDIA GeForce RTX 2070 Super")
        );
        let header = doc
            .entries()
            .iter()
            .find(|e| e.message.starts_with("Built from"))
            .unwrap();
        assert_eq!(
            header.details.len(),
            2,
            "OS and Date fold into the header entry"
        );
    }

    #[test]
    fn licensing_lines_fold_into_one_noise_entry() {
        let doc = parse(
            "[Licensing::Client] Successfully resolved entitlement details
[Licensing::Module] License group:
  Id: F4-XXXX
  Product: Unity Personal
[Licensing::Module] License group:
  Id: 1053258-XXXX
Library Redirect Path: Library/
",
        );
        assert_eq!(doc.entries().len(), 2);
        assert_eq!(doc.entries()[0].category, Category::Licensing);
        assert!(doc.entries()[0].category.is_noise());
        assert_eq!(doc.entries()[0].line_range, 1..7);
    }

    #[test]
    fn unloading_folds_memory_lines_and_emits_metrics() {
        let doc = parse(
            "Unloading 1579 unused Assets / (2.9 MB). Loaded Objects now: 2181.
Memory consumption went from 141.1 MB to 138.1 MB.
Total: 7.788200 ms (FindLiveObjects: 0.209100 ms CreateObjectMapping: 0.119400 ms MarkObjects: 5.467700 ms  DeleteObjects: 1.991200 ms)
",
        );
        assert_eq!(doc.entries().len(), 1);
        assert_eq!(
            doc.entries()[0].message,
            "Unloaded 1579 unused assets (2.9 MB); 2181 objects still loaded"
        );
        assert_eq!(doc.metrics().len(), 2);
    }

    #[test]
    fn unloading_line_is_not_swallowed_by_preceding_memory_group() {
        let doc = parse(
            "Unloading 16 Unused Serialized files (Serialized files now loaded: 0)
Unloading 1579 unused Assets / (2.9 MB). Loaded Objects now: 2181.
Memory consumption went from 141.1 MB to 138.1 MB.
",
        );
        assert_eq!(doc.entries().len(), 2);
        assert_eq!(
            doc.entries()[1].message,
            "Unloaded 1579 unused assets (2.9 MB); 2181 objects still loaded"
        );
        assert_eq!(doc.entries()[1].details.len(), 1);
        assert_eq!(doc.metrics().len(), 2);
    }

    #[test]
    fn shutdown_closes_open_sections() {
        let doc = parse("Reloading assemblies for play mode.\nCleanup mono\nbye\n");
        let kinds: Vec<_> = doc
            .sections()
            .iter()
            .map(|s| (s.kind, s.depth, s.closed))
            .collect();
        assert_eq!(
            kinds,
            [
                (SectionKind::Startup, 0, true),
                (SectionKind::PlayMode, 0, true),
                (SectionKind::Shutdown, 0, true)
            ]
        );
        assert_eq!(doc.entries().last().unwrap().section_id, Some(2));
    }
}
