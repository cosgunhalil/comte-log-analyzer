<img src="docs/images/icon.png" width="72" align="right" alt="">

# Comte Logger

*« Savoir pour prévoir »* (Auguste Comte): know the log so you can predict the bug.

Comte Logger is a Windows desktop app that turns Unity's noisy `Editor.log` and
`Player.log` into something a person can read. It shows a timeline of what
happened, which problems occurred and how often, and where in your code they
started. You can save a log with a note to share it, compare two runs, and
paste a short report into Jira or Slack.

![Overview of a Player log that crashed](docs/images/overview.png)

## What it does

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
- **Says where Unity got stuck.** When the log ends in the middle of
  something, the verdict says what: "⏳ Stopped while importing
  Assets/Hero.fbx", a domain reload or a script compile. It also says how long
  the log has been quiet, so you can tell stuck from busy.
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

![Details of an exception with links into the project](docs/images/details.png)

![Comparing the previous and the current Editor run](docs/images/compare.png)

## Getting started

You need [Rust](https://rustup.rs/) 1.88 or newer on Windows (MSVC toolchain).

```bash
cargo run --release -p comte-app --bin comte-logger
```

Or build once and use the executables in `target/release`:

```bash
cargo build --release
```

| Program | What it is |
|---|---|
| `comte-logger.exe [log file]` | The desktop app. With no file, it opens your Unity `Editor.log`. |
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

Settings are stored in `%APPDATA%\ComteLogger\config\settings.toml`.

## Project layout

| Crate | Contents |
|---|---|
| `crates/comte-core` | Engine-independent parts: the log model, parser trait, document, insights, comparison, redaction, sessions, reports, live-tail parsing. |
| `crates/comte-unity` | Unity support: Editor and Player parsers, stack frame cleanup, log discovery. Sanitized test logs live in `tests/fixtures`. |
| `crates/comte-app` | The desktop app (egui) and the `comte` command line. |

[`docs/DESIGN.md`](docs/DESIGN.md) describes the design, the parsing rules
learned from real logs, and the milestones.

## Development

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

- **Snapshot tests** use [insta](https://insta.rs/); review changes with `cargo insta review`.
- **Visual checks:** the app can screenshot itself in a given state. See `crates/comte-app/src/devtools.rs` for the `COMTE_*` variables. For example:

  ```bash
  COMTE_SELECT=problem COMTE_SCREENSHOT=shot.png cargo run -p comte-app --bin comte-logger -- crates/comte-unity/tests/fixtures/player_session.log
  ```

- **Redaction check:** `COMTE_REAL_LOG=<a real log> cargo test -p comte-app -- --ignored` checks that no trace of your user name is left after redaction.

Never commit real logs: they contain user names, tokens and machine ids.
`samples/private/` is git-ignored for local samples.
