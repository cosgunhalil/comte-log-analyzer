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
- macOS and Linux roots are wired up in the locator but untested (see §13).
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
- **Live tail** (`comte-app::live` + `comte-core::TailDocument`): a background thread checks the file every 250 ms and reads only the bytes appended since the last offset. A half-written last line waits for its newline.
  - **Polling instead of `notify`:** a `stat` is nearly free, behaves the same on every file system, and still notices when Unity replaces `Editor.log` on restart, which change notifications can report late.
  - **Restart detection:** the file got shorter, or its first 512 bytes changed (a replaced file that is already longer). The log is then read again from the start, and the UI says "Log restarted".
  - **Snapshots:** `TailDocument` keeps the parser running between chunks. For display it finishes a *clone* of the parser (`LogParser::boxed_clone`), so an entry still waiting for its stack trace appears now and is completed correctly later. Tests check that feeding both fixtures in chunks of 1, 2, 17, … bytes, with a snapshot after every chunk, gives exactly the same document as a full parse.
  - **UI:** the "● Live" toggle (off by default, unavailable for saved sessions) keeps your selection, filters and collapsed sections, and collapses new quiet sections. The list sticks to the bottom unless you scroll up. The status bar shows "Live · +23 lines 4 s ago". F5 re-reads the file while staying live.
  - **Cost:** each update clones the document and its lines, which is fine for normal Editor logs (tens of MB). Very large files may need a cheaper snapshot later.

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
| `[WorkerN] Start importing <path> using Guid(…) …` … `[WorkerN]  -> (artifact id: '…') in X seconds`, or both on one line | an **import** with its duration (`ParseEvent::Import`). Imports are matched per worker; the lines still fold into Asset pipeline entries. |
| `Build Report` … size by category … `Complete build size` … `Used Assets …, sorted by uncompressed size:` … `-----` | one **Build** entry ("Build report: 164.2 MB complete build, largest asset …") and a `ParseEvent::BuildReport` with the totals, categories and up to 200 assets |
| First lines: `Built from…`, `OS:`, `Physical Memory`, `Date:`, `Successfully changed project path to:`, the `Direct3D:` block | **Facts** for the info card |

**Fold groups:** consecutive lines that share a prefix (`[Licensing::*]`, `[Package Manager]`, Bee output, Mono reload chatter, `[Tag] …`) merge into one entry, and indented lines become details of the entry above them. This turns 4,195 real lines into about 255 entries.

**Where it stopped:** at the end of the log, `finish` sends `ParseEvent::Unfinished` when something is still running:
- **An import** that never got its result. Unity writes the result onto the `Start importing` line once it is done, so a hung import is the last, unfinished line. The oldest one is named, and the others are listed as "also still running".
- **A domain reload or script compile** whose section is still open and started within the last 40 lines. A stuck editor writes nothing more, while a section the log merely never closed is followed by plenty of output.
- A shutdown clears imports and closes sections, so anything still open at the end started after it. Player logs don't use this.
- Live tail feeds the unfinished last line to a copy of the parser for each snapshot, so an import in progress shows up while nothing new is written. Entries and sections never start on that half line.

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

### 4.4 Unity rich text in messages
Games often style their own logs, e.g. `Debug.Log("<color='cyan'>Starting new race</color>")`. Unity's console shows that in color; the log file keeps the tags. Comte Logger renders it like the console does (`comte-core::markup`):

- **Tags:**
  - `<color=…>` accepts named colors (`cyan`, `red`, `lightblue`, …), `#RGB`, `#RGBA`, `#RRGGBB` and `#RRGGBBAA`, unquoted or in `'single'` or `"double"` quotes.
  - `<i>` is italic. `<b>` uses the stronger text color, because egui's default fonts have no bold weight.
  - `<size>`, `<material>` and other TextMeshPro tags are removed without changing the look.
- **Look-alikes stay text:** only real rich-text tags count. `List<int>`, `<null>`, `<>c__DisplayClass`, a `</color>` with nothing to close, and `<color=notacolor>` stay as written. Unclosed tags style the rest of the message, as in Unity.
- **The model keeps the message as written.** Styling happens when drawing (entry rows and the details panel); "Original log lines" shows the raw text.
- **Contrast:** Unity's console is dark. On the light theme, colors with too little contrast (WCAG ratio below 3) are mixed toward the text color, so `cyan` becomes teal and `yellow` dark olive.
- **Tags never get in the way:** search, fingerprints (so a colored and an uncolored message group together), headlines, reports and comparisons use the tag-free text.
- **Setting:** "Show Unity rich text in color" (on by default) can be turned off to show the tags as written.
- **Tests:** `tests/fixtures/player_rich_text.log` covers the forms above.

---

## 5. Insights panel

Built as `comte-core::insights::Insights::of(&doc)`, computed once per load or live update and shown as the **Overview** in the right panel whenever nothing is selected (a "← Overview" button and Esc go back to it).

**What's implemented:**
- **Verdict:**
  - *Crashed in <section around the crash>*: names the last error before the crash and the first game-code frame of the native stack.
  - *⏳ Stopped while importing X / during a domain reload / while compiling scripts* (from `ParseEvent::Unfinished`): where it started, what to try, other work still running, and the last error before it. This ranks above errors, because "why is it stuck?" is the question. For a log that is still written (not a session or a `-prev` log), the overview adds when the file was last written: "Last written 15 min ago: if Unity is still running this session, it is stuck here" versus "written moments ago: Unity may still be working on it".
  - *N errors (M different)*: mentions a failed script compilation and its compiler error count. Then either "Most frequent: X ×n at File.cs:42" or, when nothing repeats, "First error: X", because the first error is usually the root cause.
  - *No errors, N warnings*.
  - *Clean*.
- **Most repeated problems:** warnings or worse, seen at least twice, top 5.
- **Most repeated messages:** your own logs, excluding engine chatter and the app's summaries (those are operations, covered by timings).
- **Slowest operations:** the top 8 timed operations as bars.
- **Timings table:** count, total, average and slowest per kind.
- **Slowest imports:** the top 10 asset imports as bars, with the number of imports and their total time. Hover for the full path, click to go to the import.
- **Build size:** from the latest build report, the complete build size, your assets' size, categories and the 8 largest assets as bars.
- **Huge-log notice** (when only the end of the file was read): the file size, how much is shown, and `Insights::loudest`, the most repeated message of any kind, engine chatter included, which is usually what filled the file.
- Reports get **Slowest imports** (top 5) and **Build size** (top categories and assets) sections.
- **Environment** facts, and the **Timeline** as clickable section links.
- Every item jumps to its entry: sections around it open, and if the current filters hide it, the filters are reset so it can be shown.
- On the real Editor log this surfaced a 23-second asset refresh (11 refreshes, 29 s in total).

Original plan:
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
# > Crashes when Steam is not running. Repro: start the game from
# > the Epic launcher with Steam closed.
# --- pins -----------------------------------------------------
# @ 412  last thing before the crash
# @ 388
# ==============================================================
<log content: the original lines, unchanged apart from redaction>
```

- **Content**: the full log, or only the entries you can currently see (written as their original lines, so the file is still a valid log).
- Comte Logger reads the header back when you reopen the file, restoring the note, tags and kind (no detection needed). Line numbers in a reopened session count from the first log line after the header.
- Note lines are written as `# > text`, so a note can never end the header early. The header also records `source_path`, which is redacted too.
- **Pins** are written as `# @ <line>  <note>`, with line numbers counted in the saved body. In a filtered save they are renumbered to the saved lines, and pins on left-out lines are dropped. Pin notes are redacted like everything else. Reopening the file restores the pins and their notes.
- Default folder: `Documents\ComteLogger\sessions\<yyyy-MM-dd_HH-mm>_<title>.log`. `Documents` comes from the Windows known-folder API, so OneDrive redirection works. A different folder can be chosen in Settings (M12).
- **Save dialog** (toolbar or Ctrl+S): note, tags, whole log or only the entries matching the current filters, Redact on or off, an "Also redact" field for your own words, and a live preview ("106 values will be replaced: 89 user names, 5 tokens, …").
- The left panel lists the sessions saved in the default folder, with their tags and the note's first line as a tooltip. A reopened session shows a card with its note, tags, origin and redaction status. Saving it again keeps the note and tags.
- `comte-logger --summary <file>` uses the same loader, so it understands sessions too.

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

More rules were added from real logs:

| What | Example seen in real logs | Replaced with |
|---|---|---|
| Flag whose value is on the next line | `-accessToken` ⏎ `kJw97…`, `-hubSessionId` ⏎ `34e4…` | `<redacted>` |
| User names outside paths | `LicenseClient-cosgu` (the name is learned from paths in the same log) | `LicenseClient-<user>` |
| Player identity fields | `displayName = …`, `"playerName":"…"`, `epicId = …` | `<user>` / `<id>` (`0` values are kept) |
| Machine names | `WindowsEditor(7,DESKTOP-MUOR995)` | `<host>` |
| Crash handler user | `UserName: 'player'` | `<user>` |

Names found in identity fields and `-epicusername=` are also masked wherever they appear as free text. Names *learned from the log* match **case-sensitively**, the way they appeared, so a Windows account named `player` doesn't turn "Unity Player" into "Unity <user>" (found in M11). Names you type in "Also redact", and the current Windows user, match in any case. Loopback IPs (`127.x`) are kept. Version numbers, package ids (`pkg@1.2.3`), asset GUIDs and Mono assembly hashes are left alone; there are tests for these.

The rules live in `comte-core::redact` and are tested against real log lines. An opt-in test (`COMTE_REAL_LOG=<file> cargo test -p comte-app -- --ignored`) checks that a real log has no trace of the user left after redaction. All 28 local logs pass except one: Terraforming Mars logs the Epic display name as unlabeled free text. That case is what "Also redact" is for, because no rule can safely guess an unlabeled name.

### 6.3 Markdown report export
A short report to paste into Jira, Slack or a pull request, built by `comte-core::report::render_report` and opened from **⇪ Report…** in the toolbar. The CLI equivalent is `comte-logger --report <file> [--slack] [--no-redact]`.

- **Content, in order:**
  1. Title and meta line (kind, Unity version, lines, when it was generated).
  2. The verdict as a quote.
  3. Note and tags.
  4. **Pinned entries**: line, severity, the message's first line and the pin's note.
  5. Problem counts.
  6. The **top N problems** (default 5), worst first, then most frequent. Each has where it happened, its section path, its first line, and a code block with the message, its detail lines and a **trimmed stack**: your frames with paths, engine and package runs summarized ("… 3 engine/package frames"), at most about 8 lines.
  7. "…and N more problems".
  8. **Changes since A**, when a comparison is open and included.
  9. Timings table, environment table, timeline.
- **Two styles:**
  - **Markdown** (headings and tables) for Jira, GitHub and Confluence.
  - **Slack**: Slack's mrkdwn has no headings or tables, so headings become bold lines and tables become bullet lines.
- **Safe formatting:** log text always goes into code spans or code blocks (with double backticks when it contains a backtick), and table cells escape `|`.
- **Crashes:** a native crash's detail lines (symbol-loader output with local paths) are left out; only its stack is shown.
- **Headlines:** specific exceptions show their type (`NullReferenceException`). A bare `System.Exception` keeps its message, because the type alone says nothing.
- **Dialog:** style, how many problems, note and tags (prefilled from a reopened session), Redact plus "Also redact" (shared with the save dialog), a live monospace preview, **Copy** (to the clipboard) and **Save .md…** (`Documents\ComteLogger\reports`).
- **Redaction:** the same rules as sessions. User names are learned from the whole log, then the report is masked line by line.
- **Tests:** snapshots of full reports (Player in Markdown and Slack, Editor in Markdown).

---

## 7. Compare two logs

**B** is the open log; **A** ("before") defaults to the previous run (`Editor.log` → `Editor-prev.log`, `Player.log` → `Player-prev.log`), otherwise a file you pick. Implemented as `comte-core::compare::Comparison::of(a, b)`.

- **Matching:** warnings or worse are matched with `fingerprint::across_runs`: same category, same message with numbers and IDs ignored, same file, *any line*. Editing a script between runs moves an error to another line, and it still counts as the same problem. The test case: "Save failed" moving from line 77 to 81 stays "in both".
- **Lists:**
  - **New in B:** worst and most frequent first. Click to show it in B.
  - **Gone since A:** click to open A at that entry.
  - **In both:** counts as ×A → ×B, growing ones first; a count that grew is shown in red.
- **Summary chips:** errors and warnings, A → B, with the difference.
- **Timings:** average per kind, A vs B, with the change in %. Red means a **regression**: more than 25 % *and* more than 100 ms slower. Green means the same amount faster. Smaller moves are treated as noise.
- **Environment changes:** facts whose values differ (Unity version, GPU driver, project, …). The start date is skipped because it always differs.
- **In the app:** the ⇄ Compare button replaces the list with the report. A can be picked from the found logs, saved sessions, or Browse. Swap reverses the direction; Close or Esc goes back. The report recomputes when B changes, including live updates.
- **CLI:** `comte-logger --compare <A> <B>` prints the same report.
- **On this machine:** `Editor-prev.log` → `Editor.log` shows a MissingReferenceException fixed in `SpreadView.cs:75`, a new one in `TurnView.cs:98`, and a script recompile mid-session that made compile, reload and refresh times slower.

---

## 8. Open in IDE

- **How to open a file:** click a location link (the "Where" row, user-frame and package-frame locations), use the "↗ Open TurnView.cs:98 in Rider" button, press **Ctrl+E**, or double-click an entry row.
- **Links only for files that exist:** a location is a link only when its file is found under a project root. Otherwise it is plain text, with a hint to choose the project folder.
- **Project roots, tried in order:**
  1. The folder chosen in ⚙ (remembered).
  2. The Editor log's `Successfully changed project path to:` fact, if that folder exists.

  A Player log, a log from another machine, or a redacted session needs the folder chosen once.
- **Which editor (⚙ › "Open code in"):**
  - **Auto** uses Unity's own External Script Editor, read from `HKCU\Software\Unity Technologies\Unity Editor 5.x\kScriptsDefaultApp_h…` (a NUL-terminated string). On this machine that is Rider. If it isn't set, Auto uses the first installed Rider (standalone, Toolbox or Program Files), then VS Code, then Visual Studio (found with `vswhere`).
  - Rider, VS Code or Visual Studio, chosen explicitly.
  - System default (`explorer <file>`).
  - A custom command template with `{file}`, `{line}` and `{column}` (double quotes group words).
- **Command lines:**

  | Editor | Command |
  |---|---|
  | Rider | `rider64.exe --line N [--column C] file` |
  | VS Code | `Code.exe --goto file:N[:C]` |
  | Visual Studio | `devenv.exe /edit file /command "Edit.GoTo N"` |

  `.cmd`/`.bat` launchers run through `cmd /C` without a console window.
- **Settings file:** `%APPDATA%\ComteLogger\config\settings.toml` (serde + toml). `COMTE_SETTINGS` overrides the path for tests.
- **Verification:** a dry run (`COMTE_IDE_DRY_RUN=1`) on the real Editor log produced `rider64.exe --line 98 …\Spirit Cabinet\Assets\SpiritCabinet\Scripts\UI\TurnView.cs`. A real launch through a harmless custom command passed the right `file:line`.
- Release Player frames rarely have a path, so they are not links (the culprit label still shows the frame name).

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
- **Huge logs** (over 512 MB) are read from their last 64 MB, starting at a whole line. The first 256 KB is read too, for engine detection and environment facts.
  - `Loaded::truncated` records the sizes. Line numbers count from the first line read.
  - The status bar shows "📏 last 64 MB of 30 GB", and saving "the whole log" records it as `filtered (last 64 MB of a 30 GB log)`.
  - Live tail follows a huge log the same way.
  - `COMTE_HUGE_KB=64` lowers the limit for automated checks.
- **Dev hooks** (`COMTE_SCREENSHOT=out.png`, `COMTE_SELECT=problem`, `COMTE_SEARCH=…`, `COMTE_NOISE=1`) let the UI screenshot itself in a given state and quit, for automated visual checks.
- The list is virtualized (only visible rows are drawn), so it stays fast on very large logs.
- Keyboard. Single letters are used for list actions, so they're easy to remember, and they only act while no text box has focus:
  - `N` / `Shift+N` next or previous problem; this opens collapsed sections. `F8` / `Shift+F8` do the same for people used to it from code editors.
  - `]` / `[` next or previous top-level section (Startup, Script compilation, Play mode #n, …), counted from the selection.
  - `E` / `W` / `I` toggle the **E**rrors / **W**arnings / **I**nfo buttons.
  - `P` pins the selected entry or unpins it; `Alt+↓` / `Alt+↑` go to the next or previous pin, wrapping around.
  - `↑` / `↓` move the selection, `Home` / `End` jump to the first or last entry, `Esc` clears it.
  - `Ctrl+Shift+C` copies the selected entry's original lines. Plain `Ctrl+C` is left for copying text you've selected in the details.
  - `Ctrl+F` search; `Enter` / `Shift+Enter` or `F3` / `Shift+F3` for the next or previous match; `Esc` in the search box clears it.
  - `F5` reloads, `Ctrl+O` opens a file, `Ctrl+S` saves the session (M6).
  - egui ignores extra Shift and Alt when matching a shortcut, so the more specific one (`Shift+N`, `Alt+↓`) is always checked first.
- **Copying:**
  - The details panel has **📋 Copy lines** (the original lines, exactly as in the log), **Copy message** (without rich-text tags) and **Copy reference** (`Editor.log:120 [Error] first line of the message`, to point a teammate at it).
  - Right-clicking a row offers the same, plus "Open in editor".
  - The overview's **📋 Copy summary** copies the log's title, verdict, problem counts, environment facts and line count as plain text for a bug report, redacted like reports.
  - The status bar confirms what was copied. `COMTE_COPY=summary|lines|message` triggers a copy for automated checks.
- **Pins** (`comte-app::pins`): entries you want to come back to, each with a note ("the save breaks here"). They are part of the "save logs with notes" idea, not just markers.
  - Pin with `P`, the **📌 Pin** button in the details or the row's right-click menu. Pinned rows show 📌 before the message.
  - A pinned entry's details have a note field. The note is saved with the session and shown in reports.
  - Listed under **PINS** in the left panel, colored by severity. A pin's note is shown instead of the message when it has one. Click to go there, right-click to unpin.
  - Stored as the entry's first line number per file, not its id, so they survive a reload of the same file and live-tail updates (lines are only appended). When live tail sees the file replaced (Unity restarted), that file's pins are cleared.
  - Saved in the session header (§6.1) and listed in reports as **Pinned entries** (§6.3); reopening a session restores them.
  - `COMTE_PIN=75:a note,98` pins the entries starting on those lines (with optional notes), for automated checks.
- **Timeline strip** (`comte-app::strip`): an 18 px strip to the right of the list that shows the log's shape.
  - **Bands** are the top-level sections, tinted by their worst problem (red for errors, amber for warnings, alternating greys otherwise). Rows outside any section get no band.
  - **Ticks:** errors and warnings on the left half; pins (diamonds) and search matches (yellow, while the search only highlights) on the right half. A frame shows the part of the list on screen.
  - Everything is placed by row, so it lines up with what the list shows, whatever is filtered or collapsed. A collapsed section header gets the tick of the worst problem it hides; section titles that match the search get a match tick.
  - Hover a band for the section's name, duration and counts. Click a band to go to its header, click a tick to select its entry, drag to scroll.
  - The legend under the list (Sections · Problems · Pins · Matches) turns each layer on or off; the choice is saved in settings as `[strip]`.
- **Search** (`comte-app::search`):
  - **Words:** every word must appear somewhere in the entry (message without rich-text tags, details, stack frames, `file:line`), in any field and any order. `"a phrase"` keeps words together and in order, and `-word` / `-"a phrase"` excludes.
  - **Toggles:** **Aa** matches case, **.*** treats the whole text as one regular expression, and **Hide others | Highlight** chooses between showing only matching entries (the default) and showing everything with matches highlighted (and marked on the timeline strip).
  - **Matching:** words compile to escaped, case-insensitive regexes, so matching and highlighting always agree on byte ranges, even for non-ASCII text.
  - **Invalid regex:** an unfinished or invalid regex shows its error and hides nothing, so the list doesn't go blank while you type.
  - **Highlighting:** matches are highlighted in entry rows, section titles, the details message, detail lines and stack frames.
  - **Match count:** shown next to the box ("12 matches", "No matches").
  - **Collapsed sections:** while a search is filtering, they open so their matches show. In highlight-only mode they stay as you set them, and a highlighted section title shows where the matches are.

---

## 10. Tech stack

| Concern | Crate |
|---|---|
| GUI | `eframe` / `egui` 0.37 (default wgpu renderer) |
| Native dialogs, drag and drop | `rfd`, egui file drop |
| Parsing | `regex`, `std::sync::LazyLock` |
| Hashing (fingerprints) | built-in FNV-1a 64 (stable across runs, no dependency) |
| Diff | own fingerprint set comparison |
| File watching | 250 ms metadata polling (no `notify`; see §2) |
| Threads | `std::sync::mpsc` |
| Paths | `directories` (known folders, so OneDrive redirection works) |
| Settings | `serde`, `toml` |
| Time | `chrono` (local timestamps) |
| Registry (Unity's script editor) | `winreg` (Windows only) |
| Screenshots | `png` (dev hooks; also the exe icon at build time) |
| Exe icon and version info | `winresource` + the Windows SDK `rc.exe` (build-time) |
| Errors | `std::io::Error`; no error crates were needed |
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
| M6 | Save session: header, note, tags, redaction, reopen | Round trip keeps the note and tags; redaction tests pass ✅ |
| M7 | Live tail | New Unity output appears in under 1 s; truncation handled ✅ |
| M8 | Insights panel | Verdict, counts, slowest items, top repeated messages ✅ |
| M9 | Open in IDE | Clicking a frame opens Rider or VS Code at the line ✅ |
| M10 | Compare two logs | Editor-prev vs Editor diff view ✅ |
| M11 | Markdown report export | Report pastes cleanly into Jira or Slack ✅ |
| M12 | Polish: settings UI, icon, release build | `cargo build --release` gives a working `.exe` ✅ |

**M12 details:**
- **Two programs:** `comte-logger.exe` is a GUI program (Windows subsystem 2, no console window), and `comte.exe` is the command line (subsystem 3). Both are thin `main`s over the `comte_app` library.
- **Icon:** drawn in code (`src/icon_art.rs`) as an indigo tile with four "log lines" whose dots are gray, gray, amber and red. It is the window icon at 64 px, and `build.rs` embeds it into both exes at 16–256 px along with version info. Without a resource compiler (e.g. the GNU toolchain), the build only warns.
- **Settings window (⚙):**
  - Code editor, Unity project folder and saved-sessions folder.
  - "Always also redact" words, shared by the save and report dialogs.
  - What a newly opened log shows: collapse duplicates, engine noise, follow `Editor.log` live.
  - Light, dark or the Windows setting.

  Everything is stored in `settings.toml` and written only when something changes.
- **README** with screenshots taken from the sanitized fixtures in a neutral folder, using the `COMTE_NO_DISCOVERY` hook, so no personal paths or installed games appear.
- **Release build:** `comte-logger.exe` 16.9 MB (mostly the wgpu renderer), `comte.exe` 2.1 MB.

---

## 12. Conventions

- Rust edition 2024, MSRV 1.88; `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` clean.
- **The assistant never commits.** Each task ends with a suggested [Conventional Commits](https://www.conventionalcommits.org/) message.
- Real logs never go into the repo. Fixtures are trimmed and redacted, and `samples/private/` is git-ignored.

## 13. Known limits
- **Compile metric:** this comes from Tundra's own duration. It can differ from the separate `AssetDatabase: script compilation time` line, which Unity sometimes prints before the compile starts.
- **Multi-line `Debug.Log` messages:** only the last line before the stack trace becomes the message (§4.1).
- **Native crash formats** are tested with reconstructed fixtures; no real crash log was available.
- **macOS and Linux** log locations are wired up but untested.
- **Live tail** copies the document on every update; very large files may need a cheaper snapshot.
- **Unlabeled names in logs** (e.g. `User info retrieved: <name>`) can only be redacted through "Always also redact".

## 14. Open questions (for later)
- macOS and Linux support after v1?
- Should saved sessions have a library view with search across them by tag?
- Unreal or Godot next?
