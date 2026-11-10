//! Patterns shared by the Editor and Player parsers.

use std::time::Duration;

use comte_core::{Fact, FactKey, Metric, MetricKind, SourceRef};

use crate::frames::display_path;

// `-timestamps` / UNITY_EXT_LOGGING: `2022-04-26T08:59:29.580Z|0x7f43819a4300|message`
// (UTC time, thread id, then the line as usual).
regex!(
    TIMESTAMPED =
        r"^(?P<ts>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z)\|0x[0-9a-fA-F]+\|(?P<rest>.*)$"
);
regex!(EXCEPTION_HEADER = r"^(?:[A-Za-z_]\w*\.)*\w*Exception(?::\s|$)");
regex!(FILENAME = r"^\(Filename: (?P<path>.*?) Line: (?P<line>-?\d+)\)$");
regex!(
    UNLOADING = r"^Unloading (?P<count>\d+) unused Assets(?: / \((?P<size>[^)]+)\))?(?: to reduce memory usage)?\. Loaded Objects now: (?P<objects>\d+)"
);
regex!(GPU_ID_SUFFIX = r"\s*\(ID=0x[0-9a-fA-F]+\)\s*$");

/// `NullReferenceException: …`, `System.IO.IOException: …`, `UnityConnectWebRequestException: …`
/// Splits a timestamped line into its time (`2022-04-26T08:59:29.580Z`) and
/// the line itself.
pub fn split_timestamp(line: &str) -> Option<(&str, &str)> {
    let c = TIMESTAMPED.captures(line)?;
    Some((
        c.name("ts")?.as_str(),
        c.name("rest").map_or("", |m| m.as_str()),
    ))
}

pub fn is_exception_header(line: &str) -> bool {
    EXCEPTION_HEADER.is_match(line)
}

/// `(Filename: Assets/Foo.cs Line: 84)` → location. Line ≤ 0 means "unknown".
pub fn parse_filename_line(line: &str) -> Option<Option<SourceRef>> {
    let caps = FILENAME.captures(line)?;
    let path = display_path(&caps["path"]);
    let line_no: i64 = caps["line"].parse().unwrap_or(0);
    Some(path.map(|p| {
        let r = SourceRef::new(p);
        match u32::try_from(line_no) {
            Ok(l) if l > 0 => r.with_line(l),
            _ => r,
        }
    }))
}

/// `Unloading 54 unused Assets / (17.0 MB). Loaded Objects now: 3683.` (Editor)
/// `Unloading 3576 unused Assets to reduce memory usage. Loaded Objects now: 261350.` (Player)
pub struct Unloading {
    pub message: String,
    pub metrics: [Metric; 2],
}

pub fn parse_unloading(line: &str, line_no: usize) -> Option<Unloading> {
    let caps = UNLOADING.captures(line)?;
    let count: u64 = caps["count"].parse().ok()?;
    let objects: u64 = caps["objects"].parse().ok()?;
    let size = caps
        .name("size")
        .map(|s| format!(" ({})", s.as_str()))
        .unwrap_or_default();
    Some(Unloading {
        message: format!("Unloaded {count} unused assets{size}; {objects} objects still loaded"),
        metrics: [
            Metric::count(MetricKind::UnusedAssetsUnloaded, count, line_no),
            Metric::count(MetricKind::LoadedObjects, objects, line_no),
        ],
    })
}

/// One indented line of the `Direct3D:` block → a fact.
pub fn graphics_fact(line: &str, line_no: usize) -> Option<Fact> {
    let (key, value) = line.trim().split_once(':')?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let key = match key.trim() {
        "Version" => FactKey::GraphicsApi,
        "Renderer" => {
            let gpu = GPU_ID_SUFFIX.replace(value, "");
            return Some(Fact::new(FactKey::Gpu, gpu.into_owned(), line_no));
        }
        "VRAM" => FactKey::Vram,
        "Driver" => FactKey::GpuDriver,
        _ => return None,
    };
    Some(Fact::new(key, value, line_no))
}

/// `3.530` (seconds) → Duration.
pub fn secs(text: &str) -> Option<Duration> {
    let value: f64 = text.trim().parse().ok()?;
    (value.is_finite() && value >= 0.0).then(|| Duration::from_secs_f64(value))
}

/// `205.298` (milliseconds) → Duration.
pub fn millis(text: &str) -> Option<Duration> {
    secs(text).map(|d| d / 1000)
}

/// The file name part of a Unity asset path (`Assets/Scenes/Boot.unity` → `Boot.unity`).
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_headers() {
        assert!(is_exception_header(
            "NullReferenceException: Object reference not set to an instance of an object"
        ));
        assert!(is_exception_header(
            "System.IO.DirectoryNotFoundException: Could not find"
        ));
        assert!(is_exception_header("MissingReferenceException"));
        assert!(is_exception_header(
            "System.Exception: SteamApi_Init returned false."
        ));
        assert!(is_exception_header("Exception: plain"));
        assert!(!is_exception_header(
            "Exception for POST request at \"https://x\""
        ));
        assert!(!is_exception_header(
            "No platform profile, message: System.IO.DirectoryNotFoundException: x"
        ));
        assert!(!is_exception_header("Rethrow as ZenjectException: Error"));
    }

    #[test]
    fn filename_lines() {
        let r = parse_filename_line("(Filename: Assets/A/B.cs Line: 84)")
            .unwrap()
            .unwrap();
        assert_eq!(r.to_string(), "Assets/A/B.cs:84");
        let engine = parse_filename_line(
            r"(Filename: C:\buildslave\unity\build\Runtime/Export/Debug/Debug.bindings.h Line: 35)",
        )
        .unwrap()
        .unwrap();
        assert_eq!(engine.short(), "Debug.bindings.h:35");
        let unknown = parse_filename_line("(Filename:  Line: 0)").unwrap();
        assert!(unknown.is_none());
    }

    #[test]
    fn unloading_both_forms() {
        let editor = parse_unloading(
            "Unloading 54 unused Assets / (17.0 MB). Loaded Objects now: 3683.",
            7,
        )
        .unwrap();
        assert_eq!(
            editor.message,
            "Unloaded 54 unused assets (17.0 MB); 3683 objects still loaded"
        );
        let player = parse_unloading(
            "Unloading 3576 unused Assets to reduce memory usage. Loaded Objects now: 261350.",
            9,
        )
        .unwrap();
        assert_eq!(
            player.metrics[1].value,
            comte_core::MetricValue::Count(261_350)
        );
    }

    #[test]
    fn graphics_facts() {
        let gpu = graphics_fact(
            "    Renderer:        NVIDIA GeForce RTX 2070 Super (ID=0x1e91)",
            3,
        )
        .unwrap();
        assert_eq!(gpu.key, FactKey::Gpu);
        assert_eq!(gpu.value, "NVIDIA GeForce RTX 2070 Super");
        assert_eq!(
            graphics_fact("    Version:  Direct3D 11.0 [level 11.1]", 2)
                .unwrap()
                .value,
            "Direct3D 11.0 [level 11.1]"
        );
        assert!(graphics_fact("    Vendor:   ", 4).is_none());
    }
}
