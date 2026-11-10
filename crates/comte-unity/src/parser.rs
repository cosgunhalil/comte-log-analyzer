//! Streaming parser shared by Unity `Editor.log` (DESIGN §4.1) and
//! `Player.log` (DESIGN §4.2). Both logs use the same building blocks
//! (`Debug.Log` traces, Mono exceptions, `(Filename: …)` lines, memory and
//! graphics blocks); a [`Flavor`] switches the rules that differ, such as
//! play mode sessions (Editor) versus scene sections (Player).
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
    AssetImport, BuildReport, Category, Fact, FactKey, FrameKind, LogEntry, LogParser, Metric,
    MetricKind, ParseEvent, ReloadPhase, ReloadProfile, SectionKind, SectionStart, Severity,
    SizeRow, SourceRef, StackFrame, Unfinished, format_bytes, format_duration, parse_size,
};

use crate::common::{
    file_name, graphics_fact, is_exception_header, millis, parse_filename_line, parse_unloading,
    secs,
};
use crate::frames::{self, ParsedFrame};
use crate::hints::{CrashStats, crash_hint, entry_text, rule_hint};

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
regex!(ENGINE_VERSION = r"^Initialize engine version: (?P<version>\S+)");
regex!(PLAYER_DATA = r"^Loading player data from (?P<path>.+?)/[^/]+_Data/data\.unity3d$");
regex!(PLAYER_MONO_PATH = r"^Mono path\[0\] = '(?P<path>.+?)/[^/]+_Data/Managed'$");
regex!(COMPLETED_RELOAD = r"^- Completed reload, in\s+(?P<secs>[\d.]+) seconds");
regex!(UNLOAD_TIME = r"^UnloadTime: (?P<ms>[\d.]+) ms");
regex!(
    NATIVE_FRAME = r"^\s*0x[0-9A-Fa-f]+ \((?P<module>[^)]*)\)\s*(?:\[(?P<path>[^\]]*?):(?P<line>\d+)\]\s*)?(?P<func>.*)$"
);
// `[Worker2] Start importing Assets/a.png using Guid(…) Importer(…)`, with the
// result on the same line (` -> (artifact id: '…') in 0.05 seconds`) or later on
// a line of its own from the same worker.
regex!(
    IMPORT_START = r"^(?:\[Worker(?P<worker>\d+)\] )?Start importing (?P<path>.+?) using Guid\([0-9a-fA-F]+\)(?P<rest>.*)$"
);
regex!(IMPORT_RESULT = r"-> \(artifact id: '[0-9a-fA-F]*'\) in (?P<secs>[\d.]+) seconds");
regex!(
    IMPORT_DONE = r"^(?:\[Worker(?P<worker>\d+)\])?\s*-> \(artifact id: '[0-9a-fA-F]*'\) in (?P<secs>[\d.]+) seconds"
);
// Build report rows: `Textures   12.4 mb   52.9%` and ` 8.0 mb  34.2% Assets/x.png`.
regex!(
    BUILD_CATEGORY =
        r"^(?P<name>[A-Za-z][A-Za-z ]*?)\s{2,}(?P<size>[\d.]+ ?[kmgKMG]?[bB])\s+[\d.]+%\s*$"
);
regex!(BUILD_TOTAL = r"^Complete (?:build )?size\s+(?P<size>[\d.]+ ?[kmgKMG]?[bB])");
regex!(BUILD_ASSET = r"^\s*(?P<size>[\d.]+ ?[kmgKMG]?[bB])\s+[\d.]+%\s+(?P<path>\S.*)$");
// `C:\Game\UnityPlayer.dll:UnityPlayer.dll (00007FFB2A6B0000), size: 28655616 (result: 0), SymType: '-exported-', PDB: ''`
regex!(CRASH_MODULE = r"^\S.*:[^:\\/]+\.(?i:dll|exe) \([0-9A-Fa-f]+\), size: \d+");
// `\t\tProcessInitializeOnLoadAttributes (179ms)` under `Domain Reload Profiling`.
regex!(RELOAD_PHASE = r"^(?P<tabs>\t+)(?P<name>[A-Za-z][\w.]*) \((?P<ms>\d+)ms\)$");
regex!(POSIX_FRAME = r"^\s*#\d+\s+(?:pc\s+)?(?:0x)?[0-9a-fA-F]+\s+(?:in\s+)?(?P<func>.+)$");

/// Which Unity program wrote the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Editor,
    Player,
}

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
    ("New input system", Category::Input, "input"),
    ("Setting up ", Category::System, "threads"),
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
    /// A native crash: everything up to `END OF STACKTRACE` belongs to it.
    Crash,
    /// A `Build Report` size table.
    BuildReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    None,
    /// The `Domain Reload Profiling` tree.
    ReloadProfile,
    Graphics,
    SceneLoad,
    CrashReport,
}

/// Most lines a native crash entry absorbs when the log has no end marker.
const MAX_CRASH_LINES: usize = 400;
/// Assets kept from a build report (the list can be thousands long).
const MAX_BUILD_ASSETS: usize = 200;
/// A domain reload or compile counts as "where it stopped" only if it
/// started within this many lines of the end: an editor that got stuck
/// writes nothing more, while a section the log merely never closed
/// explicitly is followed by plenty of other output.
const STOPPED_WINDOW: usize = 40;

#[derive(Debug, Clone)]
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
    /// A blank line followed this (plain) entry. Only `(Filename: …)` may
    /// still attach to it: release Player logs write `message`, ` `, `(Filename: …)`.
    after_blank: bool,
    /// The size report this entry is collecting (`Mode::BuildReport`).
    build: Option<BuildReport>,
    /// Module and symbol counts of a native crash (`Mode::Crash`).
    crash: CrashStats,
    /// The domain reload breakdown this entry collects (`Block::ReloadProfile`).
    profile: Option<ReloadProfile>,
    /// The crash report folder named by this entry (`Block::CrashReport`).
    crash_folder: Option<String>,
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
            after_blank: false,
            build: None,
            crash: CrashStats::default(),
            crash_folder: None,
            profile: None,
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

/// An Info entry whose message the parser wrote for humans.
fn summary(category: Category, message: impl Into<String>, n: usize) -> LogEntry {
    info(category, message, n).as_summary()
}

/// The shared Unity parser. Use [`EditorParser`] or [`PlayerParser`].
#[derive(Debug, Clone)]
pub struct UnityParser {
    flavor: Flavor,
    pending: Option<Pending>,
    open: Vec<SectionKind>,
    play_mode_count: u32,
    /// The enter-time backup scene load of the current play session was seen.
    play_scene_loaded: bool,
    scene_count: u32,
    started: bool,
    /// First line of each open section, parallel to `open`.
    open_lines: Vec<usize>,
    /// Imports started but not finished: (worker, path, line), oldest first.
    importing: Vec<(Option<u32>, String, usize)>,
    last_line: usize,
    /// The GPU, once the graphics block named it.
    gpu: Option<String>,
    /// Native crashes seen: (line, stack, counts). Triaged in `finish`,
    /// once the crash report folder (printed after the stack) is known.
    crashes: Vec<(usize, Vec<StackFrame>, CrashStats)>,
    crash_folder: Option<String>,
    /// Rules already reported (each once per log).
    hinted: Vec<&'static str>,
    /// A licensing failure line, and whether Unity ever connected to the
    /// licensing client. Healthy logs are full of licensing "errors".
    licensing_problem: Option<(usize, String)>,
    licensing_connected: bool,
}

impl UnityParser {
    pub fn new(flavor: Flavor) -> Self {
        Self {
            flavor,
            pending: None,
            open: Vec::new(),
            play_mode_count: 0,
            play_scene_loaded: false,
            scene_count: 0,
            started: false,
            open_lines: Vec::new(),
            importing: Vec::new(),
            last_line: 0,
            gpu: None,
            crashes: Vec::new(),
            crash_folder: None,
            hinted: Vec::new(),
            licensing_problem: None,
            licensing_connected: false,
        }
    }

    pub fn flavor(&self) -> Flavor {
        self.flavor
    }

    fn is_player(&self) -> bool {
        self.flavor == Flavor::Player
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
        self.open_lines.push(line);
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
            self.open_lines.pop();
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
        self.open_lines.clear();
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

    /// First line of the innermost open section of `kind`.
    fn open_since(&self, kind: SectionKind) -> Option<usize> {
        let i = self.open.iter().rposition(|k| *k == kind)?;
        self.open_lines.get(i).copied()
    }

    // -- imports and unfinished work ----------------------------------------

    /// Tracks asset imports on every line, whatever entry the line ends up in.
    fn scan_imports(&mut self, n: usize, text: &str, out: &mut Vec<ParseEvent>) {
        if let Some(c) = IMPORT_START.captures(text) {
            let worker = c.name("worker").and_then(|w| w.as_str().parse().ok());
            let path = c["path"].trim().to_owned();
            if let Some(d) = IMPORT_RESULT
                .captures(&c["rest"])
                .and_then(|r| secs(&r["secs"]))
            {
                out.push(ParseEvent::Import(AssetImport {
                    path,
                    duration: d,
                    line: n,
                }));
            } else {
                // A worker starts its next import only after the last one.
                self.importing.retain(|(w, _, _)| *w != worker);
                self.importing.push((worker, path, n));
            }
        } else if let Some(c) = IMPORT_DONE.captures(text) {
            let worker = c.name("worker").and_then(|w| w.as_str().parse().ok());
            if let Some(i) = self.importing.iter().position(|(w, _, _)| *w == worker) {
                let (_, path, line) = self.importing.remove(i);
                if let Some(d) = secs(&c["secs"]) {
                    out.push(ParseEvent::Import(AssetImport {
                        path,
                        duration: d,
                        line,
                    }));
                }
            }
        }
    }

    fn scan_licensing(&mut self, n: usize, text: &str) {
        if !text.contains("icens") {
            return;
        }
        if text.contains("Successfully connected to LicensingClient")
            || text.contains("Successfully resolved entitlement")
        {
            self.licensing_connected = true;
        } else if self.licensing_problem.is_none()
            && [
                "No valid Unity Editor license",
                "No ULF license found",
                "Cannot connect to Licensing Client",
                "Channel doesn't exist",
                "Failed to launch LicensingClient",
                "LicensingClient has failed to start",
            ]
            .iter()
            .any(|needle| text.contains(needle))
        {
            self.licensing_problem = Some((n, text.to_owned()));
        }
    }

    /// Hints that need the whole log: crash triage (the crash folder comes
    /// after the stack) and licensing (it only failed if it never connected).
    fn final_hints(&self, out: &mut Vec<ParseEvent>) {
        for (line, stack, stats) in &self.crashes {
            out.push(ParseEvent::Hint(crash_hint(
                stack,
                *stats,
                self.crash_folder.as_deref(),
                *line,
            )));
        }
        if let Some((line, _)) = &self.licensing_problem
            && !self.licensing_connected
            && !self.is_player()
        {
            out.push(ParseEvent::Hint(comte_core::Hint {
                title: "Unity couldn't get a license".to_owned(),
                advice: vec![
                    "The Editor never connected to Unity's licensing client. Restart Unity \
                     Hub (it starts the client) and sign in again."
                        .to_owned(),
                    "A firewall, proxy or VPN can block it. Unity.Licensing.Client.log (in \
                     the Unity folder of your local app data) has the details."
                        .to_owned(),
                    "Licensing \"errors\" in a session that did connect are normal; only \
                     this case needs fixing."
                        .to_owned(),
                ],
                line: *line,
                path: None,
                topic: "licensing".to_owned(),
            }));
        }
    }

    /// What the editor was still doing at the end of the log, if anything.
    fn unfinished(&self) -> Option<Unfinished> {
        // A shutdown clears imports and closes sections: whatever is still
        // open at the end started after it.
        if self.is_player() {
            return None;
        }
        if let Some(((_, path, line), rest)) = self.importing.split_first() {
            return Some(Unfinished {
                headline: format!("Stopped while importing {path}"),
                hint: "Unity is stuck on this asset, or quit before it was done. If it stays stuck, move the asset out of Assets and reopen the project to confirm."
                    .to_owned(),
                subject: Some(path.clone()),
                line: *line,
                also: rest
                    .iter()
                    .map(|(_, path, _)| format!("importing {path}"))
                    .collect(),
            });
        }
        let recent = |line: &usize| line + STOPPED_WINDOW >= self.last_line;
        if let Some(line) = self.open_since(SectionKind::DomainReload).filter(recent) {
            return Some(Unfinished {
                headline: "Stopped during a domain reload".to_owned(),
                hint: "Unity was reloading scripts. Code that never returns in a static constructor or an [InitializeOnLoad] method is a common cause."
                    .to_owned(),
                subject: None,
                line,
                also: Vec::new(),
            });
        }
        if let Some(line) = self.open_since(SectionKind::Compile).filter(recent) {
            return Some(Unfinished {
                headline: "Stopped while compiling scripts".to_owned(),
                hint: "The compiler never reported back. Look for a stuck bee_backend process, or a crash right after this point."
                    .to_owned(),
                subject: None,
                line,
                also: Vec::new(),
            });
        }
        None
    }

    /// Closes any previous play session (and startup) and opens the next one.
    fn begin_play_mode(&mut self, n: usize, out: &mut Vec<ParseEvent>) {
        let before = n.saturating_sub(1);
        self.close_section(SectionKind::PlayMode, before, None, out);
        self.close_section(SectionKind::Startup, before, None, out);
        // Entering play mode means the editor was not shutting down after all.
        self.close_section(SectionKind::Shutdown, before, None, out);
        self.play_mode_count += 1;
        self.play_scene_loaded = false;
        let title = format!("Play mode #{}", self.play_mode_count);
        self.open_section(SectionKind::PlayMode, title, n, out);
    }

    // -- pending entry ------------------------------------------------------

    fn flush(&mut self, out: &mut Vec<ParseEvent>) {
        if let Some(mut p) = self.pending.take() {
            if let Some(report) = p.build.take() {
                p.entry.message = build_report_message(&report);
                out.push(ParseEvent::BuildReport(report));
            }
            if let Some(profile) = p.profile.take() {
                out.push(ParseEvent::ReloadProfile(profile));
            }
            if let Some(gpu) = p.gfx_gpu.take() {
                self.gpu = Some(gpu);
            }
            if let Some(folder) = p.crash_folder.take() {
                self.crash_folder = Some(folder);
            }
            if p.mode == Mode::Crash {
                let c = p.crash;
                if c.modules > 0 {
                    p.entry.details.push(format!(
                        "{} modules loaded ({} without symbols)",
                        c.modules, c.modules_without_symbols
                    ));
                }
                if c.symbol_errors > 0 {
                    p.entry.details.push(format!(
                        "{} symbol lookups failed (frames Windows couldn't name)",
                        c.symbol_errors
                    ));
                }
                self.crashes
                    .push((p.entry.line_range.start, p.entry.stack.clone(), c));
            }
            let text = entry_text(&p.entry.message, &p.entry.details);
            if let Some((id, mut hint)) = rule_hint(&text, self.gpu.as_deref()) {
                // Unity prints some of these as plain lines; they are errors.
                if p.entry.severity < Severity::Error {
                    p.entry.severity = Severity::Error;
                }
                if !self.hinted.contains(&id) {
                    self.hinted.push(id);
                    hint.line = p.entry.line_range.start;
                    id.clone_into(&mut hint.topic);
                    out.push(ParseEvent::Hint(hint));
                }
            }
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

        if p.mode != Mode::Crash
            && let Some(source) = parse_filename_line(text)
        {
            // Release builds without stack traces log `message` +
            // `(Filename: …/Debug.bindings.h Line: N)`: that is a Debug.Log call.
            let from_debug_log = source
                .as_ref()
                .is_some_and(|s| s.path.ends_with("Debug.bindings.h"));
            if from_debug_log && p.entry.category == Category::Other {
                p.entry.category = Category::UserLog;
            }
            // `Debug.bindings.h` is where every release-build Debug.Log ends up;
            // as a location it says nothing, so it is not kept.
            if p.entry.source.is_none() && !from_debug_log {
                p.entry.source = source;
            }
            p.absorb(n);
            self.flush(out);
            return true;
        }

        match p.mode {
            Mode::BuildReport => {
                let report = p.build.get_or_insert_with(BuildReport::default);
                if text.starts_with("-----") {
                    p.absorb(n);
                    self.flush(out);
                    return true;
                }
                if let Some(c) = BUILD_TOTAL.captures(text) {
                    report.total = parse_size(&c["size"]);
                } else if let Some(c) = BUILD_CATEGORY.captures(text) {
                    let bytes = parse_size(&c["size"]).unwrap_or(0);
                    if c["name"].trim() == "Total User Assets" {
                        report.user_assets = Some(bytes);
                    } else {
                        report.categories.push(SizeRow {
                            name: c["name"].trim().to_owned(),
                            bytes,
                        });
                    }
                } else if let Some(c) = BUILD_ASSET.captures(text) {
                    if report.assets.len() < MAX_BUILD_ASSETS {
                        report.assets.push(SizeRow {
                            name: c["path"].trim().to_owned(),
                            bytes: parse_size(&c["size"]).unwrap_or(0),
                        });
                    }
                } else if !(text.starts_with("Uncompressed usage by category")
                    || text.starts_with("Used Assets"))
                {
                    return false; // the report is over
                }
                p.entry.details.push(text.to_owned());
                p.absorb(n);
                true
            }
            Mode::Crash => {
                if text.starts_with("========== END OF STACKTRACE") {
                    p.absorb(n);
                    self.flush(out);
                    return true;
                }
                if p.entry.line_count() >= MAX_CRASH_LINES {
                    return false;
                }
                // Noise for people: counted, not listed.
                let trimmed = text.trim_start();
                if trimmed.starts_with("ERROR: SymGet") || trimmed.starts_with("ERROR: SymFrom") {
                    p.crash.symbol_errors += 1;
                    p.absorb(n);
                    return true;
                }
                if CRASH_MODULE.is_match(text) {
                    p.crash.modules += 1;
                    if text.contains("PDB: ''") {
                        p.crash.modules_without_symbols += 1;
                    }
                    p.absorb(n);
                    return true;
                }
                if let Some(frame) = native_frame(text) {
                    p.entry.stack.push(frame);
                } else if !text.starts_with("==========") {
                    p.entry.details.push(text.trim().to_owned());
                }
                p.absorb(n);
                true
            }
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
                if p.after_blank {
                    return false;
                }
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
                        Block::CrashReport => {
                            if let Some(folder) = text.trim().strip_prefix("* ") {
                                out.push(ParseEvent::Fact(Fact::new(
                                    FactKey::CrashReport,
                                    folder.trim(),
                                    n,
                                )));
                                p.crash_folder = Some(folder.trim().to_owned());
                            }
                        }
                        Block::ReloadProfile => {
                            if let (Some(c), Some(profile)) =
                                (RELOAD_PHASE.captures(text), p.profile.as_mut())
                            {
                                let name = c["name"].to_owned();
                                let (about, your_code) = phase_about(&name);
                                profile.phases.push(ReloadPhase {
                                    depth: u8::try_from(c["tabs"].len()).unwrap_or(u8::MAX),
                                    duration: Duration::from_millis(c["ms"].parse().unwrap_or(0)),
                                    about: about.map(str::to_owned),
                                    your_code,
                                    name,
                                });
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
                let structural = is_structural(text, n);
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
            Some(p) if matches!(p.mode, Mode::Crash | Mode::BuildReport) => p.absorb(n),
            Some(p) if p.mode == Mode::Plain && !p.after_blank => p.after_blank = true,
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

        if let Some(message) = crash_start(text) {
            self.flush(out);
            if !self.is_open(SectionKind::Crash) {
                self.open_section(SectionKind::Crash, "Crash".into(), n, out);
            }
            let e = entry(Severity::Crash, Category::Crash, message, n);
            self.start(Pending::new(e).mode(Mode::Crash), out);
            return;
        }

        if text.starts_with("A crash has been intercepted by the crash handler") {
            let e = summary(
                Category::Crash,
                "Unity's crash handler wrote a crash report",
                n,
            );
            self.start(Pending::new(e).block(Block::CrashReport), out);
            return;
        }

        if text == "Uploading Crash Report" {
            // Despite the wording this is Unity Cloud Diagnostics reporting the
            // exception logged right after it, not a crash.
            let e = summary(
                Category::System,
                "Exception reported to Unity Cloud Diagnostics",
                n,
            );
            self.emit(e, out);
            return;
        }

        if let Some(d) = COMPLETED_RELOAD
            .captures(text)
            .and_then(|c| secs(&c["secs"]))
        {
            self.flush(out);
            let (kind, message) = if self.is_player() {
                (
                    MetricKind::Startup,
                    format!("Scripts loaded in {}", format_duration(d)),
                )
            } else {
                (
                    MetricKind::DomainReload,
                    format!("Domain reload took {}", format_duration(d)),
                )
            };
            out.push(ParseEvent::Metric(Metric::duration(kind, d, n)));
            let e = summary(Category::DomainReload, message, n);
            self.start(
                Pending::new(e).closing(SectionKind::DomainReload, Some(d)),
                out,
            );
            return;
        }

        if let Some(d) = UNLOAD_TIME.captures(text).and_then(|c| millis(&c["ms"])) {
            self.flush(out);
            out.push(ParseEvent::Metric(Metric::duration(
                MetricKind::SceneUnload,
                d,
                n,
            )));
            let message = if self.is_player() {
                // Unity prints UnloadTime once a new scene finished loading:
                // each one starts a scene section in the Player timeline.
                let before = n.saturating_sub(1);
                self.close_section(SectionKind::SceneLoad, before, None, out);
                self.close_section(SectionKind::Startup, before, None, out);
                self.scene_count += 1;
                let k = self.scene_count;
                self.open_section(SectionKind::SceneLoad, format!("Scene #{k}"), n, out);
                format!(
                    "Scene #{k} loaded (previous scene unloaded in {})",
                    format_duration(d)
                )
            } else {
                format!("Scene unloaded in {}", format_duration(d))
            };
            self.emit(summary(Category::SceneLoad, message, n), out);
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
            let e = summary(
                Category::DomainReload,
                format!("Domain reload took {}", format_duration(d)),
                n,
            );
            let mut p = Pending::new(e)
                .closing(SectionKind::DomainReload, Some(d))
                .block(Block::ReloadProfile);
            p.profile = Some(ReloadProfile {
                line: n,
                total: d,
                phases: Vec::new(),
            });
            self.start(p, out);
            return;
        }

        if text.starts_with("Reloading assemblies for play mode.") {
            self.flush(out);
            self.begin_play_mode(n, out);
            let k = self.play_mode_count;
            self.emit(
                summary(Category::PlayMode, format!("Entering play mode (#{k})"), n),
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
                    let e = summary(
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
                    Pending::new(summary(
                        Category::PlayMode,
                        format!("Play mode #{k} scene loaded"),
                        n,
                    ))
                }
            } else {
                Pending::new(summary(
                    Category::SceneLoad,
                    format!("Loaded scene {}", file_name(scene)),
                    n,
                ))
            };
            self.start(p.block(Block::SceneLoad), out);
            return;
        }

        if let Some(c) = SCENE_OPENING.captures(text) {
            let e = summary(
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
            let e = summary(
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
                        summary(
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
            self.emit(summary(Category::AssetPipeline, message, n), out);
            return;
        }

        if let Some(u) = parse_unloading(text, n) {
            self.flush(out);
            for metric in u.metrics {
                out.push(ParseEvent::Metric(metric));
            }
            self.start(
                Pending::new(summary(Category::Memory, u.message, n)).fold("unload"),
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
            let e = summary(
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
                self.importing.clear();
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
        if text == "Build Report" {
            let e = summary(Category::Build, "Build report", n);
            self.start(Pending::new(e).mode(Mode::BuildReport), out);
            return;
        }
        if PROJECT_PATH.is_match(text) {
            self.emit(info(Category::System, text, n), out);
            return;
        } else if text == "COMMAND LINE ARGUMENTS:" {
            let e = info(Category::System, "Command line arguments", n);
            self.start(Pending::new(e).mode(Mode::CommandLine), out);
            return;
        } else if matches!(text, "Direct3D:" | "OpenGL:" | "Vulkan:" | "Metal:") {
            let e = summary(Category::Graphics, "Graphics device", n);
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
        if self.is_player() {
            if let Some(c) = ENGINE_VERSION.captures(text) {
                out.push(ParseEvent::Fact(Fact::new(
                    FactKey::EngineVersion,
                    &c["version"],
                    n,
                )));
            } else if let Some(c) = PLAYER_DATA
                .captures(text)
                .or_else(|| PLAYER_MONO_PATH.captures(text))
            {
                out.push(ParseEvent::Fact(Fact::new(
                    FactKey::GamePath,
                    &c["path"],
                    n,
                )));
            }
            return;
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

impl LogParser for UnityParser {
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
        self.last_line = line_no;
        let text = line.trim_end();
        if text.is_empty() {
            self.on_blank(line_no, out);
            return;
        }
        self.scan_facts(line_no, text, out);
        self.scan_imports(line_no, text, out);
        self.scan_licensing(line_no, text);
        if self.continue_pending(line_no, text, out) {
            return;
        }
        self.on_line(line_no, text, out);
    }

    fn finish(&mut self, out: &mut Vec<ParseEvent>) {
        self.flush(out);
        self.final_hints(out);
        if let Some(unfinished) = self.unfinished() {
            out.push(ParseEvent::Unfinished(unfinished));
        }
        // Sections still open are closed at the last line by `LogDocument::finish`.
    }

    fn boxed_clone(&self) -> Box<dyn LogParser> {
        Box::new(self.clone())
    }
}

/// Plain words for Unity's domain reload steps, and whether the step runs
/// the project's code. Unknown steps keep their engine name only.
fn phase_about(name: &str) -> (Option<&'static str>, bool) {
    match name {
        "ProcessInitializeOnLoadAttributes" => (
            Some("[InitializeOnLoad] classes: static constructors in your scripts and packages"),
            true,
        ),
        "ProcessInitializeOnLoadMethodAttributes" => (
            Some("[InitializeOnLoadMethod] methods in your scripts and packages"),
            true,
        ),
        "DisableScriptedObjects" => (
            Some("OnDisable on scripted objects before the reload"),
            true,
        ),
        "AwakeInstancesAfterBackupRestoration" => (
            Some("OnEnable / Awake on scripted objects after the reload"),
            true,
        ),
        "BeforeProcessingInitializeOnLoad" => (Some("preparing [InitializeOnLoad]"), false),
        "InitializePlatformSupportModulesInManaged" => (
            Some("platform support modules (one per installed platform)"),
            false,
        ),
        "LoadAssemblies" => (Some("loading the compiled assemblies"), false),
        "TypeCache.ScanAssembly" => (Some("scanning assemblies for types and attributes"), false),
        "CreateAndSetChildDomain" => (Some("creating the new script domain"), false),
        "RebuildCommonClasses" | "RebuildNativeTypeToScriptingClass" => {
            (Some("the engine rebinding its script classes"), false)
        }
        "initialDomainReloadingComplete" => (Some("the engine finishing the reload"), false),
        _ => (None, false),
    }
}

/// "Build report: 106.8 MB complete build, largest asset Assets/Big.png (8.0 MB)".
fn build_report_message(report: &BuildReport) -> String {
    let mut message = "Build report".to_owned();
    if let Some(total) = report.total {
        message.push_str(&format!(": {} complete build", format_bytes(total)));
    }
    if let Some(top) = report.assets.first() {
        message.push_str(&format!(
            ", largest asset {} ({})",
            top.name,
            format_bytes(top.bytes)
        ));
    }
    message
}

macro_rules! flavored_parser {
    ($(#[$doc:meta])* $name:ident => $flavor:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone)]
        pub struct $name(UnityParser);

        impl $name {
            pub fn new() -> Self {
                Self(UnityParser::new($flavor))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl LogParser for $name {
            fn feed_line(&mut self, line_no: usize, line: &str, out: &mut Vec<ParseEvent>) {
                self.0.feed_line(line_no, line, out);
            }

            fn finish(&mut self, out: &mut Vec<ParseEvent>) {
                self.0.finish(out);
            }

            fn boxed_clone(&self) -> Box<dyn LogParser> {
                Box::new(self.clone())
            }
        }
    };
}

flavored_parser!(
    /// Streaming parser for `Editor.log` / `Editor-prev.log`. Feed it with
    /// [`comte_core::parse_file`] or line by line for live tail.
    EditorParser => Flavor::Editor
);

flavored_parser!(
    /// Streaming parser for `Player.log` / `Player-prev.log` of built games.
    PlayerParser => Flavor::Player
);

/// `Crash!!!`, `Received signal SIGSEGV`, … → the crash entry's message.
fn crash_start(text: &str) -> Option<String> {
    if text.starts_with("Crash!!!") || text.starts_with("========== OUTPUTTING STACK TRACE") {
        Some("Native crash".to_owned())
    } else if text.starts_with("Received signal")
        || text.starts_with("Caught fatal signal")
        || text.starts_with("Native stacktrace:")
    {
        Some(format!("Native crash: {text}"))
    } else {
        None
    }
}

/// A native crash frame:
/// `0x00007FF6A1B2C3D4 (UnityPlayer) [C:\…\File.cpp:123] Function` (Windows) or
/// `#3  0x7f12345678 in Function` (Linux / Android).
///
/// Frames from `GameAssembly` (IL2CPP-compiled game code) count as user code.
fn native_frame(text: &str) -> Option<StackFrame> {
    if let Some(c) = NATIVE_FRAME.captures(text) {
        let module = c["module"].trim();
        let func = c["func"].trim();
        let label = if func.is_empty() {
            module.to_owned()
        } else {
            format!("{func} ({module})")
        };
        let kind = if module.eq_ignore_ascii_case("GameAssembly") {
            FrameKind::User
        } else {
            FrameKind::Native
        };
        let mut frame = StackFrame::new(label, kind).with_raw(text.trim());
        if let Some(path) = c.name("path") {
            let mut location = SourceRef::new(path.as_str().replace('\\', "/"));
            if let Some(line) = c.name("line").and_then(|l| l.as_str().parse().ok()) {
                location = location.with_line(line);
            }
            frame.location = Some(location);
        }
        return Some(frame);
    }
    POSIX_FRAME
        .captures(text)
        .map(|c| StackFrame::new(c["func"].trim(), FrameKind::Native).with_raw(text.trim()))
}

/// Lines that carry their own meaning and must not be swallowed by a fold group.
fn is_structural(text: &str, n: usize) -> bool {
    parse_unloading(text, n).is_some() || COMPLETED_RELOAD.is_match(text)
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
    // `Debug.LogException` repeats the exception's top frame in Unity's own
    // format right after the Mono frames; keep it once.
    if entry
        .stack
        .last()
        .is_some_and(|last| last.text == frame.frame.text && last.location == frame.frame.location)
    {
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

    use comte_core::{LogDocument, parse_reader};

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
    fn play_mode_after_shutdown_marker_is_not_nested_in_it() {
        let doc = parse(
            "Input System module state changed to: ShutdownInProgress.
Reloading assemblies for play mode.
hello
",
        );
        let play = doc
            .sections()
            .iter()
            .find(|s| s.kind == SectionKind::PlayMode)
            .unwrap();
        assert_eq!(play.depth, 0);
        assert_eq!(play.parent, None);
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

    #[test]
    fn imports_are_timed_per_worker_and_on_one_line() {
        let doc = parse(concat!(
            "[Worker0] Start importing Assets/Scenes/Boot.unity using Guid(6888a054a369c1d4) Importer(815301076,1909f56b) 
",
            "[Worker0] (PreviewImporter)
",
            "[Worker1] Start importing Assets/Models/Ship.fbx using Guid(a35eda5456421f74) Importer(2089858483,b0f066a2) 
",
            "[Worker1] (ScriptedImporter)
",
            "[Worker0]  -> (artifact id: 'd440876f2fcf5ab7') in 0.0057372 seconds
",
            "[Worker1]  -> (artifact id: 'fbaa3db831b5e903') in 2.5 seconds
",
            "Start importing Assets/Fonts/Sans.asset using Guid(2e498d1c8094) (NativeFormatImporter) -> (artifact id: 'a94a4c034600') in 0.0061 seconds
",
        ));
        let imports: Vec<(&str, u128, usize)> = doc
            .imports()
            .iter()
            .map(|i| (i.path.as_str(), i.duration.as_millis(), i.line))
            .collect();
        assert_eq!(
            imports,
            [
                ("Assets/Scenes/Boot.unity", 5, 1),
                ("Assets/Models/Ship.fbx", 2500, 3),
                ("Assets/Fonts/Sans.asset", 6, 7),
            ]
        );
        assert!(doc.unfinished().is_none(), "every import finished");
    }

    #[test]
    fn a_log_ending_in_an_import_says_which_asset() {
        let doc = parse(concat!(
            "[Worker0] Start importing Assets/a.png using Guid(01) Importer(1,2) 
",
            "[Worker1] Start importing Assets/Models/Huge.fbx using Guid(02) Importer(1,2) 
",
            "[Worker0]  -> (artifact id: 'aa') in 0.01 seconds
",
            "[Worker0] Start importing Assets/b.png using Guid(03) Importer(1,2) 
",
        ));
        let stopped = doc.unfinished().expect("the log ends mid-import");
        assert_eq!(
            stopped.headline,
            "Stopped while importing Assets/Models/Huge.fbx"
        );
        assert_eq!(stopped.subject.as_deref(), Some("Assets/Models/Huge.fbx"));
        assert_eq!(stopped.line, 2);
        assert_eq!(stopped.also, ["importing Assets/b.png"]);
    }

    #[test]
    fn a_domain_reload_counts_as_stopped_only_at_the_very_end() {
        let stuck = parse(
            "Begin MonoManager ReloadAssembly
- Loaded All Assemblies, in  0.3 seconds
",
        );
        assert_eq!(
            stuck.unfinished().map(|u| u.headline.as_str()),
            Some("Stopped during a domain reload")
        );

        let mut busy = String::from(
            "Begin MonoManager ReloadAssembly
",
        );
        for i in 0..60 {
            busy.push_str(&format!(
                "[Flow] step {i}
"
            ));
        }
        assert!(
            parse(&busy).unfinished().is_none(),
            "plenty happened after it"
        );

        let finished = parse(
            "Begin MonoManager ReloadAssembly
Domain Reload Profiling: 1304ms
",
        );
        assert!(finished.unfinished().is_none());

        let quit = parse(
            "Begin MonoManager ReloadAssembly
Cleanup mono
",
        );
        assert!(
            quit.unfinished().is_none(),
            "a clean shutdown is not a hang"
        );
    }

    #[test]
    fn crash_symbol_noise_is_counted_and_the_crash_is_triaged() {
        let text = concat!(
            "Crash!!!\n",
            "SymInit: Symbol-SearchPath: '.', symOptions: 534, UserName: 'player'\n",
            "C:\\Game\\UnityPlayer.dll:UnityPlayer.dll (00007FFB2A6B0000), size: 28655616 (result: 0), SymType: '-exported-', PDB: ''\n",
            "C:\\Windows\\System32\\nvwgf2umx.dll:nvwgf2umx.dll (00007FFB10000000), size: 1000 (result: 0), SymType: '-exported-', PDB: ''\n",
            "========== OUTPUTTING STACK TRACE ==================\n",
            "0x00007FFB10001234 (nvwgf2umx) (function-name not available)\n",
            "  ERROR: SymGetSymFromAddr64, GetLastError: 'Attempt to access invalid address.' (Address: 00007FFB10001234)\n",
            "0x00007FFB2ABF1E3D (UnityPlayer) GfxDeviceD3D11::Present\n",
            "========== END OF STACKTRACE ===========\n",
            "A crash has been intercepted by the crash handler. For call stack and other details, see the latest crash report generated in:\n",
            " * C:/Temp/Studio/Game/Crashes\n",
        );
        let doc = parse_reader(Cursor::new(text), &mut PlayerParser::new()).unwrap();
        let crash = &doc.entries()[0];
        assert_eq!(crash.stack.len(), 2);
        assert!(
            !crash
                .details
                .iter()
                .any(|d| d.contains("SymGetSymFromAddr64"))
        );
        assert!(
            crash
                .details
                .contains(&"2 modules loaded (2 without symbols)".to_owned())
        );
        let hint = doc.hints_for(crash).next().expect("triaged");
        assert_eq!(
            hint.title,
            "Crashed inside the NVIDIA graphics driver (nvwgf2umx)"
        );
        assert_eq!(hint.path.as_deref(), Some("C:/Temp/Studio/Game/Crashes"));
    }

    #[test]
    fn licensing_is_a_problem_only_if_it_never_connected() {
        let failed = parse(concat!(
            "[Licensing::Module] Trying to connect to existing licensing client channel...\n",
            "[Licensing::Module] Channel doesn't exist: \"LicenseClient-dev\"\n",
            "[Licensing::Module] Error: Timed-out after 60.00s, waiting for channel\n",
        ));
        assert_eq!(doc_titles(&failed), ["Unity couldn't get a license"]);
        let recovered = parse(concat!(
            "[Licensing::Module] Channel doesn't exist: \"LicenseClient-dev\"\n",
            "[Licensing::Module] Successfully connected to LicensingClient on channel: \"LicenseClient-dev\"\n",
            "[Licensing::Client] Error: Timed-out after 30.00s, waiting for AccessTokenResponse\n",
        ));
        assert!(doc_titles(&recovered).is_empty(), "healthy logs are noisy");
    }

    fn doc_titles(doc: &LogDocument) -> Vec<String> {
        doc.hints().iter().map(|h| h.title.clone()).collect()
    }

    #[test]
    fn reload_profiling_trees_become_phase_breakdowns() {
        let tree = concat!(
            "Begin MonoManager ReloadAssembly\n",
            "Domain Reload Profiling: 1185ms\n",
            "\tBeginReloadAssembly (190ms)\n",
            "\t\tDisableScriptedObjects (20ms)\n",
            "\t\tCreateAndSetChildDomain (14ms)\n",
            "\tLoadAllAssembliesAndSetupDomain (242ms)\n",
            "\t\tLoadAssemblies (171ms)\n",
            "\t\tAnalyzeDomain (238ms)\n",
            "\t\t\tTypeCache.Refresh (235ms)\n",
            "\t\t\t\tTypeCache.ScanAssembly (217ms)\n",
            "\tFinalizeReload (614ms)\n",
            "\t\tSetupLoadedEditorAssemblies (556ms)\n",
            "\t\t\tProcessInitializeOnLoadAttributes (400ms)\n",
            "\t\t\tProcessInitializeOnLoadMethodAttributes (97ms)\n",
        );
        let doc = parse(&format!("{tree}{tree}"));
        let profile = &doc.reload_profiles()[0];
        assert_eq!(profile.total.as_millis(), 1185);
        assert_eq!(profile.phases.len(), 12);
        let leaves: Vec<&str> = profile.leaves().map(|p| p.name.as_str()).collect();
        assert_eq!(
            leaves,
            [
                "DisableScriptedObjects",
                "CreateAndSetChildDomain",
                "LoadAssemblies",
                "TypeCache.ScanAssembly",
                "ProcessInitializeOnLoadAttributes",
                "ProcessInitializeOnLoadMethodAttributes",
            ]
        );
        let insights = comte_core::Insights::of(&doc);
        let top = &insights.reload_phases[0];
        assert_eq!(top.name, "ProcessInitializeOnLoadAttributes");
        assert!(top.your_code);
        assert_eq!((top.total.as_millis(), top.average.as_millis()), (800, 400));
        assert_eq!(doc.reload_profiles().len(), 2);
    }

    #[test]
    fn rules_fire_once_per_log() {
        let doc = parse(concat!(
            "Failed to present D3D11 swapchain due to device reset/removed.\n",
            "[Flow] retry\n",
            "Failed to present D3D11 swapchain due to device reset/removed.\n",
        ));
        assert_eq!(doc.hints().len(), 1);
        assert_eq!(doc.hints()[0].line, 1);
        assert_eq!(doc.counts().error, 2, "both lines are errors, not chatter");
    }

    #[test]
    fn live_tail_sees_an_import_on_the_unfinished_last_line() {
        // Unity adds the result to the `Start importing` line once the import
        // is done, so while it runs the line has no newline yet.
        let mut tail = comte_core::TailDocument::new(Box::new(EditorParser::new()));
        tail.push_bytes(b"Refreshing native plugins\nStart importing Assets/Big.fbx using Guid(01) Importer(1,2) ");
        assert_eq!(
            tail.snapshot().unfinished().map(|u| u.headline.clone()),
            Some("Stopped while importing Assets/Big.fbx".to_owned())
        );
        tail.push_bytes(b" -> (artifact id: 'ab') in 4.5 seconds\n");
        let done = tail.snapshot();
        assert!(done.unfinished().is_none());
        assert_eq!(done.imports()[0].path, "Assets/Big.fbx");
    }

    #[test]
    fn build_report_sizes_are_read() {
        let doc = parse(concat!(
            "Build Report
",
            "Uncompressed usage by category (Percentages based on user generated assets only):
",
            "Textures               12.0 mb	 60.0% 
",
            "Meshes                 0.0 kb	 0.0% 
",
            "Scripts                8.0 mb	 40.0% 
",
            "Total User Assets      20.0 mb	 100.0% 
",
            "Complete build size    106.5 mb
",
            "Used Assets and files from the Resources folder, sorted by uncompressed size:
",
            " 8.0 mb	 40.0% Assets/Textures/Sky Box.png
",
            " 512.0 kb	 2.5% Assets/Audio/theme.ogg
",
            "-------------------------------------------------------------------------------
",
            "Build completed
",
        ));
        let report = doc.build_reports().last().expect("one report");
        let mb = 1024 * 1024;
        assert_eq!(report.total, Some(106 * mb + mb / 2));
        assert_eq!(report.user_assets, Some(20 * mb));
        assert_eq!(report.categories.len(), 3);
        assert_eq!(report.categories[0].name, "Textures");
        assert_eq!(report.assets[0].name, "Assets/Textures/Sky Box.png");
        assert_eq!(report.assets[1].bytes, 512 * 1024);
        let entry = &doc.entries()[0];
        assert_eq!(
            entry.message,
            "Build report: 106.5 MB complete build, largest asset Assets/Textures/Sky Box.png (8.0 MB)"
        );
        assert_eq!(entry.line_range, 1..12, "the whole table is one entry");
        assert_eq!(doc.entries()[1].message, "Build completed");
    }
}
