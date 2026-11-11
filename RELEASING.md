# Releasing Comte Log Analyzer

How versions are numbered and how a release ships. The pipeline has three parts:

| File | Job |
|---|---|
| `release-plz.toml` + `.github/workflows/release-plz.yml` | Compute the next version from commits, maintain `CHANGELOG.md`, keep the **Release PR** open, tag on merge |
| `.github/workflows/release.yml` | On a `v*` tag: check the tag matches `Cargo.toml`, test, build the Windows binaries, zip, attest provenance, publish the GitHub Release |
| `.github/workflows/ci.yml` | fmt + clippy + tests on every push and PR, and a build on the minimum Rust |

## Versioning rules

- **SemVer** `MAJOR.MINOR.PATCH`. The git tag is `v{version}` and MUST equal the
  `version` in `[workspace.package]` of the root `Cargo.toml` (all three crates
  share it). `release.yml` refuses to publish when they differ.
- The bump is decided by the **Conventional Commit types since the last release**
  (highest wins). After 1.0:
  - any commit with `!` or `BREAKING CHANGE:` → **major**. For this app,
    "breaking" means the user's world breaks: saved sessions that no longer
    open, an incompatible settings format, a removed feature.
  - any `feat:` → **minor**
  - only `fix:` / `perf:` / `chore:` / `docs:` / `ci:` / `test:` → **patch**
- Before 1.0, semver treats the minor digit as the breaking slot, so bumps are
  more conservative. The Release PR always shows the computed version: **review
  it there, and edit the PR if a different bump is warranted.**
- `1.0.0` is a deliberate, human decision.
- Never move, reuse or delete a published tag. A bad release is fixed by a new
  patch release.

## Day-to-day flow

1. Work merges into `master` with Conventional Commit messages, as always.
2. release-plz keeps a **Release PR** open ("chore: release vX.Y.Z") with the
   version bump in `Cargo.toml` and the changelog since the last release. Only
   `comte-app` is released; the changelog also lists changes to `comte-core`
   and `comte-unity`.
3. **Ship = merge that PR.** The `v*` tag is created automatically, and
   `release.yml` tests, builds and publishes the GitHub Release with the
   Windows zip attached.

Release notes: if `docs/release-notes/v{version}.md` exists, it is used as the
release text (write one for releases worth explaining). Otherwise GitHub
generates notes from the commits.

## After every release: the antivirus check

Each release is a new, unsigned file, so Windows Defender's machine-learning
checks judge it fresh. Until releases are code-signed, do this after the
release publishes:

1. Download the zip from the Releases page. If the browser or Defender flags
   it, submit it as a **false positive** at
   <https://www.microsoft.com/en-us/wdsi/filesubmission> ("Incorrectly detected
   as malware", product: Microsoft Defender Antivirus, include the repo link).
2. Optionally spot-check locally first:
   `& "$env:ProgramFiles\Windows Defender\MpCmdRun.exe" -Scan -ScanType 3 -File <zip> -DisableRemediation`

The release profile keeps the symbol table (`strip = "debuginfo"`, thin LTO)
for the same reason: fully stripped binaries look "packed" to those heuristics.

## One-time setup: the `RELEASE_PLZ_TOKEN` secret

GitHub does not let a workflow's default `GITHUB_TOKEN` trigger other
workflows, so a tag created with it would never fire `release.yml`. release-plz
therefore needs a personal access token:

1. GitHub → Settings → Developer settings → **Fine-grained personal access
   tokens** → Generate new token.
2. Scope it to this repository with permissions **Contents: Read and write**
   and **Pull requests: Read and write**.
3. Repository → Settings → Secrets and variables → Actions → **New repository
   secret**, name it `RELEASE_PLZ_TOKEN`, paste the token.

## Manual fallback (and the first release)

The automated path is optional; a hand-cut release always works (v0.1.2, the
first published release, was cut this way):

1. Bump `version` in `[workspace.package]` of the root `Cargo.toml`, run
   `cargo update -w` so `Cargo.lock` follows, and update `CHANGELOG.md`.
   Commit as `chore(release): vX.Y.Z`.
2. Tag **that** commit and push the tag:

```bash
git tag v0.1.2
```

```bash
git push origin v0.1.2
```

`release.yml` takes it from there. Tagging before the bump is committed (or
tagging an older commit) is refused: "Tag vX.Y.Z does not match Cargo.toml
version". The `v0.1.0` and `v0.1.1` tags are examples; they stay without
releases, since published tags are never moved or deleted.
