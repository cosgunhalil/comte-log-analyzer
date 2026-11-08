# Comte Logger — Application Design

> *"Savoir pour prévoir"* — Auguste Comte. Know the log so you can predict the bug.

Comte Logger is a Windows desktop log analyzer for game engines, written in Rust.
**v1 targets Unity**. The architecture leaves room for Unreal and Godot later.

**Main goal:** turn noisy Unity logs (mainly the Editor log, plus Player logs) into something a person can read, then save and share them with notes.

---

## 0. Decisions so far

| Topic | Decision |
|---|---|
| Primary use | Post-mortem debugging, QA and teammates sharing logs, player bug reports |
| Problems to fix first | Engine noise, unreadable stack traces, no timeline, hidden performance clues |
| GUI | `egui` / `eframe` (pure Rust, single `.exe`) |
| Saved session | Plain `.log` file with a notes header (any text editor can open it) |
| Notes | One session note + tags |
| Privacy | Redact by default, can be turned off for each save |
| Live tail | In the MVP, built right after the viewer |
| Timeline | Grouped, collapsible sections in the entry list |
| Platforms (v1) | Windows only |
| v1 extras | Insights panel, open `file:line` in an IDE, compare two logs, Markdown report export |

---

## 1. Log sources (Windows)

| Source              | Path                                                                  |
|---------------------|-----------------------------------------------------------------------|
| Editor log          | `%LOCALAPPDATA%\Unity\Editor\Editor.log`                              |
| Previous Editor log | `%LOCALAPPDATA%\Unity\Editor\Editor-prev.log`                         |
| Player log          | `%USERPROFILE%\AppData\LocalLow\<Company>\<Product>\Player.log`       |
| Previous Player log | `%USERPROFILE%\AppData\LocalLow\<Company>\<Product>\Player-prev.log`  |
| Crash reports       | `…\LocalLow\<Company>\<Product>\Crashes\` (when present)              |
| Any file            | **Open file…** or drag and drop (logs sent by testers or players, saved sessions) |

- **Player discovery** (`comte_unity::UnityLocator`): scan `LocalLow\<Company>\<Product>\` for exactly `Player.log` and `Player-prev.log` (other names, such as CEF's `Player-browser.log`, are skipped). Each game is listed as Company › Product with its size and last-modified time. Games are sorted by their newest log, and each game's current log comes before its previous one. Unreadable folders are skipped silently.
- **Editor logs** are always listed, even when missing (`size: None`), so the UI can show them greyed out.
- **LocalLow** is found as the sibling of `%LOCALAPPDATA%`, falling back to `%USERPROFILE%\AppData\LocalLow`.
- **File-kind detection** (`comte_core::detect_engine` + `EngineSupport::detect`): read the first 64 lines and ask each engine in turn. A file no engine recognizes opens with the plain-text parser. File names are never trusted, because testers rename files.
- macOS and Linux roots are wired up in the locator but untested until M12.
- Logs that players and QA send usually arrive as loose files, so drag and drop is a first-class way to open one.

---

## 2. Architecture

A Cargo workspace. Engine-specific code stays out of the core and the GUI.

```
comte-log-analyzer/
├── Cargo.toml                    # [workspace]
├── crates/
│   ├── comte-core/               # model, traits, sections, filters, redaction,
│   │                             # session save/load, Markdown export, diff
│   ├── comte-unity/              # Unity locator + Editor parser + Player parser
│   └── comte-app/                # egui GUI → binary "comte-logger"
├── docs/
└── crates/comte-unity/tests/fixtures/   # small, redacted samples
```

```
            ┌──────────────┐
            │  comte-app   │  egui UI · worker thread · file watcher
            └──────┬───────┘
     ┌─────────────┴──────────────┐
     ▼                            ▼
┌────────────┐  implements  ┌──────────────┐
│ comte-core │◄─────────────│ comte-unity  │   (later: comte-unreal, comte-godot)
└────────────┘              └──────────────┘
```

### Core traits

```rust
pub trait LogLocator {
    fn discover(&self) -> Vec<LogSource>;          // Editor, Editor-prev, Players…
}

/// Streaming and stateful: one Unity entry can span many lines.
/// The same parser serves full files, huge files and live tail.
/// One line can produce several events (e.g. close an entry AND open a
/// section), so events are pushed into a reusable buffer.
pub trait LogParser: Send {
    fn feed_line(&mut self, line_no: usize, line: &str, out: &mut Vec<ParseEvent>);
    fn finish(&mut self, out: &mut Vec<ParseEvent>);
}

pub enum ParseEvent {
    Entry(LogEntry),
    SectionStart(SectionStart),  // "Play mode #2", "Script compilation", … (may nest)
    SectionEnd { line: usize, duration: Option<Duration> }, // closes innermost
    Fact(Fact),                  // Unity version, GPU, OS… → info card
    Metric(Metric),              // timings and counts → insights panel
}

pub trait EngineSupport {
    fn id(&self) -> &'static str;
    fn locator(&self) -> Box<dyn LogLocator>;
    fn detect(&self, first_lines: &[&str]) -> Option<LogKind>; // Editor or Player?
    fn new_parser(&self, kind: LogKind) -> Box<dyn LogParser>;
}
```

`LogDocument` (in `comte-core`) consumes the events. It assigns entry IDs, section IDs, parents and depths, each entry's section and fingerprint, and keeps severity counts per section (including nested ones) and overall. Line numbers are 1-based; ranges are half-open.

`detect` lets a dropped file open with the right parser. For example, `Initialize engine version:` means a Player log, and `Built from '…' branch` means an Editor log.

### Threading
- The UI thread only draws.
- A worker thread reads the file (using a byte offset) and sends `ParseEvent` batches over `crossbeam-channel`.
- Live tail: `notify` watches the file, then the app reads from the last offset. When the file shrinks (Unity restarted and truncated it), the app shows "log restarted" and parses it again.

---

## 3. Data model

```rust
pub struct LogEntry {
    pub id: u64,
    pub line_range: Range<usize>,      // position in the original file
    pub section_id: Option<u32>,       // which timeline section it belongs to
    pub severity: Severity,            // Crash | Error | Exception | Warning | Info
    pub category: Category,
    pub message: String,
    pub details: Vec<String>,
    pub stack: Vec<StackFrame>,
    pub source: Option<SourceRef>,     // file + line when known
    pub fingerprint: u64,              // used to collapse duplicates
}

pub struct StackFrame {
    pub text: String,                  // cleaned: "GameInstaller.Report(TextData)"
    pub location: Option<SourceRef>,   // Assets/…/GameInstaller.cs:84
    pub kind: FrameKind,               // User | Package | Engine | Native
}

pub struct Section {                   // timeline block
    pub id: u32,
    pub kind: SectionKind,             // Startup | Compile | DomainReload | AssetRefresh
                                       // | PlayMode | SceneLoad | Build | Shutdown | Crash
    pub title: String,                 // "Play mode #2"
    pub line_range: Range<usize>,
    pub duration: Option<Duration>,
    pub counts: SeverityCounts,        // errors and warnings inside this block
}

pub enum Category {
    UserLog, Exception, Compiler, Crash,
    AssetPipeline, DomainReload, PlayMode, SceneLoad, Memory,
    PackageManager, Licensing, Build, Shader, Graphics, Input, System, Other,
}
```

**Noise policy:** each category has a default visibility. `Licensing`, `PackageManager`, profiling detail, `Input` and `Memory` are **hidden by default**. A single "Show engine noise" toggle shows them again.

---

## 4. Parsing rules

The rules below come from a real Unity 6000.3 `Editor.log` and from 2019.4 `Player.log` files found on this machine.
Stack frame syntax differs between versions, so the parser accepts both forms:
- 6000.x: `UnityEngine.Debug:Log (object)`
- 2019.x: `UnityEngine.Debug:Log(Object)`

### 4.1 Editor.log

| Pattern | Result |
|---|---|
| Message → `UnityEngine.Debug:ExtractStackTraceNoAlloc…` → frames → `(Filename: X Line: N)` → blank line | **UserLog**. Severity comes from `Debug:Log` / `LogWarning` / `LogError` / `LogException`. The internal frames (`StackTraceUtility`, `DebugLogHandler`, `Logger`) are dropped. |
| `Assets/Foo.cs(12,5): error CS0103: …` | **Compiler** error or warning, with a location |
| `<Type>Exception: msg` followed by frames | **Exception** |
| `[Licensing::*] …` and its indented continuation lines | **Licensing**, folded into one entry |
| `[Package Manager] …` | **PackageManager** |
| `Asset Pipeline Refresh (id=…): Total: X seconds - Initiated by …` | **AssetPipeline** entry ("Asset refresh took 3.53 s (initiated by …)") and metric. It is not a section, because Unity only prints this line once the refresh has finished. |
| `Begin MonoManager ReloadAssembly` … `Domain Reload Profiling: Nms` | **DomainReload** section and metric; the profiling tree is folded |
| `[ScriptCompilation] Requested…` or `Starting: …bee_backend…` … `*** Tundra build success/failed (X seconds)` | **Compile** section and metric; Bee progress lines are folded; a failed build is an Error |
| `Reloading assemblies for play mode.` | starts a **PlayMode #n** section |
| `Loaded scene 'Temp/__Backupscenes/…'` | Unity loads the backup scene **twice** per play session. The 1st load is the play scene being loaded; the 2nd is the exit, which closes the section. With Enter Play Mode Options (no domain reload) there is no "Reloading assemblies" line, so the 1st load is what starts the session. |
| `Loaded scene 'X'` + `Total Operation Time: N ms` | **SceneLoad** entry and metric |
| `Unloading N unused Assets / (size). Loaded Objects now: M.` | **Memory** entry and metrics |
| `[Project] Loading completed in X seconds` | closes **Startup** with its duration |
| `Cleanup mono` / `Input System … ShutdownInProgress` | closes everything and opens **Shutdown** |
| First lines: `Built from…`, `OS:`, `Physical Memory`, `Date:`, `Successfully changed project path to:`, the `Direct3D:` block | **Facts** for the info card |

**Fold groups:** consecutive lines that share a prefix (`[Licensing::*]`, `[Package Manager]`, Bee output, Mono reload chatter, `[Tag] …`) merge into one entry, and indented lines become details of the entry above them. This turns 4,195 real lines into about 255 entries.

**Known limit:** a `Debug.Log` message is the single line right above its stack trace. If a message has several lines, only its last line becomes the entry's message; the earlier lines become entries of their own.

### 4.2 Player.log

| Pattern | Result |
|---|---|
The Editor and Player parsers are one shared `UnityParser` with an Editor or Player flavor. Most rules (Debug.Log traces, exceptions, `(Filename: …)`, memory, graphics, crashes) are shared.

| Pattern | Result |
|---|---|
| `Initialize engine version: 2019.4.40f1 (…)` | Fact: Unity version; this line also identifies the file as a Player log |
| `Loading player data from …/X_Data/data.unity3d` or `Mono path[0] = '…/X_Data/Managed'` | Fact: game install folder |
| `Direct3D:` (also `OpenGL:`, `Vulkan:`, `Metal:`) block: `Version`, `Renderer`, `VRAM`, `Driver` | Facts: graphics API, GPU, VRAM, driver |
| `Begin MonoManager ReloadAssembly` → `- Completed reload, in X seconds` | Domain reload section nested in **Startup**, plus a **Startup** metric ("Scripts loaded in 8.14 s") |
| `UnloadTime: X ms` | Unity prints this once a new scene has finished loading. It closes Startup and the previous scene, and opens a **Scene #n** section, plus a scene-unload metric. Player logs rarely name scenes, so sections are numbered. |
| `Unloading N unused Assets to reduce memory usage. Loaded Objects now: M.` | **Memory** entry and metrics |
| `message` + `(Filename: …/Debug.bindings.h Line: N)`, possibly with a `" "` line between them | A release-build `Debug.Log` without a stack trace: a **UserLog**, severity Info (the log does not say). `Debug.bindings.h` is not kept as a location. |
| `message` + `(Filename: <engine path or empty> Line: N)` | A native engine message; the category stays Other |
| `Exception: msg` + `  at Ns.Class.Method (…) [0x00046] in <hash>:0`, sometimes followed by Unity-format `Debug.LogException` frames | **Exception**. Release frames have no paths, so they are classified by namespace (known third-party prefixes such as `Steamworks.` count as packages). The frame `LogException` repeats is kept once. |
| `Uploading Crash Report` | **Not a crash.** Unity Cloud Diagnostics is reporting the exception logged on the next line. Shown as "Exception reported to Unity Cloud Diagnostics". |
| `Crash!!!` / `========== OUTPUTTING STACK TRACE` … `========== END OF STACKTRACE` | **Crash** section and entry. `0x… (Module) [file:line] Function` lines become native frames; `GameAssembly` (IL2CPP game code) frames count as user code. |
| `Received signal SIGSEGV`, `Caught fatal signal`, `Native stacktrace:` + `#N 0x… in func` | **Crash** (Linux/Android style), with no end marker; capped at 400 lines |
| `A crash has been intercepted by the crash handler…` + ` * <folder>` | Fact: crash report folder |

Checked against 30 real Player logs (Unity 2019.4 to 6000.x builds): every file parses without errors, in 0.4 to 87 ms each. Native crash formats are covered by reconstructed fixtures, because none of the local logs contain one.

### 4.3 Stack trace readability
- Classify each frame as **User** (`Assets/`), **Package** (`Packages/`, `Library/PackageCache`, and `Assets/Plugins/`, which usually holds third-party code), **Engine** (`UnityEngine.*`, `UnityEditor.*`, `System.*`) or **Native**. Frames without a path (release builds) are classified by namespace.
- Make compiler-generated names readable: `Foo/<Run>d__4:MoveNext` → `Foo.Run() [async]`, `Foo/<>c:<Start>b__0_0` → `Foo.Start() [lambda]`.
- Show user frames in bold, fold engine frames into "▸ 6 engine frames", and drop the logging plumbing frames.
- Clean the signature: `SpiritCabinet.Installers.GameInstaller:Report (SpiritCabinet.Text.Import.TextData)` becomes `GameInstaller.Report(TextData)`, with the full namespace in a tooltip.
- The **culprit** is the first user frame. It appears on the entry row as `GameInstaller.cs:84`.

---

## 5. Insights panel

Computed after parsing:
- **Verdict line**, for example: "💥 Crashed in Play mode #3 after NullReferenceException in EnemySpawner.cs:57", or "✅ Clean session, 2 warnings".
- Counts by severity and category.
- **Top repeated messages** (by fingerprint) with counts.
- **Slowest items**: domain reloads, asset refreshes, compiles and scene unloads, with a small bar per item.
- Device/Editor info card: Unity version, OS, RAM, GPU, driver.
- Clicking any item jumps to its entry or section in the list.

---

## 6. Saving and sharing

### 6.1 Saved session (`.log`)

```
# ==== Comte Logger session ====================================
# comte_version: 0.1.0
# source:        Player.log (Flanne/MinutesTillDawn)
# kind:          unity-player
# saved_at:      2026-11-08T14:05:00+03:00
# unity:         2019.4.40f1
# content:       filtered (severity>=Warning, noise hidden)
# redacted:      yes
# tags:          crash, steam, build-1.2.3
# --- note -----------------------------------------------------
# Crashes when Steam is not running. Repro: start the game from
# the Epic launcher with Steam closed.
# ==============================================================
<log content: the original lines, unchanged apart from redaction>
```

- **Content**: the full log, or only the entries you can currently see (written as their original lines, so the file is still a valid log).
- Comte Logger reads the header back when you reopen the file, restoring the note, tags and kind (no detection needed).
- Default folder: `Documents\ComteLogger\sessions\<yyyy-MM-dd_HH-mm>_<product-or-editor>.log`, changeable in settings.

### 6.2 Redaction (on by default)
The real logs on this machine contain values that must not be shared:

| What | Example seen in real logs | Replaced with |
|---|---|---|
| Windows user name in paths | `C:\Users\cosgu\…` | `C:\Users\<user>\…` |
| Launcher auth arguments | `-AUTH_PASSWORD=…`, `-epicuserid=…`, `-epicusername="…"` | `-AUTH_PASSWORD=<redacted>` |
| Licensing IDs | `Machine Id:`, `Session Id:`, `Correlation Id:` | `<redacted>` |
| Access tokens | `access token kJw97zns…` | `<token>` |
| Emails and IPs | | `<email>`, `<ip>` |
| GUID-like user or save IDs (32 hex characters) | `Saves\b120cfa6…\Profile.json` | `<id>` |

The rules live in `comte-core`, are tested with fixtures, and show a "N values redacted" summary before saving.

### 6.3 Markdown report export
A short report to paste into Jira or Slack: verdict, info card, the note and tags, the top errors with their culprit frame and a trimmed stack, and the slowest items. It is redacted with the same rules.

---

## 7. Compare two logs

Choose **A** and **B** (by default `Editor-prev.log` vs `Editor.log`). The comparison shows:
- **New**, **gone** and **common** errors and warnings, matched by fingerprint, with counts.
- Changes in timings, for example: "domain reload average 1.3 s → 2.5 s (+92%)".
- Changes in facts (Unity version, GPU, driver).

---

## 8. Open in IDE

- Clicking a user frame or a culprit location opens the file at that line.
- The project root comes from the Editor log (the project path line) or is chosen once in settings.
- Supported editors, configurable: **Rider** (`rider64.exe --line N file`), **VS Code** (`code -g file:N`), **Visual Studio** (`devenv /edit file`), or a custom command template `{file} {line}`.
- Player log frames rarely have a path; those frames are not clickable.

---

## 9. UI layout

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ Comte Logger  [Source ▾ Editor.log] [⟳] [● Live] [⇄ Compare] [💾 Save] [⇪ MD] │
├───────────────┬───────────────────────────────────────────┬──────────────────┤
│ SOURCES       │ 🔍 search…  [⛔3][⚠12][ℹ240] ☐Collapse ☐Noise │ INSIGHTS ▾        │
│ Unity Editor  │───────────────────────────────────────────│ ✅ 3 errors,      │
│  • Editor.log │ ▾ Startup                    ⚠1      4.1s │   no crash       │
│  • Editor-prev│ ▸ Script compilation                 2.6s │ Slowest reload   │
│ Unity Players │ ▸ Domain reload                      1.8s │  ████ 2.5s       │
│  • Windbound  │ ▾ Play mode #1               ⛔2 ⚠3        │ Top repeated     │
│  • AKNY       │   ⛔ NullReferenceException  GameFlow.cs:42│  ×42 FlowTrace…  │
│ Saved sessions│   ⚠  Draft text allowed ×3  Installer:84  │──────────────────│
│  • crash-1.log│   ℹ  [Flow] Boot → MainMenu FlowTrace:26  │ DETAILS          │
│ [+ Open file] │ ▸ Play mode #2               ⛔1           │ ⛔ NullReference… │
│               │                                           │ ▸ GameFlow.Tick  │
│ TAGS          │                                           │   GameFlow.cs:42 │
│ crash · perf  │                                           │ ▸ 6 engine frames│
├───────────────┴───────────────────────────────────────────┴──────────────────┤
│ 4,012 lines · 318 entries · 3 errors · Unity 6000.3.11f1 · parsed 42 ms      │
└──────────────────────────────────────────────────────────────────────────────┘
```

- Sections can be collapsed. Each header shows its error and warning counts, its duration and its line range. When a log opens, sections with no problems start collapsed, except Play mode, Scene and Crash sections, where the game's own logs are.
- **Noise:** "Engine noise" off (the default) hides `LogEntry::is_engine_chatter()` entries. Those are everything in a noise category, plus Info entries that are neither user logs nor summaries. Parsers mark human-written messages ("Domain reload took 1.30 s") as `summary`, so these stay visible.
- **Collapse duplicates** (on by default) merges entries with the same fingerprint within a section into one row with a ×N badge. With it, TerraScape's 103 NullReferenceExceptions become 4 rows.
- Rows show the culprit on the right. For release builds without paths, they show the first user frame instead (`Gun.StopShooting()`).
- The details panel shows the message, where it happened, the section path ("During"), line numbers, details, the stack trace with engine and package runs folded, and the original log lines. With nothing selected, it shows the info card: facts, problem counts and a timeline overview.
- Open a log from the sources list, with Ctrl+O, by dropping a file on the window, or as a command-line argument. With no argument, the app opens `Editor.log`.
- Loading runs on a background thread (`std::sync::mpsc`; crossbeam was not needed). Only the latest request's result is shown.
- **Dev hooks** (`COMTE_SCREENSHOT=out.png`, `COMTE_SELECT=problem`, `COMTE_SEARCH=…`, `COMTE_NOISE=1`) let the UI screenshot itself in a given state and quit, for automated visual checks.
- The list is virtualized (only visible rows are drawn), so it stays fast on very large logs.
- Keyboard: `Ctrl+F` search, `F8` / `Shift+F8` next or previous problem (this opens collapsed sections), `↑` / `↓` move the selection, `Esc` clears it, `F5` reloads, `Ctrl+O` opens a file, `Ctrl+S` saves the session (M6).

---

## 10. Tech stack

| Concern | Crate |
|---|---|
| GUI | `eframe`, `egui`, `egui_extras` |
| Native dialogs, drag and drop | `rfd`, egui file drop |
| Parsing | `regex`, `std::sync::LazyLock` |
| Hashing (fingerprints) | built-in FNV-1a 64 (stable across runs, no dependency) |
| Diff | own fingerprint set comparison |
| File watching | `notify` |
| Threads | `crossbeam-channel` |
| Paths | `directories` |
| Settings | `serde`, `toml` |
| Time | `chrono` |
| Errors | `thiserror` (libraries), `anyhow` (app) |
| Our own logs | `tracing`, `tracing-subscriber` |
| Tests | unit tests and `insta` snapshots on fixtures |

---

## 11. Milestones

Each milestone ends with a suggested conventional commit. You commit it yourself.

| # | Milestone | Done when |
|---|---|---|
| M0 | Scaffold: Cargo project, git init, design | Builds and opens in RustRover ✅ |
| M1 | Workspace split, `comte-core` model and traits | Core types compile and are tested ✅ |
| M2 | Unity Editor parser with timeline sections and frame classification | Snapshot tests cover §4.1 and §4.3 ✅ |
| M3 | Unity Player parser (crash, device facts, exceptions, scene/memory) | Snapshot tests cover §4.2 ✅ |
| M4 | Locator, Player discovery and file-kind detection | Lists the real logs on this machine ✅ |
| M5 | GUI viewer: sources, sections, list, filters, search, details, collapse, noise toggle | `Editor.log` reads clearly ✅ |
| M6 | Save session: header, note, tags, redaction, reopen | Round trip keeps the note and tags; redaction tests pass |
| M7 | Live tail | New Unity output appears in under 1 s; truncation handled |
| M8 | Insights panel | Verdict, counts, slowest items, top repeated messages |
| M9 | Open in IDE | Clicking a frame opens Rider or VS Code at the line |
| M10 | Compare two logs | Editor-prev vs Editor diff view |
| M11 | Markdown report export | Report pastes cleanly into Jira or Slack |
| M12 | Polish: settings UI, icon, release build | `cargo build --release` gives a working `.exe` |

---

## 12. Conventions

- Rust edition 2024, MSRV 1.88; `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` clean.
- **The assistant never commits.** Each task ends with a suggested [Conventional Commits](https://www.conventionalcommits.org/) message.
- Real logs never go into the repo. Fixtures are trimmed and redacted, and `samples/private/` is git-ignored.

## 13. Notes for later milestones
- **M12 console window:** the binary still uses the console subsystem, so `--list` and `--summary` can print. Launched from Explorer, a console window opens behind the GUI. Fix this in M12, for example with a separate CLI binary or by attaching to the parent console.
- **M6 redaction:** the real Editor log's `COMMAND LINE ARGUMENTS` block contains the full `-accessToken` value and `-hubSessionId`, so redaction must cover them.
- **M5 culprit for release builds:** Player frames usually have no file path, so `culprit()` finds no location. The UI should fall back to showing the first user frame's name (for example `Gun.StopShooting()`).
- **M8 insights:** the crash verdict can use the section tree directly. A Crash section nested in "Scene #3" means "crashed in scene #3".
- **Compile metric:** this comes from Tundra's own duration. It can differ from the separate `AssetDatabase: script compilation time` line, which Unity sometimes prints before the compile starts.

## 14. Open questions (for later)
- macOS and Linux support after v1?
- Should saved sessions have a library view with search across them by tag?
- Unreal or Godot next?
