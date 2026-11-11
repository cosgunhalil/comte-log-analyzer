# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2](https://github.com/cosgunhalil/comte-log-analyzer/releases/tag/v0.1.2) - 2026-11-11

The first published release. The `v0.1.0` and `v0.1.1` tags were cut while the
release pipeline was being set up and have no builds.

### Added

- *(unity)* Editor.log and Player.log parsers with timeline sections
- *(unity)* discover Editor and Player logs on this machine
- *(app)* desktop log viewer with timeline, filters and details
- save sessions with note, tags and redaction
- insights overview with verdict, repeats and timings
- *(app)* open stack frame locations in the code editor
- compare two logs (new, gone and common problems, timings)
- export a Markdown or Slack report of a log
- *(app)* settings window, app icon and a separate CLI binary
- render Unity rich text (color, bold, italic) in log messages
- *(app)* search with words, phrases, exclusions and highlighting
- *(app)* copy actions and keyboard shortcuts for filters and selection
- pins with notes, a timeline strip and a letter-key scheme
- say where Unity stopped, open huge logs from the end, show slow imports and build size
- explain known problems and triage native crashes
- find companion logs and offer them where they help
- track editor timing trends and show where domain reload time goes
- zip a redacted support pack with logs, crash report and summary
- read timestamped logs and notice two editors writing one log

### Fixed

- *(test)* make fixture tests independent of line endings

### Other

- *(ci)* release pipeline with release-plz, build provenance and a tag check
