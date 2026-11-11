# Security Policy

Comte Log Analyzer is a local desktop application. It makes no network
connections: it reads log files on your own machine and writes sessions,
reports and support packs only where you choose. Its attack surface is small,
but we still take any report seriously.

## What counts as a security problem

- **Redaction that misses something.** Saved sessions, reports, copied
  summaries and support packs are redacted by default so they can be shared.
  A user name, token, licence id, e-mail, IP or machine name that survives
  redaction is a security problem; please report it privately, with a
  minimal example line (made up or scrubbed) rather than a real log.
- A crafted log file that makes the app crash, hang or write outside the
  place you chose.
- Anything that makes the app run a program or open a file you did not ask
  for (the "open in editor" and "open folder" actions start other programs).

## Supported versions

Only the latest release receives security fixes. Please update to the newest
version from the [Releases](https://github.com/cosgunhalil/comte-log-analyzer/releases)
page before reporting.

## Reporting a vulnerability

Please do not open a public issue for security problems.

1. Preferred: use GitHub's private vulnerability reporting. Open the
   [Security tab](https://github.com/cosgunhalil/comte-log-analyzer/security)
   of this repository and choose "Report a vulnerability". The report stays
   private between you and the maintainer until a fix is released.
2. Fallback: e-mail cosgun.halil@gmail.com with the details.

Include what you found, steps to reproduce it, and the version you tested.
Never attach real logs: they contain user names, tokens and machine ids.

## What to expect

You will get a first response within a few days. If the report is confirmed,
a fix ships in the next release and you will be credited in the release notes
unless you prefer otherwise.
