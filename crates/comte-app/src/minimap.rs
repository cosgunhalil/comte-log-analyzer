//! The minimap: a thin strip beside the entry list showing where errors,
//! warnings, bookmarks and search matches are, and which part of the list is
//! on screen. Click or drag to jump; right-click to choose what it shows.
//!
//! Marks are placed by *row* position, so they line up with the list's
//! scrollbar whatever is filtered or collapsed.

use comte_core::{LogDocument, LogEntry, Severity};
use eframe::egui::{self, Color32, Rect, Sense, Ui, pos2};
use serde::{Deserialize, Serialize};

use crate::search::Query;
use crate::view::Row;

/// Which kinds of marks the minimap shows (saved in settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MinimapKinds {
    pub errors: bool,
    pub warnings: bool,
    pub bookmarks: bool,
    pub matches: bool,
}

impl Default for MinimapKinds {
    fn default() -> Self {
        Self {
            errors: true,
            warnings: true,
            bookmarks: true,
            matches: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    Error,
    Warning,
    Bookmark,
    Match,
}

/// One mark: a row of the list and what it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub row: usize,
    pub kind: MarkKind,
}

/// Marks for the current rows. `matches` is the search query when it only
/// highlights (when it filters, every row is a match, so it adds nothing).
/// A collapsed section header gets the worst mark of what it hides.
pub fn marks(
    rows: &[Row],
    doc: &LogDocument,
    kinds: MinimapKinds,
    is_bookmarked: impl Fn(&LogEntry) -> bool,
    is_collapsed: impl Fn(u32) -> bool,
    matches: Option<&Query>,
) -> Vec<Mark> {
    let mut out = Vec::new();
    for (row, item) in rows.iter().enumerate() {
        match *item {
            Row::Entry { id, .. } => {
                let Some(entry) = doc.entry(id) else { continue };
                if kinds.errors && entry.severity.is_error_like() {
                    out.push(Mark {
                        row,
                        kind: MarkKind::Error,
                    });
                } else if kinds.warnings && entry.severity == Severity::Warning {
                    out.push(Mark {
                        row,
                        kind: MarkKind::Warning,
                    });
                }
                if kinds.bookmarks && is_bookmarked(entry) {
                    out.push(Mark {
                        row,
                        kind: MarkKind::Bookmark,
                    });
                }
                if kinds.matches && matches.is_some_and(|q| !q.is_empty() && q.matches_entry(entry))
                {
                    out.push(Mark {
                        row,
                        kind: MarkKind::Match,
                    });
                }
            }
            Row::Section { id, .. } => {
                let Some(section) = doc.section(id) else {
                    continue;
                };
                if is_collapsed(id) {
                    if kinds.errors && section.counts.errors() > 0 {
                        out.push(Mark {
                            row,
                            kind: MarkKind::Error,
                        });
                    } else if kinds.warnings && section.counts.warning > 0 {
                        out.push(Mark {
                            row,
                            kind: MarkKind::Warning,
                        });
                    }
                }
                if kinds.matches && matches.is_some_and(|q| !q.ranges(&section.title).is_empty()) {
                    out.push(Mark {
                        row,
                        kind: MarkKind::Match,
                    });
                }
            }
        }
    }
    out
}

/// What the user did with the minimap this frame.
#[derive(Debug, Default)]
pub struct MinimapResponse {
    /// Scroll to (and, for a mark, select) this row.
    pub go_to_row: Option<usize>,
    /// The click landed on a mark (select its entry, not just scroll).
    pub on_mark: bool,
}

/// The bookmark star's color, in the list and the minimap.
pub fn bookmark_color() -> Color32 {
    Color32::from_rgb(0x3d, 0x8b, 0xfd)
}

pub fn color(kind: MarkKind, visuals: &egui::Visuals) -> Color32 {
    match kind {
        MarkKind::Error => visuals.error_fg_color,
        MarkKind::Warning => visuals.warn_fg_color,
        MarkKind::Bookmark => bookmark_color(),
        MarkKind::Match => Color32::from_rgb(0xf5, 0xc5, 0x18),
    }
}

/// Draws the minimap in `rect`. `viewport` is the visible part of the list
/// as fractions (start, end) of its height.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    marks: &[Mark],
    total_rows: usize,
    viewport: (f32, f32),
    kinds: &mut MinimapKinds,
) -> MinimapResponse {
    let mut out = MinimapResponse::default();
    let response = ui
        .interact(rect, egui::Id::unique("comte-minimap"), Sense::click_and_drag())
        .on_hover_text("Errors, warnings, bookmarks and search matches.\nClick or drag to jump; right-click to choose.");
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, visuals.faint_bg_color);

    let total = total_rows.max(1) as f32;
    let y_of = |row: usize| rect.top() + (row as f32 + 0.5) / total * rect.height();
    let mid = rect.center().x;
    // Severity on the left half, bookmarks and matches on the right half;
    // drawn in order so bookmarks end up on top.
    for kind in [
        MarkKind::Match,
        MarkKind::Warning,
        MarkKind::Error,
        MarkKind::Bookmark,
    ] {
        for mark in marks.iter().filter(|m| m.kind == kind) {
            let y = y_of(mark.row);
            let (x0, x1) = match kind {
                MarkKind::Error | MarkKind::Warning => (rect.left() + 1.0, mid),
                MarkKind::Bookmark | MarkKind::Match => (mid, rect.right() - 1.0),
            };
            let line = Rect::from_min_max(pos2(x0, y - 1.5), pos2(x1, y + 1.5));
            painter.rect_filled(line, 0.0, color(kind, &visuals));
        }
    }

    // The visible part of the list.
    let (start, end) = viewport;
    if end > start {
        let frame = Rect::from_min_max(
            pos2(rect.left(), rect.top() + start * rect.height()),
            pos2(
                rect.right(),
                (rect.top() + end * rect.height()).max(rect.top() + start * rect.height() + 4.0),
            ),
        );
        painter.rect_filled(frame, 2.0, visuals.text_color().gamma_multiply(0.08));
        painter.rect_stroke(
            frame,
            2.0,
            egui::Stroke::new(1.0, visuals.weak_text_color()),
            egui::StrokeKind::Inside,
        );
    }

    if (response.clicked() || response.dragged())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let frac = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let row = ((frac * total) as usize).min(total_rows.saturating_sub(1));
        // A click near a mark goes to that mark's row.
        let near = marks
            .iter()
            .map(|m| (m, (y_of(m.row) - pos.y).abs()))
            .filter(|(_, d)| *d <= 4.0)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match near {
            Some((mark, _)) if response.clicked() => {
                out.go_to_row = Some(mark.row);
                out.on_mark = true;
            }
            _ => out.go_to_row = Some(row),
        }
    }

    response.context_menu(|ui| {
        ui.label(egui::RichText::new("Show in the minimap").strong());
        ui.checkbox(&mut kinds.errors, "Errors");
        ui.checkbox(&mut kinds.warnings, "Warnings");
        ui.checkbox(&mut kinds.bookmarks, "Bookmarks");
        ui.checkbox(&mut kinds.matches, "Search matches (highlight-only search)");
    });
    out
}

#[cfg(test)]
mod tests {
    use comte_core::{Category, ParseEvent, SectionKind, SectionStart};

    use super::*;
    use crate::view::{DocIndex, Filter, build_rows};
    use std::collections::HashSet;

    fn doc() -> LogDocument {
        let mut doc = LogDocument::new();
        let e = |sev, msg: &str, line| {
            ParseEvent::Entry(LogEntry::new(sev, Category::UserLog, msg, line..line + 1))
        };
        doc.apply_all([
            e(Severity::Info, "boot", 1),
            e(Severity::Warning, "careful", 2),
            ParseEvent::SectionStart(SectionStart {
                kind: SectionKind::DomainReload,
                title: "Domain reload".to_owned(),
                line: 3,
            }),
            e(Severity::Error, "hidden failure", 4),
            ParseEvent::SectionEnd {
                line: 5,
                duration: None,
            },
            e(Severity::Exception, "boom", 6),
        ]);
        doc.finish();
        doc
    }

    #[test]
    fn marks_follow_rows_and_collapsed_sections() {
        let doc = doc();
        let collapsed = HashSet::from([0]);
        let rows = build_rows(&doc, &DocIndex::new(&doc), &Filter::default(), &collapsed);
        // rows: boot, careful, [Domain reload] (collapsed), boom
        let query = Query::compile("boot", false, false);
        let reload = Query::compile("reload", false, false);
        let section_match = super::marks(
            &rows,
            &doc,
            MinimapKinds::default(),
            |_| false,
            |_| false,
            Some(&reload),
        );
        assert!(
            section_match.contains(&Mark {
                row: 2,
                kind: MarkKind::Match
            }),
            "section titles match too"
        );
        let marks = marks(
            &rows,
            &doc,
            MinimapKinds::default(),
            |e| e.message == "boom",
            |id| collapsed.contains(&id),
            Some(&query),
        );
        assert_eq!(
            marks,
            [
                Mark {
                    row: 0,
                    kind: MarkKind::Match
                },
                Mark {
                    row: 1,
                    kind: MarkKind::Warning
                },
                Mark {
                    row: 2,
                    kind: MarkKind::Error
                },
                Mark {
                    row: 3,
                    kind: MarkKind::Error
                },
                Mark {
                    row: 3,
                    kind: MarkKind::Bookmark
                },
            ],
            "the collapsed section shows the error it hides"
        );
    }

    #[test]
    fn kinds_can_be_switched_off() {
        let doc = doc();
        let rows = build_rows(
            &doc,
            &DocIndex::new(&doc),
            &Filter::default(),
            &HashSet::new(),
        );
        let only_bookmarks = MinimapKinds {
            errors: false,
            warnings: false,
            bookmarks: true,
            matches: false,
        };
        let marks = marks(
            &rows,
            &doc,
            only_bookmarks,
            |e| e.message == "careful",
            |_| false,
            None,
        );
        assert_eq!(
            marks,
            [Mark {
                row: 1,
                kind: MarkKind::Bookmark
            }]
        );
    }
}
