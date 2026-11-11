# Contributing to Comte Log Analyzer

Thanks for your interest! This covers how to build the project, what every
change has to pass, and the few rules that keep logs and releases safe.

## Getting started

- Rust **1.95+** (edition 2024) on Windows with the MSVC toolchain. The minimum
  comes from egui/eframe 0.37; CI checks the code still builds on exactly 1.95.
- `cargo run --release -p comte-app --bin comte-log-analyzer` builds and starts
  the app. `cargo build` also builds `comte`, the command-line version, which
  is handy for checking a parser change on a log:
  `cargo run -p comte-app --bin comte -- --summary path/to/Player.log`.
- Your settings and timing history live in `%APPDATA%\ComteLogAnalyzer\config`,
  not in the repository.

## Before every commit

All three must pass. CI runs them on every push and pull request:

```bash
cargo fmt --all -- --check
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

A clippy warning is a build failure. Don't disable lints globally; a narrowly
scoped, justified `#[allow]` is the only accepted suppression. `unsafe` code
is forbidden in the whole workspace.

## Never commit real logs

Real Unity logs contain user names, licence tokens, machine names and player
ids. Test logs live in `crates/comte-unity/tests/fixtures/` and are
hand-sanitized (`C:\Users\dev`, `player`, made-up project and studio names).
Put your own samples in `samples/private/`, which is git-ignored. The same
goes for issues and pull requests: paste scrubbed excerpts only. The app's
**Copy lines** copies the lines exactly as they are, so replace names, tokens
and paths before posting; saved sessions and reports are redacted by default.

## Commit messages: Conventional Commits (required)

Commits MUST follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
(`feat:`, `fix:`, `perf:`, `docs:`, `chore:`, `test:`, `ci:`, `refactor:`, …
with an optional scope, e.g. `feat(unity): …`).

This is not just style: `scripts/release.ps1` writes each release's changelog
from the commit messages (`feat` → Added, `fix` → Fixed, `perf` →
Performance, the rest → Other), and a `!` or `BREAKING CHANGE:` is marked as
breaking. See [RELEASING.md](RELEASING.md).

## How the code is laid out

| Crate | What belongs there |
|---|---|
| `crates/comte-core` | Everything engine-independent: the log model, the `LogParser` and `EngineSupport` traits, the document, insights, comparison, redaction, sessions and reports. No egui, no Unity knowledge. |
| `crates/comte-unity` | Unity's log formats: the Editor and Player parsers, stack frames, known-problem rules, log discovery. |
| `crates/comte-app` | The egui desktop app and the `comte` command line. |

Dependencies point one way: `comte-app → comte-unity → comte-core`. A new
engine (Unreal, Godot, …) is a new crate implementing `EngineSupport`, listed
in `crates/comte-app/src/engines.rs`.

[`docs/DESIGN.md`](docs/DESIGN.md) describes each feature and the parsing rules
learned from real logs. Update it in the same pull request when behaviour
changes.

## Tests

- **Parser changes come with a test** that feeds the exact line shapes (from a
  real log, with names scrubbed) and checks the entries, sections or hints
  they produce. Bugs about a log that was read wrongly get a regression test.
- **Snapshot tests** use [insta](https://insta.rs/); review changes with
  `cargo insta review` and commit the updated `.snap` files.
- **Redaction** changes need a test for what is now masked and for what must
  stay (shared folders such as `C:\Users\Public` are not people).
- **UI changes** are checked by running the app. It can screenshot itself in a
  given state; see the `COMTE_*` variables in
  `crates/comte-app/src/devtools.rs`.

## Pull requests

- Keep pull requests small and focused: one logical change each.
- CI (fmt, clippy, tests on Windows, the minimum Rust, CodeQL) must be green.
- Fill in the pull request template; update `README.md` and `docs/DESIGN.md`
  when behaviour changes. The changelog comes from your commit messages.

## Reporting bugs and security problems

Open an issue with the matching template. For security problems, such as a
value the redaction misses, use private reporting instead, as described in
[SECURITY.md](SECURITY.md).

By contributing you agree that your contributions are dual licensed under
MIT or Apache-2.0, as described in the README.
