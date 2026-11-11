# Using Comte Log Analyzer

The full feature list, the keyboard shortcuts, the settings and the command
line. For how each feature works inside, see [DESIGN.md](DESIGN.md).

- [Features](#features)
- [Keyboard](#keyboard)
- [Settings](#settings)
- [Command line](#command-line)

## Features

### Finding and reading logs

- **Finds your logs.** `Editor.log`, `Editor-prev.log`, and every game's
  `Player.log` / `Player-prev.log` under `AppData\LocalLow`. Drop any other
  file onto the window, or open it with Ctrl+O.
- **Reads them like a person would.**
  - The timeline is grouped into Startup, Script compilation, Domain reload,
    Play mode #n, Scene #n, Crash and Shutdown.
  - Engine chatter (licensing, package manager, Mono) is hidden.
  - Repeated messages collapse into one row with a count (×95).
  - Stack traces show your code in bold and fold the engine frames.
  - Unity rich text in your own logs (`<color=cyan>`, `<b>`, `<i>`) is shown
    in color, as in Unity's console.
- **Search** finds entries containing all your words, anywhere in the message,
  details, stack frames or file paths. `"a phrase"` matches exactly, `-word`
  excludes, and there are case and regex toggles. Matches are highlighted,
  and **Hide others | Highlight** chooses whether non-matches stay in view.
- **Timeline strip.** A strip beside the list shows the log's sections as
  bands, tinted by their worst problem, with ticks for errors, pins and
  matches. Hover a band to see the section, click to go there.
- **Reads timestamped logs.** Logs written with `-timestamps` parse like any
  other, and show each line's time (in your time zone) and how long after the
  start it happened. It notices when two Editors wrote into the same log.
- **Opens runaway logs.** Logs over 512 MB (some reach tens of gigabytes) open
  from their last 64 MB, with the environment read from the start and the
  message that filled the file named.
- **Follows Unity live.** Turn on ● Live and new output appears within a second.

### Understanding what went wrong

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
- **Says where Unity got stuck.** When the log ends in the middle of
  something, the verdict says what: "⏳ Stopped while importing
  Assets/Hero.fbx", a domain reload or a script compile. It also says how long
  the log has been quiet, so you can tell stuck from busy.
- **Points to the other logs.** Package Manager's `upm.log`, the import
  workers' logs, Gradle's build folder, the licensing client's log and Editor
  crash reports are found when they exist. They are offered right where they
  help, such as `upm.log` on a package error, or the worker logs on a stuck
  import.
- **Takes you to the code.** Click `TurnView.cs:98` (or press Ctrl+E) and the
  file opens at that line in Rider, VS Code or Visual Studio. By default it
  uses the same editor Unity is set to.

### Performance

- **Notices when the editor gets slower.** Domain reload, compile and asset
  refresh times are kept per project, so the overview can say "domain
  reloads: 1.82 s now, 43% slower than your last 4 sessions". It also shows
  where reload time goes, and marks the steps that run your own code
  (`[InitializeOnLoad]`, `OnEnable`).
- **Finds what is slow or big.** The slowest asset imports, and after a
  build, its size by category and its largest assets.

### Saving and sharing

- **Saves sessions.** Ctrl+S saves the log as a plain `.log` file with your
  note and tags. **Sensitive values are redacted by default**: user names,
  launcher tokens, licensing ids, player names and ids, emails, IPs and
  machine names.
- **Pins with notes.** Press P to pin an entry (📌) and write why it matters.
  Pins are listed on the left, saved with the session and included in
  reports, so whoever opens the file sees what you saw.
- **Packs it up for support.** 📦 Pack zips the log, the previous run, the
  latest crash report, related logs and a summary (verdict, environment,
  mods, known problems). Everything is redacted; the crash dump is left out
  unless you ask, because process memory can't be redacted.
- **Compares runs.** ⇄ Compare shows what's new, what's gone and what got
  slower since the previous run.
- **Writes reports.** ⇪ Report creates a short Markdown or Slack summary to
  copy or save.
- **Copies what you need.** Copy an entry's original lines (exactly as in the
  log, not redacted) or its message, in the details or by right-clicking a
  row, or a redacted summary of the log and its environment for a bug report.

## Keyboard

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

## Settings

⚙ in the toolbar sets:

- the code editor,
- the Unity project folder (needed for Player logs, which don't name it),
- where sessions are saved,
- words to always redact (for example your display name),
- what a newly opened log shows,
- light or dark mode.

Settings are stored in `%APPDATA%\ComteLogAnalyzer\config\settings.toml`.

## Command line

`comte.exe` is a separate command-line program for scripts and terminals.
Run it from a terminal (double-clicking it only flashes a window, since it
prints and exits):

| Command | What it prints |
|---|---|
| `comte --list` | The logs found on this machine |
| `comte --summary path/to/Player.log` | A parse summary: kind, facts, sections, problems |
| `comte --compare Editor-prev.log Editor.log` | What changed from the first log to the second |
| `comte --report path/to/Player.log --slack` | A Markdown (or Slack) report, redacted unless you add `--no-redact` |

The desktop app takes a log file as an argument too:
`comte-log-analyzer.exe path/to/Player.log`. With no file, it opens your
current Unity `Editor.log`.
