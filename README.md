<img src="docs/images/icon.png" width="72" align="right" alt="">

# Comte Log Analyzer

A Windows desktop app that turns Unity's noisy `Editor.log` and `Player.log` into something a person can read.

*« Savoir pour prévoir »* (Auguste Comte): know the log so you can predict the bug.

![Overview of a Player log that crashed](docs/images/overview.png)

## Install and run

- **Windows:** download `comte-log-analyzer-…-windows-x64.exe` from the [latest release](https://github.com/cosgunhalil/comte-log-analyzer/releases/latest) and run it. The `.zip` next to it also has the `comte` command line.
- **From source** (Rust 1.95 or newer, edition 2024, MSVC toolchain): `cargo install --locked --git https://github.com/cosgunhalil/comte-log-analyzer comte-app`

With no file given, the app opens your current Unity `Editor.log`. Drop any other log onto the window, or press Ctrl+O.

<details>
<summary>Windows warns about the download?</summary>

The executables aren't code-signed yet, so SmartScreen may ask you to confirm,
and Defender's machine-learning checks sometimes flag a new, unsigned release
(a false positive; each one is reported to Microsoft). Every release is built
by [GitHub Actions](https://github.com/cosgunhalil/comte-log-analyzer/actions)
straight from the tagged source, and you can check that a zip came from there
with the [GitHub CLI](https://cli.github.com):

```bash
gh attestation verify comte-log-analyzer-v0.1.3-windows-x64.exe --repo cosgunhalil/comte-log-analyzer
```

Or build it yourself from source.
</details>

## Will it read my log?

- **Windows only for now.** macOS and Linux log locations are wired up but untested, so no builds are offered for them yet.
- **Player.log:** tested with logs from 25 games built with Unity 2019.4, 2020.3, 2021.3, 2022.3 and Unity 6 (6000.0, 6000.2 and 6000.3).
- **Editor.log:** tested with Unity 6000.3. Older Editor logs use the same building blocks, but they haven't been checked against real files yet.

## What it does

- **Finds your logs on its own:** `Editor.log`, `Editor-prev.log` and every game's `Player.log` under `AppData\LocalLow`, plus related logs such as `upm.log` and the import workers' logs.
- **Turns them into a readable timeline:** Startup, compiles, domain reloads, Play mode and scenes, with engine chatter hidden and repeats collapsed (×95).
- **Explains what went wrong:** a one-line verdict ("Crashed in Scene #3"), native crashes traced to a GPU driver, your IL2CPP code, the engine or a plugin, and known problems such as out of memory or a removed graphics device. It also says where Unity got stuck: "⏳ Stopped while importing Assets/Hero.fbx".
- **Opens logs too large for an editor:** runaway logs of many gigabytes open from their end, and the message that filled the file is named.
- **Saves, shares and compares:** sessions with notes, tags and pins, redacted by default; a support pack zip for developers; Markdown or Slack reports; and a comparison with the previous run.
- **Takes you to the code:** click `TurnView.cs:98` (or press Ctrl+E) and the file opens at that line in Rider, VS Code or Visual Studio.

![Details of an exception with links into the project](docs/images/details.png)

![Comparing the previous and the current Editor run](docs/images/compare.png)

## All features

- **Finds your logs.** `Editor.log`, `Editor-prev.log`, and every game's
  `Player.log` / `Player-prev.log` under `AppData\LocalLow`. Drop any other
  file onto the window, or open it with Ctrl+O.
- **Reads them like a person would.**
  - The timeline is grouped into Startup, Script compilation, Domain reload,
    Play mode #n, Scene #n, Crash and Shutdown.
  - Engine chatter (licensing, package manager, Mono) is hidden.
  - Repeated messages collapse into one row with a count (×95).
  - Search finds entries containing all your words, anywhere in the message,
    details, stack frames or file paths. `"a phrase"` matches exactly, `-word`
    excludes, and there are case and regex toggles. Matches are highlighted,
    and **Hide others | Highlight** chooses whether non-matches stay in view.
  - A **timeline strip** beside the list shows the log's sections as bands,
    tinted by their worst problem, with ticks for errors, pins and matches.
    Hover a band to see the section, click to go there.
  - Stack traces show your code in bold and fold the engine frames.
  - Unity rich text in your own logs (`<color=cyan>`, `<b>`, `<i>`) is shown
    in color, as in Unity's console.
- **Tells you what matters.** A one-line verdict ("Crashed in Scene #3",
  "103 errors (4 different)"), the most repeated problems, and the slowest
  operations (domain reloads, asset refreshes, compiles).
- **Explains known problems.** "What this might be" turns a line into an
  answer:
  - a native crash says where it happened (a GPU driver, your IL2CPP code,
    Mono, the engine or a plugin) and how many frames have no symbols, and
    links the crash folder
  - out of memory tells a real shortage from an impossible request (a bug)
  - also covered: a removed graphics device (with a note when an integrated
    GPU was used), a full disk, Package Manager, Gradle, IL2CPP and licensing
    failures
- **Reads timestamped logs.** Logs written with `-timestamps` parse like
  any other, and show each line's time (in your time zone) and how long
  after the start it happened. It notices when two Editors wrote into the
  same log.
- **Points to the other logs.** Package Manager's `upm.log`, the import
  workers' logs, Gradle's build folder, the licensing client's log and
  Editor crash reports are found when they exist. They are offered right
  where they help, such as `upm.log` on a package error, or the worker logs
  on a stuck import.
- **Says where Unity got stuck.** When the log ends in the middle of
  something, the verdict says what: "⏳ Stopped while importing
  Assets/Hero.fbx", a domain reload or a script compile. It also says how long
  the log has been quiet, so you can tell stuck from busy.
- **Notices when the editor gets slower.** Domain reload, compile and asset
  refresh times are kept per project, so the overview can say "domain
  reloads: 1.82 s now, 43% slower than your last 4 sessions". It also shows
  where reload time goes, and marks the steps that run your own code
  (`[InitializeOnLoad]`, `OnEnable`).
- **Finds what is slow or big.** The slowest asset imports, and after a
  build, its size by category and its largest assets.
- **Opens runaway logs.** Logs over 512 MB (some reach tens of gigabytes) open
  from their last 64 MB, with the environment read from the start and the
  message that filled the file named.
- **Takes you to the code.** Click `TurnView.cs:98` (or press Ctrl+E) and the
  file opens at that line in Rider, VS Code or Visual Studio. By default it
  uses the same editor Unity is set to.
- **Follows Unity live.** Turn on ● Live and new output appears within a second.
- **Saves and shares.** Ctrl+S saves the log as a plain `.log` file with your
  note and tags. **Sensitive values are redacted by default**: user names,
  launcher tokens, licensing ids, player names and ids, emails, IPs and
  machine names.
- **Packs it up for support.** 📦 Pack zips the log, the previous run, the
  latest crash report, related logs and a summary (verdict, environment,
  mods, known problems). Everything is redacted; the crash dump is left out
  unless you ask, because process memory can't be redacted.
- **Compares runs.** ⇄ Compare shows what's new, what's gone and what got
  slower since the previous run.
- **Writes reports.** ⇪ Report creates a short Markdown or Slack summary to
  copy or save.
- **Copies what you need.** Copy an entry's original lines or its message (in
  the details or by right-clicking a row), or a redacted summary of the log
  and its environment for a bug report.
- **Pins with notes.** Press P to pin an entry (📌) and write why it matters.
  Pins are listed on the left, saved with the session and included in
  reports, so whoever opens the file sees what you saw.

[`docs/DESIGN.md`](docs/DESIGN.md) describes each feature in detail, along
with the parsing rules learned from real logs.

## Building from source

You need [Rust](https://rustup.rs/) 1.95 or newer on Windows (MSVC toolchain).
That minimum comes from the GUI library (egui/eframe 0.37); the code itself
uses edition 2024 and `if let … && …` chains.

```bash
cargo run --release -p comte-app --bin comte-log-analyzer
```

Or build once and use the executables in `target/release`:

```bash
cargo build --release
```

| Program | What it is |
|---|---|
| `comte-log-analyzer.exe [log file]` | The desktop app. With no file, it opens your Unity `Editor.log`. |
| `comte.exe` | The command line, described below. |

### Command line

```bash
comte --list
```

```bash
comte --summary path/to/Player.log
```

```bash
comte --compare Editor-prev.log Editor.log
```

```bash
comte --report path/to/Player.log --slack
```

`--report` redacts by default; add `--no-redact` to keep everything.

### Keyboard

| Keys | Action |
|---|---|
| N / Shift+N (or F8) | Next / previous warning or error |
| ] / [ | Next / previous section (Startup, Play mode #1, …) |
| E / W / I | Show or hide Errors / Warnings / Info |
| P | Pin the selected entry, or unpin it |
| Alt+↓ / Alt+↑ | Next / previous pin |
| Home / End | First / last entry |
| Ctrl+Shift+C | Copy the selected entry's original lines |
| ↑ / ↓, Esc | Move / clear the selection |
| Ctrl+F | Search messages, files and stack frames |
| Enter / Shift+Enter, F3 / Shift+F3 | Next / previous search match |
| Esc (in the search box) | Clear the search |
| Ctrl+E, double-click | Open the selected entry's file in your editor |
| Ctrl+S | Save the log as a session with a note and tags |
| Ctrl+O, F5 | Open a file, reload |

### Settings

⚙ in the toolbar sets:
- the code editor,
- the Unity project folder (needed for Player logs, which don't name it),
- where sessions are saved,
- words to always redact (for example your display name),
- what a newly opened log shows,
- light or dark mode.

Settings are stored in `%APPDATA%\ComteLogAnalyzer\config\settings.toml`.

## Project layout

| Crate | Contents |
|---|---|
| `crates/comte-core` | Engine-independent parts: the log model, parser trait, document, insights, comparison, redaction, sessions, reports, live-tail parsing. |
| `crates/comte-unity` | Unity support: Editor and Player parsers, stack frame cleanup, log discovery. Sanitized test logs live in `tests/fixtures`. |
| `crates/comte-app` | The desktop app (egui) and the `comte` command line. |

## Development

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

CI runs both on every push and pull request. To release, run
`./scripts/release.ps1` and push the tag it creates; CI builds and publishes
the exe. See [RELEASING.md](RELEASING.md) for the details and
[CHANGELOG.md](CHANGELOG.md) for the history.

- **Snapshot tests** use [insta](https://insta.rs/); review changes with `cargo insta review`.
- **Visual checks:** the app can screenshot itself in a given state. See `crates/comte-app/src/devtools.rs` for the `COMTE_*` variables. For example:

  ```bash
  COMTE_SELECT=problem COMTE_SCREENSHOT=shot.png cargo run -p comte-app --bin comte-log-analyzer -- crates/comte-unity/tests/fixtures/player_session.log
  ```

- **Redaction check:** `COMTE_REAL_LOG=<a real log> cargo test -p comte-app -- --ignored` checks that no trace of your user name is left after redaction.

Never commit real logs: they contain user names, tokens and machine ids.
`samples/private/` is git-ignored for local samples.

## Security

Found a security problem, such as a value the redaction misses? Please report
it privately, as described in [SECURITY.md](SECURITY.md), not in a public issue.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
