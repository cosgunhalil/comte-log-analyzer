//! The timeline strip beside the entry list: the log's top-level sections
//! (Startup, Script compilation, Play mode #1, …) as bands, tinted by their
//! worst problem, with ticks for problems, pins and search matches, and a
//! frame around the part of the list on screen.
//!
//! Hover a band for the section's name and counts; click it to go to the
//! section, click a tick to select its entry, drag to scroll. The legend
//! under the list turns each layer on or off.
//!
//! Everything is placed by *row*, so it lines up with what the list shows,
//! whatever is filtered or collapsed.

use comte_core::{LogDocument, LogEntry, Section, Severity, format_duration};
use eframe::egui::{self, Color32, Rect, RichText, Sense, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::search::Query;
use crate::view::{Row, section_path};

/// Which layers the strip shows (saved in settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StripLayers {
    pub sections: bool,
    pub problems: bool,
    pub pins: bool,
    pub matches: bool,
}

impl Default for StripLayers {
    fn default() -> Self {
        Self {
            sections: true,
            problems: true,
            pins: true,
            matches: true,
        }
    }
}

/// A top-level section's run of rows: `rows.start..rows.end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Band {
    pub section: u32,
    pub rows: std::ops::Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickKind {
    Error,
    Warning,
    Pin,
    Match,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    pub row: usize,
    pub kind: TickKind,
}

/// What the strip draws for the current rows.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StripData {
    pub bands: Vec<Band>,
    pub ticks: Vec<Tick>,
}

/// The top-level section a row belongs to.
fn root_of(row: &Row, doc: &LogDocument) -> Option<u32> {
    let section = match *row {
        Row::Section { id, .. } => Some(id),
        Row::Entry { id, .. } => doc.entry(id)?.section_id,
    };
    section_path(doc, section).first().copied()
}

/// Bands and ticks for the current rows. `matches` is the search when it
/// only highlights (when it filters, every row matches, so it adds nothing).
/// A collapsed section header gets the tick of the worst problem it hides.
pub fn layout(
    rows: &[Row],
    doc: &LogDocument,
    layers: StripLayers,
    is_pinned: impl Fn(&LogEntry) -> bool,
    is_collapsed: impl Fn(u32) -> bool,
    matches: Option<&Query>,
) -> StripData {
    let mut data = StripData::default();
    let matches = matches.filter(|q| layers.matches && !q.is_empty());
    for (row, item) in rows.iter().enumerate() {
        if layers.sections
            && let Some(root) = root_of(item, doc)
        {
            match data.bands.last_mut() {
                Some(band) if band.section == root && band.rows.end == row => band.rows.end += 1,
                _ => data.bands.push(Band {
                    section: root,
                    rows: row..row + 1,
                }),
            }
        }
        let mut tick = |kind| data.ticks.push(Tick { row, kind });
        match *item {
            Row::Entry { id, .. } => {
                let Some(entry) = doc.entry(id) else {
                    continue;
                };
                if layers.problems {
                    if entry.severity.is_error_like() {
                        tick(TickKind::Error);
                    } else if entry.severity == Severity::Warning {
                        tick(TickKind::Warning);
                    }
                }
                if layers.pins && is_pinned(entry) {
                    tick(TickKind::Pin);
                }
                if matches.is_some_and(|q| q.matches_entry(entry)) {
                    tick(TickKind::Match);
                }
            }
            Row::Section { id, .. } => {
                let Some(section) = doc.section(id) else {
                    continue;
                };
                if layers.problems && is_collapsed(id) {
                    if section.counts.errors() > 0 {
                        tick(TickKind::Error);
                    } else if section.counts.warning > 0 {
                        tick(TickKind::Warning);
                    }
                }
                if matches.is_some_and(|q| !q.ranges(&section.title).is_empty()) {
                    tick(TickKind::Match);
                }
            }
        }
    }
    data
}

/// The pin color, in the list, the details and the strip.
pub fn pin_color() -> Color32 {
    Color32::from_rgb(0x8e, 0x5b, 0xd8)
}

fn match_color() -> Color32 {
    Color32::from_rgb(0xf5, 0xc5, 0x18)
}

fn tick_color(kind: TickKind, visuals: &egui::Visuals) -> Color32 {
    match kind {
        TickKind::Error => visuals.error_fg_color,
        TickKind::Warning => visuals.warn_fg_color,
        TickKind::Pin => pin_color(),
        TickKind::Match => match_color(),
    }
}

/// A band's fill: tinted by its worst problem, else alternating neutrals so
/// neighbouring sections stay apart.
fn band_fill(
    section: &Section,
    index: usize,
    layers: StripLayers,
    visuals: &egui::Visuals,
) -> Color32 {
    let tint = |c: Color32, a: u8| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
    if layers.problems && section.counts.errors() > 0 {
        tint(visuals.error_fg_color, 60)
    } else if layers.problems && section.counts.warning > 0 {
        tint(visuals.warn_fg_color, 55)
    } else {
        tint(
            visuals.text_color(),
            if index.is_multiple_of(2) { 22 } else { 38 },
        )
    }
}

fn band_tooltip(section: &Section) -> String {
    let mut text = section.title.clone();
    if let Some(d) = section.duration {
        text.push_str(&format!(" · {}", format_duration(d)));
    }
    let (errors, warnings) = (section.counts.errors(), section.counts.warning);
    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    text.push('\n');
    text.push_str(&if errors + warnings == 0 {
        "no problems".to_owned()
    } else {
        format!(
            "{}, {}",
            plural(errors, "error", "errors"),
            plural(warnings, "warning", "warnings")
        )
    });
    text.push_str("\nClick to go there");
    text
}

/// What the user did with the strip this frame.
#[derive(Debug, Default)]
pub struct StripResponse {
    /// Scroll to this row.
    pub go_to_row: Option<usize>,
    /// The click landed on a tick: select that row's entry too.
    pub on_tick: bool,
}

/// Draws the strip in `rect`. `viewport` is the visible part of the list as
/// fractions (start, end) of its height.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    data: &StripData,
    doc: &LogDocument,
    total_rows: usize,
    viewport: (f32, f32),
    layers: StripLayers,
) -> StripResponse {
    let mut out = StripResponse::default();
    let response = ui.interact(
        rect,
        egui::Id::unique("comte-timeline-strip"),
        Sense::click_and_drag(),
    );
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);

    let total = total_rows.max(1) as f32;
    let y_top = |row: usize| rect.top() + row as f32 / total * rect.height();
    let y_mid = |row: usize| rect.top() + (row as f32 + 0.5) / total * rect.height();

    // Section bands, with a thin gap between neighbours.
    let mut hovered_band = None;
    let hover = response.hover_pos();
    for (index, band) in data.bands.iter().enumerate() {
        let Some(section) = doc.section(band.section) else {
            continue;
        };
        let r = Rect::from_min_max(
            pos2(rect.left() + 1.0, y_top(band.rows.start)),
            pos2(
                rect.right() - 1.0,
                (y_top(band.rows.end) - 1.0).max(y_top(band.rows.start) + 1.0),
            ),
        );
        painter.rect_filled(r, 2.0, band_fill(section, index, layers, &visuals));
        if hover.is_some_and(|p| r.contains(p)) {
            hovered_band = Some((band, section));
            painter.rect_stroke(
                r,
                2.0,
                egui::Stroke::new(1.0, visuals.strong_text_color()),
                egui::StrokeKind::Inside,
            );
        }
    }

    // Ticks: problems on the left half, pins and matches on the right half.
    let mid = rect.center().x;
    for kind in [
        TickKind::Match,
        TickKind::Warning,
        TickKind::Error,
        TickKind::Pin,
    ] {
        for tick in data.ticks.iter().filter(|t| t.kind == kind) {
            let y = y_mid(tick.row);
            let (x0, x1) = match kind {
                TickKind::Error | TickKind::Warning => (rect.left() + 2.0, mid),
                TickKind::Pin | TickKind::Match => (mid, rect.right() - 2.0),
            };
            if kind == TickKind::Pin {
                // A small diamond, so pins read differently from problem bars.
                let c = pos2((x0 + x1) / 2.0, y);
                let s = 3.0;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        pos2(c.x, c.y - s),
                        pos2(c.x + s, c.y),
                        pos2(c.x, c.y + s),
                        pos2(c.x - s, c.y),
                    ],
                    pin_color(),
                    egui::Stroke::NONE,
                ));
            } else {
                let line = Rect::from_min_max(pos2(x0, y - 1.5), pos2(x1, y + 1.5));
                painter.rect_filled(line, 1.0, tick_color(kind, &visuals));
            }
        }
    }

    // The visible part of the list.
    let (start, end) = viewport;
    if end > start {
        let top = rect.top() + start * rect.height();
        let frame = Rect::from_min_max(
            pos2(rect.left(), top),
            pos2(
                rect.right(),
                (rect.top() + end * rect.height()).max(top + 4.0),
            ),
        );
        painter.rect_stroke(
            frame,
            2.0,
            egui::Stroke::new(1.5, visuals.strong_text_color().gamma_multiply(0.7)),
            egui::StrokeKind::Inside,
        );
    }

    let response = match hovered_band {
        Some((_, section)) => response.on_hover_text(band_tooltip(section)),
        None => response,
    };

    if (response.clicked() || response.dragged())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let frac = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let row = ((frac * total) as usize).min(total_rows.saturating_sub(1));
        let near_tick = data
            .ticks
            .iter()
            .map(|t| (t, (y_mid(t.row) - pos.y).abs()))
            .filter(|(_, d)| *d <= 4.0)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let band_start = data
            .bands
            .iter()
            .find(|b| b.rows.contains(&row))
            .map(|b| b.rows.start);
        out.go_to_row = Some(match (response.clicked(), near_tick, band_start) {
            (true, Some((tick, _)), _) => {
                out.on_tick = true;
                tick.row
            }
            (true, None, Some(start)) => start,
            _ => row,
        });
    }
    out
}

/// The legend under the list: one toggle per layer, with its color.
pub fn legend(ui: &mut Ui, layers: &mut StripLayers) {
    let visuals = ui.visuals().clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new("Timeline strip:").small().weak());
        let neutral = visuals.text_color().gamma_multiply(0.35);
        for (on, color, label, tip) in [
            (
                &mut layers.sections,
                neutral,
                "Sections",
                "Bands for Startup, compiles, play modes, …",
            ),
            (
                &mut layers.problems,
                visuals.error_fg_color,
                "Problems",
                "Errors and warnings; bands take their color",
            ),
            (&mut layers.pins, pin_color(), "Pins", "Pinned entries (P)"),
            (
                &mut layers.matches,
                match_color(),
                "Matches",
                "Search matches while the search only highlights",
            ),
        ] {
            let (swatch, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
            ui.painter().rect_filled(
                swatch,
                2.0,
                if *on {
                    color
                } else {
                    color.gamma_multiply(0.25)
                },
            );
            let text = RichText::new(label).small();
            let text = if *on {
                text
            } else {
                text.weak().strikethrough()
            };
            if ui
                .add(egui::Label::new(text).sense(Sense::click()))
                .on_hover_text(format!("{tip}\nClick to show or hide"))
                .clicked()
            {
                *on = !*on;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use comte_core::{Category, ParseEvent, SectionKind, SectionStart};

    use super::*;
    use crate::view::{DocIndex, Filter, build_rows};

    fn doc() -> LogDocument {
        let mut doc = LogDocument::new();
        let e = |sev, msg: &str, line| {
            ParseEvent::Entry(LogEntry::new(sev, Category::UserLog, msg, line..line + 1))
        };
        let start = |title: &str, line| {
            ParseEvent::SectionStart(SectionStart {
                kind: SectionKind::DomainReload,
                title: title.to_owned(),
                line,
            })
        };
        doc.apply_all([
            e(Severity::Info, "boot", 1),
            e(Severity::Warning, "careful", 2),
            start("Domain reload", 3),
            e(Severity::Error, "hidden failure", 4),
            ParseEvent::SectionEnd {
                line: 5,
                duration: None,
            },
            start("Play mode #1", 6),
            e(Severity::Exception, "boom", 7),
            e(Severity::Info, "fine", 8),
            ParseEvent::SectionEnd {
                line: 9,
                duration: None,
            },
        ]);
        doc.finish();
        doc
    }

    #[test]
    fn bands_follow_top_level_sections_and_ticks_follow_rows() {
        let doc = doc();
        let collapsed = HashSet::from([0]);
        let rows = build_rows(&doc, &DocIndex::new(&doc), &Filter::default(), &collapsed);
        // rows: boot, careful, [Domain reload] (collapsed), [Play mode #1], boom, fine
        let query = Query::compile("fine", false, false);
        let data = layout(
            &rows,
            &doc,
            StripLayers::default(),
            |e| e.message == "boom",
            |id| collapsed.contains(&id),
            Some(&query),
        );
        assert_eq!(
            data.bands,
            [
                Band {
                    section: 0,
                    rows: 2..3
                },
                Band {
                    section: 1,
                    rows: 3..6
                },
            ],
            "rows outside sections get no band"
        );
        assert_eq!(
            data.ticks,
            [
                Tick {
                    row: 1,
                    kind: TickKind::Warning
                },
                Tick {
                    row: 2,
                    kind: TickKind::Error
                },
                Tick {
                    row: 4,
                    kind: TickKind::Error
                },
                Tick {
                    row: 4,
                    kind: TickKind::Pin
                },
                Tick {
                    row: 5,
                    kind: TickKind::Match
                },
            ],
            "the collapsed section shows the error it hides"
        );

        let play = Query::compile("play", false, false);
        let titled = layout(
            &rows,
            &doc,
            StripLayers::default(),
            |_| false,
            |_| false,
            Some(&play),
        );
        assert!(titled.ticks.contains(&Tick {
            row: 3,
            kind: TickKind::Match
        }));
    }

    #[test]
    fn layers_can_be_switched_off() {
        let doc = doc();
        let rows = build_rows(
            &doc,
            &DocIndex::new(&doc),
            &Filter::default(),
            &HashSet::new(),
        );
        let only_pins = StripLayers {
            sections: false,
            problems: false,
            pins: true,
            matches: false,
        };
        let data = layout(
            &rows,
            &doc,
            only_pins,
            |e| e.message == "careful",
            |_| false,
            None,
        );
        assert!(data.bands.is_empty());
        assert_eq!(
            data.ticks,
            [Tick {
                row: 1,
                kind: TickKind::Pin
            }]
        );
    }
}
