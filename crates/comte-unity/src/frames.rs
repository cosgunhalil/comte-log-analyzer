//! Stack frame parsing, classification and cleanup (DESIGN §4.3).
//!
//! Unity writes two frame syntaxes:
//!
//! - **Unity form** (`Debug.Log` traces, editor errors):
//!   `SpiritCabinet.Installers.GameInstaller:Report (SpiritCabinet.Text.Import.TextData) (at Assets/Scripts/GameInstaller.cs:84)`
//!   (Unity 2019 omits the space before `(`.)
//! - **Mono form** (exceptions):
//!   `  at SpiritCabinet.UI.TurnView.HideFile () [0x00001] in C:\…\Assets\Scripts\UI\TurnView.cs:98`

use comte_core::{FrameKind, Severity, SourceRef, StackFrame};

regex!(
    UNITY_FRAME = r"^(?P<type>[^\s:()]+):(?P<method>[^\s(]+) ?\((?P<args>[^)]*)\)(?: \(at (?P<path>.+):(?P<line>\d+)\))?$"
);
regex!(
    MONO_FRAME =
        r"^\s+at (?P<sig>.+?)(?: \[0x[0-9a-fA-F]+\])?(?: in (?P<path>.+?):(?P<line>\d+))?\s*$"
);
regex!(ASYNC_STATE_MACHINE = r"^<(?P<name>[^>]+)>d__\d+$");
regex!(LAMBDA_METHOD = r"^<(?P<name>[^>]+)>b__\w+$");
regex!(CLOSURE_CLASS = r"^<>c(?:__DisplayClass[\w]*)?$");

/// Namespaces that belong to the engine or the .NET runtime.
const ENGINE_PREFIXES: &[&str] = &[
    "UnityEngine.",
    "UnityEditor.",
    "UnityEditorInternal.",
    "Unity.",
    "System.",
    "Mono.",
    "Microsoft.",
    "Bee.",
    "(wrapper",
];

/// Well-known third-party namespaces, used when a frame has no file path
/// (release Player builds).
const PACKAGE_PREFIXES: &[&str] = &[
    "TMPro.",
    "Cinemachine.",
    "Newtonsoft.",
    "Cysharp.",
    "DG.Tweening.",
    "Zenject.",
    "Sirenix.",
    "Steamworks.",
    "Epic.OnlineServices.",
    "FMOD",
    "Photon.",
    "Mirror.",
];

/// A parsed frame plus the raw type/method names (needed to recognise
/// logging plumbing such as `UnityEngine.Debug:LogWarning`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFrame {
    pub type_name: String,
    pub method: String,
    pub frame: StackFrame,
}

/// Parses a line in either frame syntax.
pub fn parse_frame(line: &str) -> Option<ParsedFrame> {
    parse_unity_frame(line).or_else(|| parse_mono_frame(line))
}

pub fn parse_unity_frame(line: &str) -> Option<ParsedFrame> {
    let caps = UNITY_FRAME.captures(line.trim_end())?;
    let type_name = &caps["type"];
    let method = &caps["method"];
    let args = &caps["args"];
    let path = caps.name("path").map(|m| m.as_str());
    let line_no = caps.name("line").and_then(|m| m.as_str().parse().ok());
    let raw = format!("{type_name}:{method} ({args})");
    Some(build(type_name, method, args, path, line_no, raw))
}

pub fn parse_mono_frame(line: &str) -> Option<ParsedFrame> {
    let caps = MONO_FRAME.captures(line)?;
    let sig = caps["sig"].trim();
    let path = caps.name("path").map(|m| m.as_str());
    let line_no = caps.name("line").and_then(|m| m.as_str().parse().ok());

    let (name, args) = match sig.find('(') {
        Some(open) if !sig.starts_with('(') => {
            let close = sig.rfind(')').unwrap_or(sig.len());
            (sig[..open].trim(), sig.get(open + 1..close).unwrap_or(""))
        }
        _ => (sig, ""),
    };
    let (type_name, method) = match name.rfind('.') {
        Some(dot) if !name.starts_with('(') => (&name[..dot], &name[dot + 1..]),
        _ => ("", name),
    };
    Some(build(
        type_name,
        method,
        args,
        path,
        line_no,
        sig.to_owned(),
    ))
}

fn build(
    type_name: &str,
    method: &str,
    args: &str,
    path: Option<&str>,
    line: Option<u32>,
    raw: String,
) -> ParsedFrame {
    let location = path.and_then(display_path).map(|p| {
        let r = SourceRef::new(p);
        match line {
            Some(l) if l > 0 => r.with_line(l),
            _ => r,
        }
    });
    let kind = classify(type_name, path);
    let text = clean_signature(type_name, method, args);
    let mut frame = StackFrame::new(text, kind).with_raw(raw);
    frame.location = location;
    ParsedFrame {
        type_name: type_name.to_owned(),
        method: method.to_owned(),
        frame,
    }
}

/// Decides who owns a frame: the game (`Assets/`), a package, or the engine.
///
/// `Assets/Plugins/` is treated as third-party code, so the culprit points at
/// the game's own scripts rather than at a plugin it called into.
pub fn classify(type_name: &str, path: Option<&str>) -> FrameKind {
    if let Some(path) = path.filter(|p| !p.starts_with('<')) {
        let p = normalize_path(path);
        if p.starts_with("Assets/Plugins/") || p.contains("/Assets/Plugins/") {
            return FrameKind::Package;
        }
        if p.starts_with("Assets/") || p.contains("/Assets/") {
            return FrameKind::User;
        }
        if p.starts_with("Packages/")
            || p.contains("/Packages/")
            || p.contains("Library/PackageCache/")
        {
            return FrameKind::Package;
        }
        return FrameKind::Engine;
    }
    if type_name.is_empty() || ENGINE_PREFIXES.iter().any(|p| type_name.starts_with(p)) {
        FrameKind::Engine
    } else if PACKAGE_PREFIXES.iter().any(|p| type_name.starts_with(p)) {
        FrameKind::Package
    } else {
        FrameKind::User
    }
}

/// `C:\Proj\Assets\A.cs` → `Assets/A.cs`; `./Library/PackageCache/x/A.cs` →
/// `Library/PackageCache/x/A.cs`. Mono's `<hash>` pseudo-paths → `None`.
pub fn display_path(path: &str) -> Option<String> {
    if path.starts_with('<') || path.is_empty() {
        return None;
    }
    let p = normalize_path(path);
    for anchor in ["Assets/", "Library/PackageCache/", "Packages/"] {
        if p.starts_with(anchor) {
            return Some(p);
        }
        if let Some(i) = p.find(&format!("/{anchor}")) {
            return Some(p[i + 1..].to_owned());
        }
    }
    Some(p)
}

fn normalize_path(path: &str) -> String {
    let p = path.trim().replace('\\', "/");
    p.strip_prefix("./").map(str::to_owned).unwrap_or(p)
}

/// `SpiritCabinet.Installers.GameInstaller` + `Report` + `SpiritCabinet.Text.Import.TextData`
/// → `GameInstaller.Report(TextData)`.
///
/// Compiler-generated names are made readable:
/// `Foo/<Run>d__3:MoveNext ()` → `Foo.Run() [async]`,
/// `Foo/<>c:<Start>b__0_0 ()` → `Foo.Start() [lambda]`.
pub fn clean_signature(type_name: &str, method: &str, args: &str) -> String {
    let unified = type_name.replace('/', "+");
    let mut parts: Vec<&str> = unified
        .split('+')
        .map(|part| part.rsplit('.').next().unwrap_or(part))
        .filter(|part| !part.is_empty())
        .collect();

    let mut method = method.to_owned();
    let mut tag = "";
    if let Some(last) = parts.last().copied() {
        if let Some(caps) = ASYNC_STATE_MACHINE.captures(last)
            && method == "MoveNext"
        {
            method = caps["name"].to_owned();
            tag = " [async]";
            parts.pop();
        } else if CLOSURE_CLASS.is_match(last) {
            parts.pop();
        }
    }
    if let Some(caps) = LAMBDA_METHOD.captures(&method.clone()) {
        method = caps["name"].to_owned();
        tag = " [lambda]";
    }

    let args = args
        .split(',')
        .filter_map(|arg| arg.split_whitespace().next())
        .map(|ty| ty.rsplit('.').next().unwrap_or(ty))
        .collect::<Vec<_>>()
        .join(", ");

    if parts.is_empty() {
        format!("{method}({args}){tag}")
    } else {
        format!("{}.{method}({args}){tag}", parts.join("."))
    }
}

/// Frames that only exist because something was logged
/// (`Debug.Log` → `Logger` → `DebugLogHandler` → `StackTraceUtility`).
pub fn is_plumbing(frame: &ParsedFrame) -> bool {
    matches!(
        frame.type_name.as_str(),
        "UnityEngine.Debug"
            | "UnityEngine.StackTraceUtility"
            | "UnityEngine.DebugLogHandler"
            | "UnityEngine.Logger"
    )
}

/// Severity implied by a `UnityEngine.Debug` frame (`LogWarning` → Warning, ...).
pub fn log_severity(frame: &ParsedFrame) -> Option<Severity> {
    if frame.type_name != "UnityEngine.Debug" {
        return None;
    }
    match frame.method.as_str() {
        "Log" | "LogFormat" => Some(Severity::Info),
        "LogWarning" | "LogWarningFormat" => Some(Severity::Warning),
        "LogError" | "LogErrorFormat" | "LogAssertion" | "LogAssertionFormat" | "Assert"
        | "AssertFormat" => Some(Severity::Error),
        "LogException" => Some(Severity::Exception),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unity6_frame_with_location() {
        let f = parse_frame(
            "SpiritCabinet.Installers.GameInstaller:Report (SpiritCabinet.Text.Import.TextData) (at Assets/SpiritCabinet/Scripts/Installers/GameInstaller.cs:84)",
        )
        .unwrap();
        assert_eq!(f.frame.text, "GameInstaller.Report(TextData)");
        assert_eq!(f.frame.kind, FrameKind::User);
        assert_eq!(
            f.frame.location.unwrap().to_string(),
            "Assets/SpiritCabinet/Scripts/Installers/GameInstaller.cs:84"
        );
        assert_eq!(
            f.frame.raw,
            "SpiritCabinet.Installers.GameInstaller:Report (SpiritCabinet.Text.Import.TextData)"
        );
    }

    #[test]
    fn parses_unity2019_frame_without_space() {
        let f = parse_frame("FiveLives.Platform.EpicPlatform:Awake()").unwrap();
        assert_eq!(f.frame.text, "EpicPlatform.Awake()");
        assert_eq!(
            f.frame.kind,
            FrameKind::User,
            "no path, not engine namespace"
        );
        assert!(f.frame.location.is_none());

        let debug = parse_frame("UnityEngine.Debug:LogWarning(Object)").unwrap();
        assert!(is_plumbing(&debug));
        assert_eq!(log_severity(&debug), Some(Severity::Warning));
    }

    #[test]
    fn parses_mono_frames() {
        let user = parse_frame(
            r"  at SpiritCabinet.UI.TurnView.HideFile () [0x00001] in C:\Users\dev\Projects\Game\Assets\SpiritCabinet\Scripts\UI\TurnView.cs:98 ",
        )
        .unwrap();
        assert_eq!(user.frame.text, "TurnView.HideFile()");
        assert_eq!(user.frame.kind, FrameKind::User);
        assert_eq!(
            user.frame.location.unwrap().to_string(),
            "Assets/SpiritCabinet/Scripts/UI/TurnView.cs:98"
        );

        let engine = parse_frame(
            "  at UnityEngine.GameObject.SetActive (System.Boolean value) [0x00006] in <1b6535aa091d41cf8b3ca222ed9ceef0>:0 ",
        )
        .unwrap();
        assert_eq!(engine.frame.text, "GameObject.SetActive(Boolean)");
        assert_eq!(engine.frame.kind, FrameKind::Engine);
        assert!(engine.frame.location.is_none());

        let plugin = parse_frame(
            r"  at Zenject.DisposableManager.Dispose () [0x000d7] in C:\P\Game\Assets\Plugins\Zenject\Source\Runtime\DisposableManager.cs:122 ",
        )
        .unwrap();
        assert_eq!(plugin.frame.kind, FrameKind::Package);
    }

    #[test]
    fn mono_release_frame_without_path_uses_namespace() {
        let f = parse_frame(
            "  at Steamworks.SteamClient.Init (System.UInt32 appid, System.Boolean asyncCallbacks) [0x00046] in <19bd4cc6a4004258bf538b05ac1133d6>:0 ",
        )
        .unwrap();
        assert_eq!(f.frame.text, "SteamClient.Init(UInt32, Boolean)");
        assert_eq!(f.frame.kind, FrameKind::Package);
    }

    #[test]
    fn package_cache_paths_are_packages() {
        let f = parse_frame(
            "UnityEngine.EventSystems.EventSystem:Update () (at ./Library/PackageCache/com.unity.ugui@f17df9b1ab21/Runtime/UGUI/EventSystem/EventSystem.cs:514)",
        )
        .unwrap();
        assert_eq!(f.frame.kind, FrameKind::Package);
        assert_eq!(f.frame.location.unwrap().short(), "EventSystem.cs:514");
    }

    #[test]
    fn cleans_compiler_generated_names() {
        assert_eq!(
            clean_signature("Game.Flow.Boot/<Run>d__3", "MoveNext", ""),
            "Boot.Run() [async]"
        );
        assert_eq!(
            clean_signature(
                "TMPro.EditorEventCallbacks/<>c",
                "<InitializeFontAssetResourceChangeCallBacks>b__0_0",
                ""
            ),
            "EditorEventCallbacks.InitializeFontAssetResourceChangeCallBacks() [lambda]"
        );
        assert_eq!(
            clean_signature(
                "System.Runtime.CompilerServices.AsyncMethodBuilderCore+<>c",
                "<ThrowAsync>b__7_0",
                "System.Object state"
            ),
            "AsyncMethodBuilderCore.ThrowAsync(Object) [lambda]"
        );
    }

    #[test]
    fn plain_text_is_not_a_frame() {
        for line in [
            "Domain Reload Profiling: 1304ms",
            "Mono: successfully reloaded assembly",
            "d3d12: failed to query info queue interface (0x80004002).",
            "OS: 'Windows 11  (10.0.26200) CoreSingleLanguage' Language: 'en'",
            "info: Microsoft.Hosting.Lifetime[14]",
        ] {
            assert!(parse_frame(line).is_none(), "{line}");
        }
    }

    #[test]
    fn debug_severity_mapping() {
        let sev = |m: &str| {
            log_severity(&parse_frame(&format!("UnityEngine.Debug:{m} (object)")).unwrap())
        };
        assert_eq!(sev("Log"), Some(Severity::Info));
        assert_eq!(sev("LogError"), Some(Severity::Error));
        assert_eq!(sev("LogException"), Some(Severity::Exception));
        assert_eq!(sev("ExtractStackTraceNoAlloc"), None);
    }
}
