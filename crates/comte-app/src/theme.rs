//! Colors and icons shared by the UI.

use comte_core::Severity;
use eframe::egui::{Color32, Visuals};

pub fn severity_color(visuals: &Visuals, severity: Severity) -> Color32 {
    match severity {
        Severity::Info => visuals.weak_text_color(),
        Severity::Warning => visuals.warn_fg_color,
        Severity::Error | Severity::Exception => visuals.error_fg_color,
        Severity::Crash => Color32::from_rgb(0xff, 0x4f, 0xa3),
    }
}

pub fn severity_icon(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "ℹ",
        Severity::Warning => "⚠",
        Severity::Error => "⛔",
        Severity::Exception => "⛔",
        Severity::Crash => "💥",
    }
}
