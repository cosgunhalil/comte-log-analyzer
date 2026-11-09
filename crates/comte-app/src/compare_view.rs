//! The comparison report shown in the middle panel (DESIGN §7).

use std::path::{Path, PathBuf};

use comte_core::{Comparison, LogSource, ProblemDiff, Severity, SourceRole, format_duration};
use eframe::egui::{self, Color32, RichText, Sense, Ui, vec2};

use crate::loader::{LoadEvent, Loaded, Loader};
use crate::save::SessionFile;
use crate::theme::{severity_color, severity_icon};

/// What the user did in the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareAction {
    /// Show this entry of log B (the current log).
    ShowInB(u64),
    /// Open log A and show this entry.
    ShowInA(u64),
    PickA {
        path: PathBuf,
        title: String,
    },
    BrowseA,
    Swap,
    Close,
}

/// Comparison state: log A ("before"); log B is the open log.
pub struct CompareView {
    pub a: Option<Box<Loaded>>,
    loader: Loader,
    pub a_path: PathBuf,
    pub a_title: String,
    pub result: Option<Comparison>,
    pub error: Option<String>,
}

impl CompareView {
    pub fn start(path: PathBuf, title: String, ctx: &egui::Context) -> Self {
        let mut view = Self {
            a: None,
            loader: Loader::default(),
            a_path: path.clone(),
            a_title: title.clone(),
            result: None,
            error: None,
        };
        view.loader.start(path, title, ctx);
        view
    }

    /// Starts with an already loaded A (e.g. after swapping).
    pub fn with_loaded(a: Box<Loaded>) -> Self {
        Self {
            a_path: a.path.clone(),
            a_title: a.title.clone(),
            a: Some(a),
            loader: Loader::default(),
            result: None,
            error: None,
        }
    }

    pub fn pick(&mut self, path: PathBuf, title: String, ctx: &egui::Context) {
        self.a = None;
        self.result = None;
        self.error = None;
        self.a_path = path.clone();
        self.a_title = title.clone();
        self.loader.start(path, title, ctx);
    }

    /// Takes a finished load of A; recomputes against `b` when needed.
    pub fn update(&mut self, b: &Loaded) {
        match self.loader.poll() {
            Some(LoadEvent::Done(a)) => {
                self.a = Some(a);
                self.result = None;
            }
            Some(LoadEvent::Failed { error, .. }) => self.error = Some(error),
            None => {}
        }
        if self.result.is_none()
            && let Some(a) = &self.a
        {
            self.result = Some(Comparison::of(&a.doc, &b.doc));
        }
    }

    /// The log B changed (another file, live update): compare again.
    pub fn invalidate(&mut self) {
        self.result = None;
    }

    pub fn loading(&self) -> bool {
        self.loader.pending().is_some()
    }
}

/// The previous run's log next to `path`: `Editor.log` → `Editor-prev.log`,
/// `Player.log` → `Player-prev.log`.
pub fn previous_run(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let prev = match name {
        "Editor.log" => "Editor-prev.log",
        "Player.log" => "Player-prev.log",
        _ => return None,
    };
    let candidate = path.with_file_name(prev);
    candidate.is_file().then_some(candidate)
}

pub fn compare_ui(
    ui: &mut Ui,
    view: &CompareView,
    b: &Loaded,
    sources: &[LogSource],
    sessions: &[SessionFile],
) -> Option<CompareAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("⇄ Compare").strong().size(15.0));
        ui.separator();
        ui.label(RichText::new("A (before)").weak());
        egui::ComboBox::from_id_salt("compare-a")
            .selected_text(&view.a_title)
            .width(260.0)
            .show_ui(ui, |ui| {
                for source in sources.iter().filter(|s| s.exists() && s.path != b.path) {
                    let label = match source.role {
                        SourceRole::Previous => format!("{} (previous)", source.title),
                        _ => source.title.clone(),
                    };
                    if ui
                        .selectable_label(source.path == view.a_path, label)
                        .clicked()
                    {
                        action = Some(CompareAction::PickA {
                            path: source.path.clone(),
                            title: source.title.clone(),
                        });
                    }
                }
                for session in sessions.iter().filter(|s| s.path != b.path) {
                    if ui
                        .selectable_label(
                            session.path == view.a_path,
                            format!("📝 {}", session.title),
                        )
                        .clicked()
                    {
                        action = Some(CompareAction::PickA {
                            path: session.path.clone(),
                            title: session.title.clone(),
                        });
                    }
                }
                ui.separator();
                if ui.selectable_label(false, "📂 Browse…").clicked() {
                    action = Some(CompareAction::BrowseA);
                }
            });
        ui.label("→");
        ui.label(RichText::new("B (after)").weak());
        ui.label(RichText::new(&b.title).strong());
        if ui
            .small_button("⇅ Swap")
            .on_hover_text("Open A in the viewer and compare the other way round")
            .clicked()
        {
            action = Some(CompareAction::Swap);
        }
        if ui
            .small_button("✖ Close")
            .on_hover_text("Back to the log (Esc)")
            .clicked()
        {
            action = Some(CompareAction::Close);
        }
    });
    ui.separator();

    if let Some(error) = &view.error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Could not open A: {error}"),
        );
        return action;
    }
    let Some(result) = &view.result else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("Reading {}…", view.a_title));
        });
        return action;
    };

    egui::ScrollArea::vertical()
        .id_salt("compare")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            summary(ui, result);
            if result.is_identical() {
                ui.add_space(8.0);
                ui.label(RichText::new("✔ The same warnings and errors, just as often.").strong());
            }

            let visuals = ui.visuals().clone();
            section(
                ui,
                &format!("New in B ({})", result.new.len()),
                visuals.error_fg_color,
                "Problems that did not happen in A.",
            );
            for diff in &result.new {
                if diff_row(ui, diff, DiffSide::New).clicked() {
                    action = diff.entry_b.map(CompareAction::ShowInB);
                }
            }
            section(
                ui,
                &format!("Gone since A ({})", result.gone.len()),
                Color32::from_rgb(0x3c, 0xb3, 0x71),
                "Problems from A that did not happen in B. Click to open A.",
            );
            for diff in &result.gone {
                if diff_row(ui, diff, DiffSide::Gone).clicked() {
                    action = diff.entry_a.map(CompareAction::ShowInA);
                }
            }
            section(
                ui,
                &format!("In both ({})", result.common.len()),
                visuals.text_color(),
                "Problems in both logs, with how often each happened.",
            );
            for diff in &result.common {
                if diff_row(ui, diff, DiffSide::Both).clicked() {
                    action = diff.entry_b.map(CompareAction::ShowInB);
                }
            }

            if !result.timings.is_empty() {
                section(
                    ui,
                    "Timings (average)",
                    visuals.text_color(),
                    "Red: more than 25 % and 100 ms slower. Green: as much faster.",
                );
                egui::Grid::new("compare-timings")
                    .num_columns(5)
                    .striped(true)
                    .spacing([18.0, 4.0])
                    .show(ui, |ui| {
                        for label in ["", "A", "B", "change", "count A → B"] {
                            ui.label(RichText::new(label).weak().small());
                        }
                        ui.end_row();
                        for t in &result.timings {
                            ui.label(t.kind.label());
                            ui.label(
                                t.a.as_ref()
                                    .map_or("—".to_owned(), |a| format_duration(a.average())),
                            );
                            ui.label(
                                t.b.as_ref()
                                    .map_or("—".to_owned(), |b| format_duration(b.average())),
                            );
                            let change = t
                                .change_percent()
                                .map_or("—".to_owned(), comte_core::compare::format_percent);
                            let color = if t.is_regression() {
                                visuals.error_fg_color
                            } else if t.is_improvement() {
                                Color32::from_rgb(0x3c, 0xb3, 0x71)
                            } else {
                                visuals.weak_text_color()
                            };
                            ui.label(RichText::new(change).color(color).strong());
                            ui.label(format!(
                                "{} → {}",
                                t.a.as_ref().map_or(0, |a| a.count),
                                t.b.as_ref().map_or(0, |b| b.count)
                            ));
                            ui.end_row();
                        }
                    });
            }

            if !result.facts.is_empty() {
                section(
                    ui,
                    "Environment changes",
                    visuals.warn_fg_color,
                    "Facts that differ between the two logs.",
                );
                egui::Grid::new("compare-facts")
                    .num_columns(2)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for fact in &result.facts {
                            ui.label(RichText::new(fact.key.label()).weak());
                            ui.label(format!(
                                "{}  →  {}",
                                fact.a.as_deref().unwrap_or("—"),
                                fact.b.as_deref().unwrap_or("—")
                            ));
                            ui.end_row();
                        }
                    });
            }
        });
    action
}

fn summary(ui: &mut Ui, result: &Comparison) {
    let visuals = ui.visuals().clone();
    ui.horizontal_wrapped(|ui| {
        for (severity, a, b, name) in [
            (
                Severity::Error,
                result.counts_a.errors(),
                result.counts_b.errors(),
                "errors",
            ),
            (
                Severity::Warning,
                result.counts_a.warning,
                result.counts_b.warning,
                "warnings",
            ),
        ] {
            let delta = i64::from(b) - i64::from(a);
            let delta_text = match delta {
                0 => "no change".to_owned(),
                d => format!("{d:+}"),
            };
            let delta_color = match delta {
                d if d > 0 => visuals.error_fg_color,
                d if d < 0 => Color32::from_rgb(0x3c, 0xb3, 0x71),
                _ => visuals.weak_text_color(),
            };
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(
                    RichText::new(format!("{} {name}  {a} → {b}", severity_icon(severity)))
                        .color(severity_color(&visuals, severity))
                        .strong(),
                );
                ui.label(RichText::new(delta_text).color(delta_color));
            });
        }
    });
}

fn section(ui: &mut Ui, title: &str, color: Color32, hint: &str) {
    ui.add_space(12.0);
    ui.label(RichText::new(title).strong().size(14.0).color(color))
        .on_hover_text(hint);
    ui.add_space(2.0);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffSide {
    New,
    Gone,
    Both,
}

fn diff_row(ui: &mut Ui, diff: &ProblemDiff, side: DiffSide) -> egui::Response {
    let visuals = ui.visuals().clone();
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 8.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), row_height), Sense::click());
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let y = rect.center().y;
    let counts = match side {
        DiffSide::New => format!("×{}", diff.count_b),
        DiffSide::Gone => format!("×{}", diff.count_a),
        DiffSide::Both => format!("×{} → ×{}", diff.count_a, diff.count_b),
    };
    let grew = diff.count_b > diff.count_a && side == DiffSide::Both;
    let counts_rect = painter.text(
        egui::pos2(rect.left() + 6.0, y),
        egui::Align2::LEFT_CENTER,
        counts,
        egui::FontId::proportional(12.5),
        if grew {
            visuals.error_fg_color
        } else {
            visuals.strong_text_color()
        },
    );
    let icon_x = (rect.left() + 86.0).max(counts_rect.right() + 8.0);
    painter.text(
        egui::pos2(icon_x, y),
        egui::Align2::LEFT_CENTER,
        severity_icon(diff.severity),
        egui::FontId::proportional(13.0),
        severity_color(&visuals, diff.severity),
    );
    let mut right = rect.right() - 6.0;
    if let Some(location) = &diff.location {
        let r = painter.text(
            egui::pos2(right, y),
            egui::Align2::RIGHT_CENTER,
            location,
            egui::FontId::proportional(12.0),
            visuals.weak_text_color(),
        );
        right = r.left() - 10.0;
    }
    let text_color = if side == DiffSide::Gone {
        visuals.weak_text_color()
    } else {
        visuals.text_color()
    };
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(icon_x + 22.0, rect.top()),
        egui::pos2(right.max(icon_x + 80.0), rect.bottom()),
    );
    painter.with_clip_rect(text_rect).text(
        egui::pos2(text_rect.left(), y),
        egui::Align2::LEFT_CENTER,
        &diff.headline,
        egui::FontId::proportional(13.5),
        text_color,
    );
    let tip = match side {
        DiffSide::Gone => "Open log A at this entry",
        _ => "Show in the log",
    };
    response.on_hover_text(tip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_run_sibling() {
        let dir = std::env::temp_dir().join(format!("comte-prev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Editor.log"), "x").unwrap();
        assert_eq!(
            previous_run(&dir.join("Editor.log")),
            None,
            "no prev file yet"
        );
        std::fs::write(dir.join("Editor-prev.log"), "x").unwrap();
        assert_eq!(
            previous_run(&dir.join("Editor.log")),
            Some(dir.join("Editor-prev.log"))
        );
        assert_eq!(previous_run(&dir.join("custom.log")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
