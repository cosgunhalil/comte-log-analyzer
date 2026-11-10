//! Known Unity problems, explained (DESIGN §5, "What this might be").
//!
//! Two kinds:
//! - **Rules** match an entry's text: out of memory, a removed graphics
//!   device, a full disk, Package Manager, Gradle and IL2CPP failures.
//! - **Crash triage** reads a native crash's stack: which module it crashed
//!   in (a GPU driver, game code, Mono, the engine, a plugin), how many frames
//!   have no symbols, and where the crash report is.
//!
//! The wording says what it most likely is and what to try, never more than
//! the log supports.

use comte_core::{Hint, StackFrame, format_bytes};

regex!(
    ALLOCATE =
        r"Trying to allocate: (?P<bytes>\d+)B with \d+ alignment\. MemoryLabel: (?P<label>\w+)"
);
regex!(MODULE = r"^0x[0-9A-Fa-f]+ \((?P<module>[^)]*)\)");

/// An entry's text, for rules: message and detail lines.
pub fn entry_text(message: &str, details: &[String]) -> String {
    let mut text = message.to_owned();
    for line in details {
        text.push('\n');
        text.push_str(line);
    }
    text
}

/// The first rule matching `text`: its id (each rule is reported once per
/// log) and the hint (its `line` is filled in by the caller).
pub fn rule_hint(text: &str, gpu: Option<&str>) -> Option<(&'static str, Hint)> {
    let hint = |title: String, advice: Vec<String>| Hint {
        title,
        advice,
        line: 0,
        path: None,
        topic: String::new(), // the rule id, set by the caller
    };
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));

    if let Some(c) = ALLOCATE.captures(text) {
        let bytes: u128 = c["bytes"].parse().unwrap_or(0);
        let label = &c["label"];
        // Requests beyond a terabyte are no real need: a size computed from
        // garbage (a bug, or corrupted data).
        return Some(if bytes > 1 << 40 {
            (
                "oom",
                hint(
                    "Out of memory: Unity asked for an impossible amount at once".to_owned(),
                    vec![
                        format!(
                            "It asked for {bytes} bytes ({label}). No machine has that much: \
                             the size came from a bug or corrupted data, not from real memory use."
                        ),
                        "A damaged asset or .meta file can cause it; newer patch releases of \
                         the same Unity version often fix such bugs. Note what happened just before."
                            .to_owned(),
                    ],
                ),
            )
        } else {
            (
                "oom",
                hint(
                    format!(
                        "Out of memory: Unity couldn't get {} ({label})",
                        format_bytes(u64::try_from(bytes).unwrap_or(u64::MAX))
                    ),
                    vec![
                        "The process ran out of memory. Lower the peak: smaller textures and \
                         audio, unload scenes and asset bundles you no longer need."
                            .to_owned(),
                        "The Memory Profiler shows what holds the memory. On consoles and \
                         mobile the app's limit is far below the device's RAM."
                            .to_owned(),
                    ],
                ),
            )
        });
    }
    if has(&[
        "Could not allocate memory",
        "System out of memory",
        "OutOfMemoryException",
    ]) {
        return Some((
            "oom",
            hint(
                "Out of memory".to_owned(),
                vec![
                    "The process ran out of memory. The lines right after this one usually \
                     say how much was asked for and by what (MemoryLabel)."
                        .to_owned(),
                    "Lower the peak: smaller textures and audio, unload scenes and asset \
                     bundles you no longer need; the Memory Profiler shows what holds it."
                        .to_owned(),
                ],
            ),
        ));
    }
    if has(&[
        "device reset/removed",
        "due to device removed",
        "DXGI_ERROR_DEVICE_REMOVED",
        "DXGI_ERROR_DEVICE_HUNG",
        "DXGI_ERROR_DEVICE_RESET",
        "887a0005",
        "887a0006",
        "887A0005",
        "887A0006",
        "VK_ERROR_DEVICE_LOST",
    ]) {
        let mut advice = vec![
            "The graphics driver reset: it crashed, the GPU took too long on one frame \
             (Windows resets it after about 2 seconds), or video memory ran out."
                .to_owned(),
            "Update the graphics driver, or roll back if this started after an update.".to_owned(),
        ];
        match gpu {
            Some(gpu) if is_integrated_gpu(gpu) => advice.push(format!(
                "This ran on {gpu}, an integrated GPU. If the machine has a dedicated one, \
                 make Windows use it for Unity (Settings > Display > Graphics)."
            )),
            _ => advice.push(
                "If only some machines hit it, compare their GPUs and driver versions \
                 (see Environment)."
                    .to_owned(),
            ),
        }
        return Some((
            "gpu-removed",
            hint(
                "The graphics device was removed (the GPU driver reset)".to_owned(),
                advice,
            ),
        ));
    }
    if has(&[
        "No space left on device",
        "There is not enough space on the disk",
        "ERROR_DISK_FULL",
    ]) {
        return Some((
            "disk-full",
            hint(
                "The disk is full".to_owned(),
                vec![
                    "Free space on the drive with the project: Library, Temp and the build \
                     output grow a lot during imports and builds."
                        .to_owned(),
                ],
            ),
        ));
    }
    if has(&[
        "An error occurred while resolving packages",
        "Cannot perform upm operation",
        "Failed to resolve packages",
    ]) {
        return Some((
            "upm",
            hint(
                "Package Manager couldn't resolve the project's packages".to_owned(),
                vec![
                    "Check Packages/manifest.json for a typo or a package version that \
                     doesn't exist; a JSON error names the line."
                        .to_owned(),
                    "Network, proxy or Git problems also cause it. upm.log (next to \
                     Editor.log) has the details."
                        .to_owned(),
                ],
            ),
        ));
    }
    if has(&["Gradle build failed", "CommandInvokationFailure: Gradle"]) {
        return Some((
            "gradle",
            hint(
                "The Android (Gradle) build failed".to_owned(),
                vec![
                    "This line is only the summary. The cause is in Gradle's own output: \
                     look for \"What went wrong\" in the lines after it."
                        .to_owned(),
                    "The full Gradle log is in the project's Temp/gradleOut folder while \
                     Unity is open."
                        .to_owned(),
                ],
            ),
        ));
    }
    if has(&["il2cpp.exe did not run properly", "Il2CppBuildException"]) {
        return Some((
            "il2cpp",
            hint(
                "The IL2CPP step of the build failed".to_owned(),
                vec![
                    "Read IL2CPP's error output around this line. Common causes: a full \
                     disk, antivirus locking build files, or a missing C++ toolchain \
                     (Visual Studio's \"Desktop development with C++\")."
                        .to_owned(),
                ],
            ),
        ));
    }
    None
}

/// Laptop and APU graphics that share system memory.
fn is_integrated_gpu(gpu: &str) -> bool {
    let gpu = gpu.to_ascii_lowercase();
    [
        "intel(r) uhd",
        "intel(r) hd",
        "iris",
        "radeon(tm) graphics",
        "radeon graphics",
        "basic render",
    ]
    .iter()
    .any(|n| gpu.contains(n))
}

/// Counts from a native crash block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrashStats {
    /// Loaded modules listed before the stack.
    pub modules: usize,
    /// Of those, modules without a PDB (no symbols).
    pub modules_without_symbols: usize,
    /// `ERROR: SymGet…` lines: frames Windows couldn't name.
    pub symbol_errors: usize,
}

/// Where in the process a crash happened.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Place {
    GpuDriver {
        vendor: &'static str,
        module: String,
    },
    GameCode {
        method: String,
    },
    Mono,
    Engine {
        function: String,
    },
    Graphics {
        module: String,
    },
    Windows {
        module: String,
    },
    Other {
        module: String,
    },
}

fn module_of(frame: &StackFrame) -> Option<String> {
    MODULE
        .captures(frame.raw.trim())
        .map(|c| c["module"].trim().to_owned())
}

fn function_of(frame: &StackFrame) -> String {
    // Frames read `Function (Module)`; drop the module part.
    match frame.text.rfind(" (") {
        Some(i) => frame.text[..i].to_owned(),
        None => frame.text.clone(),
    }
}

fn is_windows(module: &str) -> bool {
    [
        "ntdll",
        "kernel32",
        "kernelbase",
        "ucrtbase",
        "msvcrt",
        "user32",
        "win32u",
        "combase",
        "vcruntime140",
        "msvcp140",
    ]
    .contains(&module.to_ascii_lowercase().as_str())
}

/// The crash handler's own frames at the top of an Editor crash.
fn is_crash_handler(frame: &StackFrame) -> bool {
    let text = frame.text.as_str();
    [
        "StackWalker",
        "CrashHandler",
        "crashpad",
        "DbgHelp",
        "PrintStack",
    ]
    .iter()
    .any(|n| text.contains(n))
}

fn gpu_vendor(module: &str) -> Option<&'static str> {
    let m = module.to_ascii_lowercase();
    if [
        "nvwgf2um",
        "nvd3dum",
        "nvoglv",
        "nvcuda",
        "nvldumd",
        "nvgpucomp",
    ]
    .iter()
    .any(|p| m.starts_with(p))
    {
        Some("NVIDIA")
    } else if [
        "atiumd", "atidxx", "amdxx", "atio6axx", "aticfx", "amdvlk", "amdxc",
    ]
    .iter()
    .any(|p| m.starts_with(p))
    {
        Some("AMD")
    } else if [
        "igd10",
        "igd12",
        "igdumd",
        "igc64",
        "igxelpicd",
        "ig9icd",
        "igvk",
    ]
    .iter()
    .any(|p| m.starts_with(p))
    {
        Some("Intel")
    } else {
        None
    }
}

fn place_of(stack: &[StackFrame]) -> Option<Place> {
    let mut windows = None;
    for frame in stack.iter().filter(|f| !is_crash_handler(f)) {
        let Some(module) = module_of(frame) else {
            continue;
        };
        let lower = module.to_ascii_lowercase();
        if is_windows(&module) {
            // Windows frames at the top just raise the error; look below.
            windows.get_or_insert(module);
            continue;
        }
        return Some(if let Some(vendor) = gpu_vendor(&module) {
            Place::GpuDriver { vendor, module }
        } else if lower == "gameassembly" {
            Place::GameCode {
                method: il2cpp_method(&function_of(frame)),
            }
        } else if lower.starts_with("mono") {
            Place::Mono
        } else if lower == "unityplayer" || lower == "unity" {
            Place::Engine {
                function: function_of(frame),
            }
        } else if ["d3d11", "d3d12", "dxgi", "vulkan-1", "opengl32"].contains(&lower.as_str()) {
            Place::Graphics { module }
        } else {
            Place::Other { module }
        });
    }
    windows.map(|module| Place::Windows { module })
}

/// `SampleGame_Enemies_EnemySpawner_Spawn_m3F2A…` → `SampleGame.Enemies.EnemySpawner.Spawn`
/// (IL2CPP joins namespace, class and method with `_` and adds a hash).
fn il2cpp_method(function: &str) -> String {
    let name = match function.rfind("_m") {
        Some(i)
            if function.len() > i + 2
                && function[i + 2..].chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            &function[..i]
        }
        _ => function,
    };
    name.replace('_', ".")
}

/// The triage hint for a native crash.
pub fn crash_hint(
    stack: &[StackFrame],
    stats: CrashStats,
    folder: Option<&str>,
    line: usize,
) -> Hint {
    let (title, mut advice) = match place_of(stack) {
        Some(Place::GpuDriver { vendor, module }) => (
            format!("Crashed inside the {vendor} graphics driver ({module})"),
            vec![
                "The crash is in the driver, not in Unity or your code. Update the driver, \
                 or roll back if it started after an update."
                    .to_owned(),
                "If it happens on some machines only, compare their driver versions \
                 (see Environment)."
                    .to_owned(),
            ],
        ),
        Some(Place::GameCode { method }) => (
            format!("Crashed in your game code (≈ {method})"),
            vec![
                "GameAssembly is your C# compiled by IL2CPP; the method name above is \
                 rebuilt from IL2CPP's mangled name. Start there."
                    .to_owned(),
                "A managed exception logged just before the crash often explains it.".to_owned(),
            ],
        ),
        Some(Place::Mono) => (
            "Crashed in the Mono scripting runtime".to_owned(),
            vec![
                "Often a stack overflow from endless recursion in a script, or a bad call \
                 into a native plugin. The managed frames or the last error before it say which."
                    .to_owned(),
            ],
        ),
        Some(Place::Engine { function }) => (
            format!("Crashed inside the Unity engine ({function})"),
            vec![
                "The last error before the crash is often what triggered it.".to_owned(),
                "Search Unity's issue tracker for the top frame, and try the latest patch \
                 release of this Unity version."
                    .to_owned(),
            ],
        ),
        Some(Place::Graphics { module }) => (
            format!("Crashed in the graphics runtime ({module})"),
            vec![
                "Usually the graphics driver underneath: update or roll back the driver."
                    .to_owned(),
            ],
        ),
        Some(Place::Windows { module }) => (
            format!("Crashed in Windows code ({module})"),
            vec![
                "Windows reported the fault, but the cause is usually the caller: check \
                 the frames below and the last error before the crash."
                    .to_owned(),
            ],
        ),
        Some(Place::Other { module }) => (
            format!("Crashed in {module}, outside Unity"),
            vec![
                "Probably a native plugin, or software that hooks into games (overlays, \
                 recorders, antivirus). Try again without it."
                    .to_owned(),
            ],
        ),
        None => (
            "Native crash".to_owned(),
            vec!["The log has no native stack trace for it.".to_owned()],
        ),
    };
    let unnamed = stack
        .iter()
        .filter(|f| f.text.contains("function-name not available"))
        .count();
    if unnamed > 0 || stats.symbol_errors > 0 {
        let unnamed = unnamed.max(stats.symbol_errors.min(stack.len()));
        advice.push(format!(
            "{unnamed} of {} frames have no names: Windows had no symbols for them, \
             which is normal on players' machines. With the build's symbols (Unity's \
             symbol server, GameAssembly.pdb) a debugger can name them.",
            stack.len()
        ));
    }
    if folder.is_some() {
        advice.push(
            "The crash report folder has a .dmp file to open in Visual Studio or WinDbg."
                .to_owned(),
        );
    }
    Hint {
        title,
        advice,
        line,
        path: folder.map(str::to_owned),
        topic: "crash".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use comte_core::FrameKind;

    use super::*;

    fn frame(raw: &str) -> StackFrame {
        let text = raw.split_once(") ").map_or(raw, |(_, f)| f);
        let module = MODULE.captures(raw).unwrap()["module"].to_owned();
        StackFrame::new(format!("{text} ({module})"), FrameKind::Native).with_raw(raw)
    }

    #[test]
    fn crash_place_skips_windows_frames_and_names_the_driver() {
        let stack = [
            frame("0x00007FFC1A2D7034 (KERNELBASE) RaiseException"),
            frame("0x00007FFB2ABF1E3D (nvwgf2umx) OpenAdapter12"),
            frame("0x00007FFB2AB8C1D2 (UnityPlayer) GfxDeviceD3D11::Present"),
        ];
        let hint = crash_hint(&stack, CrashStats::default(), None, 7);
        assert_eq!(
            hint.title,
            "Crashed inside the NVIDIA graphics driver (nvwgf2umx)"
        );
        assert_eq!(hint.line, 7);
    }

    #[test]
    fn game_code_crash_rebuilds_the_method_and_counts_unnamed_frames() {
        let stack = [
            frame("0x00007FFB3D2E4C1B (GameAssembly) SampleGame_Enemies_EnemySpawner_Spawn_m3F2A"),
            frame("0x00007FFB2ABF1E3D (UnityPlayer) (function-name not available)"),
            frame("0x00007FFC1B8A2651 (ntdll) RtlUserThreadStart"),
        ];
        let stats = CrashStats {
            symbol_errors: 1,
            ..CrashStats::default()
        };
        let hint = crash_hint(&stack, stats, Some("C:/Temp/Crashes"), 3);
        assert_eq!(
            hint.title,
            "Crashed in your game code (≈ SampleGame.Enemies.EnemySpawner.Spawn)"
        );
        assert!(
            hint.advice
                .iter()
                .any(|a| a.starts_with("1 of 3 frames have no names"))
        );
        assert_eq!(hint.path.as_deref(), Some("C:/Temp/Crashes"));
    }

    #[test]
    fn memory_rules_tell_a_bug_from_real_pressure() {
        let (_, absurd) = rule_hint(
            "Trying to allocate: 18446744056529682440B with 16 alignment. MemoryLabel: TempOverflow",
            None,
        )
        .unwrap();
        assert_eq!(
            absurd.title,
            "Out of memory: Unity asked for an impossible amount at once"
        );
        let (_, real) = rule_hint(
            "Could not allocate memory: System out of memory!\nTrying to allocate: 268435456B with 16 alignment. MemoryLabel: GfxDevice",
            None,
        )
        .unwrap();
        assert_eq!(
            real.title,
            "Out of memory: Unity couldn't get 256.0 MB (GfxDevice)"
        );
    }

    #[test]
    fn device_removed_mentions_an_integrated_gpu() {
        let (id, hint) = rule_hint(
            "Failed to present D3D11 swapchain due to device reset/removed.",
            Some("Intel(R) UHD Graphics 620"),
        )
        .unwrap();
        assert_eq!(id, "gpu-removed");
        assert!(hint.advice[2].contains("Intel(R) UHD Graphics 620, an integrated GPU"));
        let (_, dedicated) =
            rule_hint("DXGI_ERROR_DEVICE_REMOVED", Some("NVIDIA GeForce RTX 2070")).unwrap();
        assert!(dedicated.advice[2].starts_with("If only some machines"));
    }

    #[test]
    fn other_rules_and_ordinary_text() {
        let title = |text| rule_hint(text, None).map(|(_, h)| h.title);
        assert_eq!(
            title("ld: No space left on device").as_deref(),
            Some("The disk is full")
        );
        assert_eq!(
            title("An error occurred while resolving packages:").as_deref(),
            Some("Package Manager couldn't resolve the project's packages")
        );
        assert_eq!(
            title("CommandInvokationFailure: Gradle build failed.").as_deref(),
            Some("The Android (Gradle) build failed")
        );
        assert_eq!(title("[Flow] Boot -> MainMenu"), None);
    }
}
