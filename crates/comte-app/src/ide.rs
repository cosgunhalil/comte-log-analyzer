//! Opening `file:line` in a code editor (DESIGN §8).
//!
//! "Auto" uses the editor Unity itself is configured with (Preferences ›
//! External Tools › External Script Editor, stored in the registry), then the
//! first installed Rider, VS Code or Visual Studio, then the system default.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// What the user picked in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IdeChoice {
    /// Unity's External Script Editor, else the first installed IDE.
    #[default]
    Auto,
    Rider,
    VsCode,
    VisualStudio,
    /// The program Windows associates with `.cs` files.
    System,
    /// A command template with `{file}`, `{line}` and `{column}`.
    Custom,
}

impl IdeChoice {
    pub const ALL: [IdeChoice; 6] = [
        IdeChoice::Auto,
        IdeChoice::Rider,
        IdeChoice::VsCode,
        IdeChoice::VisualStudio,
        IdeChoice::System,
        IdeChoice::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            IdeChoice::Auto => "Auto (same as Unity)",
            IdeChoice::Rider => "JetBrains Rider",
            IdeChoice::VsCode => "Visual Studio Code",
            IdeChoice::VisualStudio => "Visual Studio",
            IdeChoice::System => "System default",
            IdeChoice::Custom => "Custom command…",
        }
    }
}

/// Families of editors we know how to pass a line number to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdeKind {
    Rider,
    VsCode,
    VisualStudio,
    /// Unknown program: it gets the file only.
    Other,
}

impl IdeKind {
    /// Guesses the family from the executable's file name.
    pub fn of(exe: &Path) -> Self {
        let name = exe
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name.starts_with("rider") {
            IdeKind::Rider
        } else if name.starts_with("code") {
            IdeKind::VsCode
        } else if name.starts_with("devenv") {
            IdeKind::VisualStudio
        } else {
            IdeKind::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            IdeKind::Rider => "Rider",
            IdeKind::VsCode => "VS Code",
            IdeKind::VisualStudio => "Visual Studio",
            IdeKind::Other => "editor",
        }
    }
}

/// An editor found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedIde {
    pub kind: IdeKind,
    pub exe: PathBuf,
    /// Unity's configured External Script Editor.
    pub from_unity: bool,
}

impl DetectedIde {
    pub fn name(&self) -> String {
        match self.kind {
            IdeKind::Other => self
                .exe
                .file_stem()
                .map_or_else(|| "editor".to_owned(), |s| s.to_string_lossy().into_owned()),
            kind => kind.name().to_owned(),
        }
    }
}

/// A program and its arguments, ready to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// How to open `file` at `line`/`column` with `exe`.
pub fn launch_for(exe: &Path, file: &Path, line: Option<u32>, column: Option<u32>) -> Launch {
    let file_arg = file.display().to_string();
    let args = match (IdeKind::of(exe), line) {
        (IdeKind::Rider, Some(line)) => {
            let mut args = vec!["--line".to_owned(), line.to_string()];
            if let Some(column) = column {
                args.extend(["--column".to_owned(), column.to_string()]);
            }
            args.push(file_arg);
            args
        }
        (IdeKind::VsCode, Some(line)) => {
            let target = match column {
                Some(column) => format!("{file_arg}:{line}:{column}"),
                None => format!("{file_arg}:{line}"),
            };
            vec!["--goto".to_owned(), target]
        }
        (IdeKind::VisualStudio, Some(line)) => vec![
            "/edit".to_owned(),
            file_arg,
            "/command".to_owned(),
            format!("Edit.GoTo {line}"),
        ],
        (IdeKind::VisualStudio, None) => vec!["/edit".to_owned(), file_arg],
        _ => vec![file_arg],
    };
    Launch {
        program: exe.to_owned(),
        args,
    }
}

/// Opens `file` with the program the system associates with it.
pub fn system_launch(file: &Path) -> Launch {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Launch {
        program: PathBuf::from(program),
        args: vec![file.display().to_string()],
    }
}

/// Fills a custom command template. Words are split on spaces; double
/// quotes group words (`"C:\Program Files\X\x.exe" --goto {file}:{line}`).
/// `None` if the template is empty.
pub fn custom_launch(
    template: &str,
    file: &Path,
    line: Option<u32>,
    column: Option<u32>,
) -> Option<Launch> {
    let fill = |word: &str| {
        word.replace("{file}", &file.display().to_string())
            .replace("{line}", &line.unwrap_or(1).to_string())
            .replace("{column}", &column.unwrap_or(1).to_string())
    };
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has_word = false;
    for c in template.trim().chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_word {
                    words.push(fill(&current));
                    current.clear();
                    has_word = false;
                }
            }
            c => {
                current.push(c);
                has_word = true;
            }
        }
    }
    if has_word {
        words.push(fill(&current));
    }
    let mut words = words.into_iter();
    let program = PathBuf::from(words.next()?);
    Some(Launch {
        program,
        args: words.collect(),
    })
}

/// Starts the program without waiting for it. `.cmd`/`.bat` scripts (VS Code's
/// `code.cmd`, Toolbox's `rider.cmd`) go through `cmd /C`, without a console window.
pub fn spawn(launch: &Launch) -> std::io::Result<()> {
    let is_script = launch
        .program
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    let mut command = if is_script {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&launch.program);
        c
    } else {
        Command::new(&launch.program)
    };
    command.args(&launch.args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if is_script {
            command.creation_flags(CREATE_NO_WINDOW);
        }
    }
    command.spawn().map(|_| ())
}

/// `C:\Users\x\Project` + `Assets/Scripts/A.cs` → the existing file, trying
/// each root in order. Absolute paths are used as they are.
pub fn resolve_source(path: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let relative = Path::new(path);
    if relative.is_absolute() {
        return relative.is_file().then(|| relative.to_owned());
    }
    roots
        .iter()
        .map(|root| root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR)))
        .find(|candidate| candidate.is_file())
}

/// Editors found on this machine, Unity's own choice first.
pub fn detect_ides() -> Vec<DetectedIde> {
    let mut found: Vec<DetectedIde> = Vec::new();
    let mut push = |exe: PathBuf, from_unity: bool| {
        if exe.is_file()
            && !found.iter().any(|d| {
                d.exe
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&exe.to_string_lossy())
            })
        {
            found.push(DetectedIde {
                kind: IdeKind::of(&exe),
                exe,
                from_unity,
            });
        }
    };
    if let Some(exe) = unity_script_editor() {
        push(exe, true);
    }
    for exe in installed_candidates() {
        push(exe, false);
    }
    found
}

fn installed_candidates() -> Vec<PathBuf> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let mut candidates = Vec::new();
    if let Some(local) = env("LOCALAPPDATA") {
        candidates.push(local.join(r"Programs\Rider\bin\rider64.exe"));
        candidates.push(local.join(r"JetBrains\Toolbox\scripts\rider.cmd"));
        candidates.push(local.join(r"Programs\Microsoft VS Code\Code.exe"));
    }
    for program_files in [env("ProgramFiles"), env("ProgramFiles(x86)")]
        .into_iter()
        .flatten()
    {
        let jetbrains = program_files.join("JetBrains");
        if let Ok(entries) = std::fs::read_dir(&jetbrains) {
            let mut riders: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().contains("Rider"))
                })
                .map(|p| p.join(r"bin\rider64.exe"))
                .collect();
            riders.sort();
            riders.reverse(); // newest version first
            candidates.extend(riders);
        }
        candidates.push(program_files.join(r"Microsoft VS Code\Code.exe"));
    }
    candidates.extend(visual_studio());
    candidates
}

/// Visual Studio's `devenv.exe`, via Microsoft's `vswhere` tool.
fn visual_studio() -> Option<PathBuf> {
    let vswhere = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?)
        .join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    if !vswhere.is_file() {
        return None;
    }
    let mut command = Command::new(vswhere);
    command.args(["-latest", "-property", "productPath"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Unity's External Script Editor (`kScriptsDefaultApp_h…` under
/// `HKCU\Software\Unity Technologies\Unity Editor 5.x`, a NUL-terminated
/// UTF-8 string stored as binary).
#[cfg(windows)]
fn unity_script_editor() -> Option<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Unity Technologies\Unity Editor 5.x")
        .ok()?;
    key.enum_values()
        .filter_map(Result::ok)
        .find(|(name, _)| name.starts_with("kScriptsDefaultApp"))
        .and_then(|(_, value)| registry_path(&value.bytes))
}

#[cfg(not(windows))]
fn unity_script_editor() -> Option<PathBuf> {
    None
}

/// Decodes Unity's registry string: UTF-8 bytes up to the first NUL.
#[cfg_attr(not(windows), allow(dead_code))]
fn registry_path(bytes: &[u8]) -> Option<PathBuf> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r"C:\Proj\Assets\Scripts\TurnView.cs";

    #[test]
    fn launch_arguments_per_editor() {
        let file = Path::new(FILE);
        let rider = launch_for(Path::new(r"C:\Rider\bin\rider64.exe"), file, Some(98), None);
        assert_eq!(rider.args, ["--line", "98", FILE]);
        let rider_col = launch_for(Path::new("rider.cmd"), file, Some(98), Some(5));
        assert_eq!(rider_col.args, ["--line", "98", "--column", "5", FILE]);
        let code = launch_for(Path::new(r"C:\VS Code\Code.exe"), file, Some(98), Some(5));
        assert_eq!(code.args, ["--goto", &format!("{FILE}:98:5")]);
        let vs = launch_for(Path::new("devenv.exe"), file, Some(98), None);
        assert_eq!(vs.args, ["/edit", FILE, "/command", "Edit.GoTo 98"]);
        let other = launch_for(Path::new("notepad++.exe"), file, Some(98), None);
        assert_eq!(other.args, [FILE]);
        let no_line = launch_for(Path::new("rider64.exe"), file, None, None);
        assert_eq!(no_line.args, [FILE]);
    }

    #[test]
    fn custom_templates_handle_quotes_and_placeholders() {
        let file = Path::new(FILE);
        let launch = custom_launch(
            r#""C:\Program Files\Sublime Text\subl.exe" "{file}:{line}:{column}" --wait"#,
            file,
            Some(98),
            None,
        )
        .unwrap();
        assert_eq!(
            launch.program,
            PathBuf::from(r"C:\Program Files\Sublime Text\subl.exe")
        );
        assert_eq!(launch.args, [format!("{FILE}:98:1"), "--wait".to_owned()]);
        assert!(custom_launch("   ", file, None, None).is_none());
    }

    #[test]
    fn resolves_relative_paths_against_project_roots() {
        let root = std::env::temp_dir().join(format!("comte-ide-{}", std::process::id()));
        let script = root.join("Assets").join("Scripts").join("A.cs");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "class A {}").unwrap();

        let missing = PathBuf::from("definitely/not/here");
        assert_eq!(
            resolve_source("Assets/Scripts/A.cs", &[missing, root.clone()]),
            Some(script.clone())
        );
        assert_eq!(
            resolve_source(&script.display().to_string(), &[]),
            Some(script.clone())
        );
        assert_eq!(resolve_source("Assets/Scripts/B.cs", std::slice::from_ref(&root)), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recognizes_editor_families_and_registry_strings() {
        assert_eq!(IdeKind::of(Path::new(r"C:\x\rider64.exe")), IdeKind::Rider);
        assert_eq!(IdeKind::of(Path::new(r"C:\x\Code.exe")), IdeKind::VsCode);
        assert_eq!(
            IdeKind::of(Path::new(r"C:\x\devenv.exe")),
            IdeKind::VisualStudio
        );
        assert_eq!(IdeKind::of(Path::new(r"C:\x\vim.exe")), IdeKind::Other);
        assert_eq!(
            registry_path(b"C:\\Rider\\bin\\rider64.exe\0"),
            Some(PathBuf::from(r"C:\Rider\bin\rider64.exe"))
        );
        assert_eq!(registry_path(b"\0"), None);
    }
}
