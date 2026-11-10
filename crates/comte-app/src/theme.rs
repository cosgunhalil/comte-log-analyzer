//! Colors and icons shared by the UI.

use comte_core::{Severity, markup};
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId, Visuals};

/// A message as styled text: Unity rich-text colors and italics applied,
/// tags removed. `base` is the color of untagged text. With `rich` off, the
/// message is shown as written, tags included.
pub fn message_job(
    text: &str,
    font: FontId,
    base: Color32,
    visuals: &Visuals,
    rich: bool,
) -> LayoutJob {
    let format = |color: Color32, italics: bool| TextFormat {
        font_id: font.clone(),
        color,
        italics,
        ..Default::default()
    };
    if !rich || !markup::has_markup(text) {
        return LayoutJob::single_section(text.to_owned(), format(base, false));
    }
    let mut job = LayoutJob::default();
    for span in markup::parse(text) {
        let color = match span.color {
            Some([r, g, b, a]) => readable(
                Color32::from_rgba_unmultiplied(r, g, b, a.max(160)),
                visuals,
            ),
            None if span.bold => visuals.strong_text_color(),
            None => base,
        };
        job.append(&span.text, 0.0, format(color, span.italic));
    }
    job
}

/// Background for search matches.
pub fn highlight_color(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgba_unmultiplied(255, 196, 0, 80)
    } else {
        Color32::from_rgba_unmultiplied(255, 200, 0, 130)
    }
}

/// Marks byte `ranges` of `job.text` with a background color, splitting the
/// job's sections where needed. Ranges must lie on character boundaries.
pub fn highlight(
    mut job: LayoutJob,
    ranges: &[std::ops::Range<usize>],
    background: Color32,
) -> LayoutJob {
    if ranges.is_empty() {
        return job;
    }
    let mut sections = Vec::with_capacity(job.sections.len() + ranges.len() * 2);
    for section in std::mem::take(&mut job.sections) {
        // egui ranges are `ByteIndex` newtypes over `usize`.
        let range = section.byte_range.start.0..section.byte_range.end.0;
        let mut cursor = range.start;
        let mut first = true;
        let mut push = |sections: &mut Vec<_>, part: std::ops::Range<usize>, marked: bool| {
            if part.is_empty() {
                return;
            }
            let mut piece = section.clone();
            if !std::mem::take(&mut first) {
                piece.leading_space = 0.0;
            }
            piece.byte_range = part.start.into()..part.end.into();
            if marked {
                piece.format.background = background;
            }
            sections.push(piece);
        };
        for hit in ranges
            .iter()
            .filter(|h| h.start < range.end && h.end > range.start)
        {
            let start = hit.start.max(range.start);
            let end = hit.end.min(range.end);
            push(&mut sections, cursor..start, false);
            push(&mut sections, start..end, true);
            cursor = end;
        }
        push(&mut sections, cursor..range.end, false);
    }
    job.sections = sections;
    job
}

/// Keeps a log's own color legible on the current theme: Unity's console is
/// dark, so `white` or `yellow` would vanish on a light background. Colors
/// with too little contrast are mixed toward the theme's text color.
pub fn readable(color: Color32, visuals: &Visuals) -> Color32 {
    let background = visuals.panel_fill;
    let target = visuals.strong_text_color();
    let mut out = color;
    for step in 1..=4 {
        if contrast(out, background) >= 3.0 {
            break;
        }
        out = mix(color, target, step as f32 * 0.25);
    }
    out
}

fn luminance(c: Color32) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG contrast ratio, 1 (none) to 21 (black on white).
fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_stay_legible_on_both_themes() {
        let light = Visuals::light();
        let dark = Visuals::dark();
        let cyan = Color32::from_rgb(0, 255, 255);
        let white = Color32::WHITE;
        assert_eq!(readable(cyan, &dark), cyan, "cyan reads fine on dark");
        assert_ne!(readable(cyan, &light), cyan, "cyan is too pale on light");
        assert!(contrast(readable(cyan, &light), light.panel_fill) >= 3.0);
        assert!(contrast(readable(white, &light), light.panel_fill) >= 3.0);
        let navy = Color32::from_rgb(0, 0, 128);
        assert!(contrast(readable(navy, &dark), dark.panel_fill) >= 3.0);
    }

    #[test]
    fn highlight_splits_sections_at_matches() {
        let visuals = Visuals::dark();
        let job = message_job(
            "[10:15] <color=cyan>Starting new race</color>",
            FontId::proportional(13.0),
            Color32::GRAY,
            &visuals,
            true,
        );
        // job.text is "[10:15] Starting new race"; highlight "new" (17..20)
        // inside the cyan section and "10" (1..3) inside the plain one.
        let marked = highlight(job, &[1..3, 17..20], Color32::YELLOW);
        let pieces: Vec<(&str, bool, Color32)> = marked
            .sections
            .iter()
            .map(|s| {
                (
                    &marked.text[s.byte_range.start.0..s.byte_range.end.0],
                    s.format.background == Color32::YELLOW,
                    s.format.color,
                )
            })
            .collect();
        let cyan = Color32::from_rgb(0, 255, 255);
        assert_eq!(
            pieces,
            [
                ("[", false, Color32::GRAY),
                ("10", true, Color32::GRAY),
                (":15] ", false, Color32::GRAY),
                ("Starting ", false, cyan),
                ("new", true, cyan),
                (" race", false, cyan),
            ]
        );
    }

    #[test]
    fn message_job_strips_tags_only_when_rich() {
        let visuals = Visuals::dark();
        let text = "[10:15] <color='cyan'>Starting new race</color>";
        let rich = message_job(
            text,
            FontId::proportional(13.0),
            Color32::GRAY,
            &visuals,
            true,
        );
        assert_eq!(rich.text, "[10:15] Starting new race");
        assert_eq!(rich.sections.len(), 2);
        assert_eq!(
            rich.sections[1].format.color,
            Color32::from_rgb(0, 255, 255)
        );
        let raw = message_job(
            text,
            FontId::proportional(13.0),
            Color32::GRAY,
            &visuals,
            false,
        );
        assert_eq!(raw.text, text);
    }
}
