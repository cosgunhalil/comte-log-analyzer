# Know the log, predict the bug: introducing Comte Log Analyzer

Auguste Comte's motto was *« Savoir pour prévoir »*: to know in order to
foresee. It is also the whole idea behind a small tool I built for anyone who has
ever opened a Unity log and closed it again.

## The problem

Every Unity developer knows the moment. A tester writes "the game crashed",
attaches `Player.log`, and you open a few thousand lines of licensing chatter,
package manager output, Mono internals and memory statistics. Somewhere in
there is the line that matters. The Editor is no better: when it hangs on
"Importing…", or a domain reload suddenly takes eight seconds, the answer is in
`Editor.log`, buried under everything else.

Forums are full of the same questions. Which asset is the import stuck on? Is
this crash my code or the graphics driver? Why do my reloads keep getting
slower? Each answer starts the same way: someone reads the log by hand.

## What Comte Log Analyzer does

Comte Log Analyzer is a free, open source Windows app that reads Unity's
`Editor.log` and `Player.log` the way an experienced developer would.

**It finds your logs on its own.** The current and previous Editor logs, every
game's Player log, and the related files that hold the real detail, such as
Package Manager's `upm.log` and the import workers' logs.

**It turns noise into a timeline.** The log is grouped into Startup, script
compiles, domain reloads, Play mode sessions and scenes. Engine chatter is
hidden, a message repeated 95 times becomes one row marked ×95, and your own
stack frames stand out from the engine's.

**It gives you a verdict first.** Before you scroll anywhere, it tells you what
happened: "Crashed in Scene #3", "103 errors, 4 different ones", or, when the
Editor was stuck, "Stopped while importing Assets/Hero.fbx", together with how
long the log has been quiet.

**It explains known problems.** A native crash is traced to where it happened:
a GPU driver, your own IL2CPP code, the engine or a plugin. An out of memory
crash tells a real shortage from an impossible request, which usually means a
bug. A removed graphics device, a full disk, or a Package Manager, Gradle or
licensing failure each comes with an explanation in plain language and a next step.

**It watches performance over time.** It keeps your domain reload, compile and
asset refresh times per project and warns you when they creep up: "1.82 s now,
43% slower than your last 4 sessions." It shows where reload time goes, marking
the steps that run your own `[InitializeOnLoad]` code, and lists your slowest
imports and largest build assets.

**It handles the logs other tools can't.** A runaway log of many gigabytes, too big
for a text editor, opens from its end, with the message that filled the file
named. Live mode
follows the Editor as it writes.

**It makes sharing safe.** Save a session with a note, tags and pinned entries;
export a Markdown or Slack report; compare a run with the previous one; or zip
a support pack with the logs, the crash report and a summary. User names,
tokens, licence ids, emails and machine names are redacted by default, so what
you send doesn't leak what you didn't mean to.

## Who it's for

Unity developers chasing a crash, QA testers writing a bug report that someone
can actually act on, and anyone supporting players who send in a `Player.log`.
It has been tested with logs from 25 games built with Unity 2019.4 through
Unity 6, and with the Unity 6 Editor.

## Try it

Comte Log Analyzer is a single Windows executable: download it from the
[releases page][releases]
and run it. With no file given, it opens your current Editor log. It is written
in Rust, licensed under MIT or the Apache License 2.0, and developed in the open
[on GitHub][repo].

Bug reports are welcome, and so are logs it reads wrongly. Scrub them of names
first, because a log knows more about you than you'd think.

[releases]: https://github.com/cosgunhalil/comte-log-analyzer/releases/latest
[repo]: https://github.com/cosgunhalil/comte-log-analyzer
