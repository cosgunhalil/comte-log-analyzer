//! Support packs (DESIGN §6.4): one zip with everything a developer asks a
//! player or tester for. That is the log, the previous run's log, the latest
//! crash report, related logs, and a `summary.txt` with the verdict,
//! environment, mods and known problems. Text is redacted by default; crash
//! dumps (`.dmp`, process memory) can't be, so they are left out unless asked.

use std::path::{Path, PathBuf};

use comte_core::{FactKey, RedactionReport, Redactor, format_bytes};

use crate::loader::Loaded;
use crate::report_export::{mod_names, summary_lines};
use crate::zip::ZipWriter;

/// Text files bigger than this are left out (a runaway log would make the
/// pack useless to send).
const MAX_TEXT_FILE: u64 = 256 * 1024 * 1024;
const TEXT_EXTENSIONS: &[&str] = &["log", "txt", "json", "xml", "csv"];

/// What goes in the pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackOptions {
    pub redact: bool,
    pub previous_run: bool,
    pub crash_report: bool,
    /// Crash dumps hold process memory and can't be redacted.
    pub crash_dump: bool,
    pub related_logs: bool,
    /// Written at the top of `summary.txt`.
    pub note: String,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            redact: true,
            previous_run: true,
            crash_report: true,
            crash_dump: false,
            related_logs: true,
            note: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// The open log itself (always included).
    Log,
    PreviousRun,
    CrashReport,
    CrashDump,
    Related,
}

/// One file that can go in the pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackItem {
    /// Path inside the zip.
    pub name: String,
    /// `None` = the open log's lines (already in memory).
    pub source: Option<PathBuf>,
    pub bytes: u64,
    pub group: Group,
}

impl PackItem {
    pub fn included(&self, options: &PackOptions) -> bool {
        match self.group {
            Group::Log => true,
            Group::PreviousRun => options.previous_run,
            Group::CrashReport => options.crash_report,
            Group::CrashDump => options.crash_report && options.crash_dump,
            Group::Related => options.related_logs,
        }
    }
}

fn is_text(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| TEXT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// `Player.log` → `Player-prev.log` next to it (not for a `-prev` log).
fn previous_run(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    if stem.ends_with("-prev") {
        return None;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("log");
    Some(path.with_file_name(format!("{stem}-prev.{ext}"))).filter(|p| p.is_file())
}

/// The newest crash report: the newest sub-folder of the crash folder, or the
/// folder itself when it holds the files.
fn latest_crash(folder: &Path) -> Option<PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .collect();
    let newest_dir = entries
        .iter()
        .filter(|e| e.path().is_dir())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    match newest_dir {
        Some(dir) => Some(dir.path()),
        None => entries
            .iter()
            .any(|e| e.path().is_file())
            .then(|| folder.to_path_buf()),
    }
}

/// The files the pack can hold, for the dialog (sizes included).
pub fn plan(loaded: &Loaded) -> Vec<PackItem> {
    let log_name = loaded.path.file_name().map_or_else(
        || "log.txt".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut items = vec![PackItem {
        name: log_name,
        source: None,
        bytes: loaded.raw.iter().map(|l| l.len() as u64 + 1).sum(),
        group: Group::Log,
    }];
    if loaded.session.is_none()
        && let Some(prev) = previous_run(&loaded.path)
    {
        items.push(PackItem {
            name: prev
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            bytes: size(&prev),
            source: Some(prev),
            group: Group::PreviousRun,
        });
    }
    let crash_folder = loaded
        .doc
        .fact(&FactKey::CrashReport)
        .map(|f| PathBuf::from(f.replace('/', std::path::MAIN_SEPARATOR_STR)));
    if let Some(report) = crash_folder.as_deref().and_then(latest_crash) {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&report)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for file in files {
            let name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let group = if is_text(&file) {
                Group::CrashReport
            } else if name.to_ascii_lowercase().ends_with(".dmp") {
                Group::CrashDump
            } else {
                continue;
            };
            items.push(PackItem {
                name: format!("crash/{name}"),
                bytes: size(&file),
                source: Some(file),
                group,
            });
        }
    }
    for companion in loaded
        .companions
        .iter()
        .filter(|c| !c.is_folder && is_text(&c.path))
    {
        items.push(PackItem {
            name: format!(
                "related/{}",
                companion
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ),
            bytes: size(&companion.path),
            source: Some(companion.path.clone()),
            group: Group::Related,
        });
    }
    items.retain(|i| {
        i.group == Group::Log || i.group == Group::CrashDump || i.bytes <= MAX_TEXT_FILE
    });
    items
}

/// A finished pack.
#[derive(Debug)]
pub struct Pack {
    pub zip: Vec<u8>,
    /// Files written (summary included).
    pub files: Vec<String>,
    pub redaction: Option<RedactionReport>,
}

/// Builds the zip in memory.
pub fn build(
    loaded: &Loaded,
    items: &[PackItem],
    options: &PackOptions,
    user_names: &[&str],
    now: chrono::NaiveDateTime,
) -> std::io::Result<Pack> {
    let chosen: Vec<&PackItem> = items.iter().filter(|i| i.included(options)).collect();
    // Text of every chosen text file, so names are learned from all of them.
    let mut texts: Vec<(String, String)> = Vec::new();
    let mut binaries: Vec<(String, PathBuf)> = Vec::new();
    for item in &chosen {
        match &item.source {
            None => texts.push((item.name.clone(), loaded.raw.join("\n") + "\n")),
            Some(path) if item.group == Group::CrashDump => {
                binaries.push((item.name.clone(), path.clone()));
            }
            Some(path) => {
                if let Ok(bytes) = std::fs::read(path) {
                    texts.push((
                        item.name.clone(),
                        String::from_utf8_lossy(&bytes).into_owned(),
                    ));
                }
            }
        }
    }

    let mut summary = vec![
        "Comte Log Analyzer support pack".to_owned(),
        format!("Made {}", now.format("%Y-%m-%d %H:%M")),
        String::new(),
    ];
    if !options.note.trim().is_empty() {
        summary.push("Note:".to_owned());
        summary.extend(options.note.trim().lines().map(|l| format!("  {l}")));
        summary.push(String::new());
    }
    // The summary lists the mods on one line; give each its own here.
    let mods = mod_names(&loaded.doc);
    summary.extend(
        summary_lines(loaded)
            .into_iter()
            .filter(|l| mods.is_empty() || !l.starts_with("Mods (")),
    );
    if !mods.is_empty() {
        summary.push(String::new());
        summary.push(format!("Mods loaded ({}):", mods.len()));
        summary.extend(mods.iter().map(|m| format!("- {m}")));
    }
    if !loaded.doc.hints().is_empty() {
        summary.push(String::new());
        summary.push("What this might be:".to_owned());
        for hint in loaded.doc.hints() {
            summary.push(format!("- {} (line {})", hint.title, hint.line));
        }
    }
    summary.push(String::new());
    summary.push("Files:".to_owned());
    for item in &chosen {
        summary.push(format!("- {} ({})", item.name, format_bytes(item.bytes)));
    }
    if chosen.iter().any(|i| i.group == Group::CrashDump) {
        summary.push("  The .dmp file is process memory and was not redacted.".to_owned());
    }

    let mut zip = ZipWriter::new(Vec::new(), now);
    let mut files = Vec::new();
    let redaction = if options.redact {
        let all_lines: Vec<&str> = texts.iter().flat_map(|(_, t)| t.lines()).collect();
        let mut redactor = Redactor::for_lines(&all_lines, user_names);
        let mut redact = |text: &str| {
            let mut out: String = text
                .lines()
                .map(|l| redactor.redact_line(l).into_owned())
                .collect::<Vec<_>>()
                .join("\n");
            if text.ends_with('\n') {
                out.push('\n');
            }
            out
        };
        let texts: Vec<(String, String)> = texts
            .iter()
            .map(|(name, text)| (name.clone(), redact(text)))
            .collect();
        let summary_text = redact(&(summary.join("\n") + "\n"));
        let report = redactor.report().clone();
        let mut summary_text = summary_text;
        summary_text.push_str(&format!(
            "\nRedacted: {}\n",
            if report.total == 0 {
                "nothing sensitive found".to_owned()
            } else {
                format!("{} values ({})", report.total, report.describe())
            }
        ));
        zip.add("summary.txt", summary_text.as_bytes())?;
        files.push("summary.txt".to_owned());
        for (name, text) in &texts {
            zip.add(name, text.as_bytes())?;
            files.push(name.clone());
        }
        Some(report)
    } else {
        let mut summary_text = summary.join("\n");
        summary_text.push_str("\n\nNot redacted.\n");
        zip.add("summary.txt", summary_text.as_bytes())?;
        files.push("summary.txt".to_owned());
        for (name, text) in &texts {
            zip.add(name, text.as_bytes())?;
            files.push(name.clone());
        }
        None
    };
    for (name, path) in binaries {
        zip.add(&name, &std::fs::read(path)?)?;
        files.push(name);
    }
    Ok(Pack {
        zip: zip.finish()?,
        files,
        redaction,
    })
}

/// `2026-11-10_14-05_Player_support.zip`
pub fn pack_file_name(title: &str, now: &chrono::DateTime<chrono::Local>) -> String {
    crate::save::default_file_name(title, now).replace(".log", "_support.zip")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::load;
    use crate::zip::read_back;

    #[test]
    fn pack_holds_logs_crash_report_and_a_redacted_summary() {
        let base = std::env::temp_dir().join(format!("comte-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let crashes = base.join("Crashes");
        let report = crashes.join("Crash_2026-11-10_120000");
        std::fs::create_dir_all(&report).unwrap();
        std::fs::write(
            report.join("error.log"),
            "Access violation in C:/Users/player/game.exe\n",
        )
        .unwrap();
        std::fs::write(report.join("crash.dmp"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(report.join("screenshot.png"), [0u8]).unwrap();

        let fixture = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../comte-unity/tests/fixtures/player_session.log"),
        )
        .unwrap();
        // Point the crash folder at our temp one.
        let folder = crashes.to_string_lossy().replace('\\', "/");
        let text = fixture.replace(
            "C:/Users/player/AppData/Local/Temp/Sample Studio/Sample Game/Crashes",
            &folder,
        );
        let log = base.join("Player.log");
        std::fs::write(&log, &text).unwrap();
        std::fs::write(
            base.join("Player-prev.log"),
            "Mono path[0] = 'C:/Users/player/Game_Data/Managed'\n",
        )
        .unwrap();

        let loaded = load(&log, "Player.log").unwrap();
        let items = plan(&loaded);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Player.log",
                "Player-prev.log",
                "crash/crash.dmp",
                "crash/error.log"
            ]
        );

        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 10)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let options = PackOptions {
            note: "Crashes when I open the map".to_owned(),
            ..PackOptions::default()
        };
        let pack = build(&loaded, &items, &options, &[], now).unwrap();
        assert_eq!(
            pack.files,
            [
                "summary.txt",
                "Player.log",
                "Player-prev.log",
                "crash/error.log"
            ],
            "no dump unless asked"
        );
        let files = read_back(&pack.zip);
        let summary = String::from_utf8(files[0].1.clone()).unwrap();
        assert!(summary.contains("Note:\n  Crashes when I open the map"));
        assert!(summary.contains("Verdict: Crashed in Scene #3"));
        assert!(summary.contains("What this might be:\n- Crashed in your game code"));
        assert!(summary.contains("\nRedacted: "));
        let error_log = String::from_utf8(files[3].1.clone()).unwrap();
        assert!(
            error_log.contains("C:/Users/<user>/game.exe"),
            "{error_log}"
        );
        let prev = String::from_utf8(files[2].1.clone()).unwrap();
        assert!(
            !prev.contains("Users/player"),
            "names learned from all files: {prev}"
        );

        let with_dump = build(
            &loaded,
            &items,
            &PackOptions {
                crash_dump: true,
                ..PackOptions::default()
            },
            &[],
            now,
        )
        .unwrap();
        assert!(with_dump.files.contains(&"crash/crash.dmp".to_owned()));
        let _ = std::fs::remove_dir_all(&base);
    }
}
