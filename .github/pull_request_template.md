## What does this change?

Describe the change and why it is needed. Link the related issue if one exists.

## Checklist

- [ ] Commit messages follow Conventional Commits. They become the release
      changelog, see RELEASING.md.
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] Parser or redaction changes come with tests, and any test logs are
      sanitized (no real user names, tokens or machine names)
- [ ] Docs are updated where behaviour changed: README, docs/DESIGN.md
