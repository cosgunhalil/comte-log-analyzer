# Releasing Comte Log Analyzer

A release is a version tag. You create it with one script and push it; CI
builds the Windows exe and publishes it. No bots, tokens or secrets are
involved.

| File | Job |
|---|---|
| `scripts/release.ps1` | On your machine: pick the next version, write its `CHANGELOG.md` section from the commits, bump `Cargo.toml` and `Cargo.lock`, commit, and tag |
| `.github/workflows/release.yml` | On a pushed `v*` tag: check the tag matches `Cargo.toml`, test, build, attest, and publish the GitHub Release |
| `.github/workflows/ci.yml` | fmt + clippy + tests on every push and pull request, and a build on the minimum Rust |

## Cutting a release

From an up-to-date `master` with a clean working tree:

```bash
./scripts/release.ps1 -DryRun
```

This shows the version and the changelog section without changing anything.
When it looks right:

```bash
./scripts/release.ps1
```

The script commits `chore(release): vX.Y.Z` and tags it, locally. Publish by
pushing both:

```bash
git push origin master
```

```bash
git push origin vX.Y.Z
```

(`./scripts/release.ps1 -Push` does the pushes for you.) The release appears
on the Releases page a few minutes later, once `release.yml` has finished.

## Versioning

- **SemVer** `MAJOR.MINOR.PATCH`. The tag is `v{version}`, and it must equal the
  `version` in `[workspace.package]` of the root `Cargo.toml` (all three crates
  share it). `release.yml` refuses to publish when they differ, which is why
  the script bumps and tags the same commit.
- **By default every release is the next patch** (`v0.1.2` → `v0.1.3`). For a
  bigger step, say so: `./scripts/release.ps1 -Version 0.2.0`.
- Choose a bigger step when the commits call for it: a `feat:` is worth a
  minor version once there are users relying on the current one, and anything
  marked `!` or `BREAKING CHANGE:` (sessions that no longer open, an
  incompatible settings format, a removed feature) is worth a major version
  after 1.0. `1.0.0` is a deliberate, human decision.
- Never move, reuse or delete a published tag; the script refuses a tag that
  exists. A bad release is fixed by a new patch release. (`v0.1.0` and
  `v0.1.1` were cut while the pipeline was being set up and have no builds.)

## What a release contains

- `comte-log-analyzer-vX.Y.Z-windows-x64.exe`: the app, ready to run.
- `comte-log-analyzer-vX.Y.Z-windows-x64.zip`: the app, the `comte` command
  line, the README and both licences.
- Release notes: a short download note, then the version's section of
  `CHANGELOG.md`, which the script generated from the Conventional Commits
  since the last tag (`feat` → Added, `fix` → Fixed, `perf` → Performance, the
  rest → Other). Edit that section before pushing if it needs a human touch.
  For a release worth a longer story, write `docs/release-notes/vX.Y.Z.md`
  and it is used instead.
- Signed build provenance for both files:
  `gh attestation verify <file> --repo cosgunhalil/comte-log-analyzer`.

## After every release: the antivirus check

Each release is a new, unsigned file, so Windows Defender's machine-learning
checks judge it fresh. Until releases are code-signed, do this after the
release publishes:

1. Download the exe and the zip from the Releases page. If the browser or
   Defender flags one, submit it as a **false positive** at
   <https://www.microsoft.com/en-us/wdsi/filesubmission> ("Incorrectly
   detected as malware", product: Microsoft Defender Antivirus, include the
   repository link).
2. Optionally spot-check locally first:
   `& "$env:ProgramFiles\Windows Defender\MpCmdRun.exe" -Scan -ScanType 3 -File <file> -DisableRemediation`

The release profile keeps the symbol table (`strip = "debuginfo"`, thin LTO)
for the same reason: fully stripped binaries look "packed" to those heuristics.

## Without the script

If you can't run PowerShell, do the same steps by hand: set `version` in
`[workspace.package]`, run `cargo update --workspace`, add a `CHANGELOG.md`
section under `## [Unreleased]`, commit as `chore(release): vX.Y.Z`, then tag
**that** commit and push it and the tag. Tagging before the bump is committed
is refused by `release.yml` ("Tag vX.Y.Z does not match Cargo.toml version").
