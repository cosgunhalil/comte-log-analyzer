//! The right panel when nothing is selected (DESIGN §5): the verdict, the
//! most repeated problems, the slowest operations, timings, facts and the
//! timeline. Every item can be clicked to jump to it.

use std::collections::HashSet;
use std::time::Duration;

use comte_core::{
    Insights, Repeated, SessionContent, SessionHeader, Severity, VerdictKind, format_duration,
};
use eframe::egui::{self, Color32, RichText, Sense, Ui, vec2};

use crate::loader::Loaded;
use crate::theme::{severity_color, severity_icon};

/// Where the user asked to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jump {
    Entry(u64),
    Section(u32),
    /// Copy the plain-text summary (verdict, counts, environment).
    CopySummary,
}

pub fn overview(ui: &mut Ui, current: &Loaded) -> Option<Jump> {
    let mut jump = None;
    if let Some(session) = &current.session {
        session_card(ui, session);
    }
    ui.label(RichText::new(&current.title).size(17.0).strong());
    ui.label(
        RichText::new(format!(
            "{} · {}",
            current.kind_label,
            current.path.display()
        ))
        .weak()
        .small(),
    );
    ui.add_space(8.0);

    let insights = &current.insights;
    jump = jump.or(verdict_banner(ui, insights));

    if !insights.repeated_problems.is_empty() {
        heading(ui, "Most repeated problems");
        jump = jump.or(repeated_list(ui, &insights.repeated_problems));
    }

    if !insights.slowest.is_empty() {
        heading(ui, "Slowest operations");
        jump = jump.or(slowest_bars(ui, insights));
    }

    if !insights.timings.is_empty() {
        heading(ui, "Timings");
        egui::Grid::new("timings")
            .num_columns(5)
            .spacing([14.0, 3.0])
            .striped(true)
            .show(ui, |ui| {
                for label in ["", "count", "total", "average", "slowest"] {
                    ui.label(RichText::new(label).weak().small());
                }
                ui.end_row();
                for t in &insights.timings {
                    ui.label(t.kind.label());
                    ui.label(t.count.to_string());
                    ui.label(format_duration(t.total));
                    ui.label(format_duration(t.average()));
                    ui.label(format_duration(t.max));
                    ui.end_row();
                }
            });
    }

    if !insights.repeated_messages.is_empty() {
        ui.add_space(10.0);
        egui::CollapsingHeader::new(RichText::new("Most repeated messages").strong())
            .default_open(insights.repeated_problems.is_empty())
            .show(ui, |ui| {
                jump = jump.or(repeated_list(ui, &insights.repeated_messages));
            });
    }

    let facts = current.doc.facts();
    if !facts.is_empty() {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Environment").strong());
            if ui
                .small_button("📋 Copy summary")
                .on_hover_text("Verdict, problem counts and environment as plain text for a bug report (redacted)")
                .clicked()
            {
                jump = Some(Jump::CopySummary);
            }
        });
        ui.add_space(2.0);
        egui::Grid::new("facts")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                let mut seen = HashSet::new();
                for fact in facts {
                    if seen.insert(fact.key.clone()) {
                        ui.label(RichText::new(fact.key.label()).weak());
                        ui.label(&fact.value);
                        ui.end_row();
                    }
                }
            });
    }

    let roots: Vec<_> = current.doc.root_sections().collect();
    if !roots.is_empty() {
        heading(ui, "Timeline");
        for section in roots {
            let duration = section
                .duration
                .map(|d| format!("  {}", format_duration(d)))
                .unwrap_or_default();
            let problems = section.counts.errors() + section.counts.warning;
            let marker = if problems > 0 {
                format!(
                    "  ({problems} {})",
                    if problems == 1 { "problem" } else { "problems" }
                )
            } else {
                String::new()
            };
            if ui
                .add(egui::Button::new(format!("{}{duration}{marker}", section.title)).frame(false))
                .on_hover_text("Show this section in the list")
                .clicked()
            {
                jump = Some(Jump::Section(section.id));
            }
        }
    }

    ui.add_space(10.0);
    ui.label(
        RichText::new(
            "Click an item to jump to it. N goes to the next problem, ] to the next section.",
        )
        .weak()
        .small(),
    );
    jump
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(12.0);
    ui.label(RichText::new(text).strong());
    ui.add_space(2.0);
}

/// Background tint for the verdict banner.
fn tint(color: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 28)
}

fn verdict_banner(ui: &mut Ui, insights: &Insights) -> Option<Jump> {
    let verdict = &insights.verdict;
    let visuals = ui.visuals().clone();
    let (icon, color) = match verdict.kind {
        VerdictKind::Crashed => ("💥", severity_color(&visuals, Severity::Crash)),
        VerdictKind::Errors => ("⛔", visuals.error_fg_color),
        VerdictKind::Warnings => ("⚠", visuals.warn_fg_color),
        VerdictKind::Clean => ("✔", Color32::from_rgb(0x3c, 0xb3, 0x71)),
    };
    let mut jump = None;
    egui::Frame::new()
        .fill(tint(color))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(20.0).color(color));
                ui.label(
                    RichText::new(&verdict.headline)
                        .size(16.0)
                        .strong()
                        .color(color),
                );
            });
            for detail in &verdict.details {
                ui.label(detail);
            }
            if let Some(id) = verdict.entry {
                ui.add_space(4.0);
                if ui.button("Show it →").clicked() {
                    jump = Some(Jump::Entry(id));
                }
            }
        });
    jump
}

fn repeated_list(ui: &mut Ui, items: &[Repeated]) -> Option<Jump> {
    let mut jump = None;
    let visuals = ui.visuals().clone();
    for item in items {
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), row_height), Sense::click());
        let painter = ui.painter_at(rect);
        if response.hovered() {
            painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let y = rect.center().y;
        let count = painter.text(
            egui::pos2(rect.left() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            format!("×{}", item.count),
            egui::FontId::proportional(13.0),
            visuals.strong_text_color(),
        );
        let icon = painter.text(
            egui::pos2(count.right() + 8.0, y),
            egui::Align2::LEFT_CENTER,
            severity_icon(item.severity),
            egui::FontId::proportional(13.0),
            severity_color(&visuals, item.severity),
        );
        let mut right = rect.right() - 4.0;
        if let Some(location) = &item.location {
            let r = painter.text(
                egui::pos2(right, y),
                egui::Align2::RIGHT_CENTER,
                location,
                egui::FontId::proportional(12.0),
                visuals.weak_text_color(),
            );
            right = r.left() - 8.0;
        }
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(icon.right() + 6.0, rect.top()),
            egui::pos2(right.max(icon.right() + 40.0), rect.bottom()),
        );
        painter.with_clip_rect(text_rect).text(
            egui::pos2(text_rect.left(), y),
            egui::Align2::LEFT_CENTER,
            &item.headline,
            egui::FontId::proportional(13.0),
            visuals.text_color(),
        );
        if response.on_hover_text(&item.headline).clicked() {
            jump = Some(Jump::Entry(item.entry));
        }
    }
    jump
}

fn slowest_bars(ui: &mut Ui, insights: &Insights) -> Option<Jump> {
    let mut jump = None;
    let visuals = ui.visuals().clone();
    let longest = insights
        .slowest
        .first()
        .map_or(Duration::from_secs(1), |t| t.duration)
        .max(Duration::from_millis(1));
    for item in &insights.slowest {
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), row_height), Sense::click());
        let painter = ui.painter_at(rect);
        if response.hovered() {
            painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
        }
        // Label | bar | duration
        let label_width = 130.0;
        let value_width = 64.0;
        let y = rect.center().y;
        painter.text(
            egui::pos2(rect.left() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            item.kind.label(),
            egui::FontId::proportional(13.0),
            visuals.text_color(),
        );
        let bar_left = rect.left() + label_width;
        let bar_max = (rect.width() - label_width - value_width).max(20.0);
        let fraction = item.duration.as_secs_f32() / longest.as_secs_f32();
        let bar = egui::Rect::from_min_size(
            egui::pos2(bar_left, y - 5.0),
            vec2((bar_max * fraction).max(2.0), 10.0),
        );
        painter.rect_filled(bar, 3.0, visuals.selection.bg_fill);
        painter.text(
            egui::pos2(rect.right() - 4.0, y),
            egui::Align2::RIGHT_CENTER,
            format_duration(item.duration),
            egui::FontId::proportional(12.5),
            visuals.strong_text_color(),
        );
        let response = response.on_hover_text(format!("Line {}", item.line));
        if response.clicked()
            && let Some(id) = item.entry
        {
            jump = Some(Jump::Entry(id));
        }
    }
    jump
}

/// Note, tags and origin of a reopened saved session.
pub fn session_card(ui: &mut Ui, session: &SessionHeader) {
    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("📝 Saved session").strong());
            if !session.tags.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for tag in &session.tags {
                        egui::Frame::new()
                            .fill(ui.visuals().selection.bg_fill)
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(8, 2))
                            .show(ui, |ui| {
                                ui.label(RichText::new(tag).small().strong());
                            });
                    }
                });
            }
            if !session.note.is_empty() {
                ui.add_space(4.0);
                ui.label(&session.note);
            }
            ui.add_space(4.0);
            let content = match &session.content {
                SessionContent::Full => "whole log".to_owned(),
                SessionContent::Filtered(d) => format!("filtered: {d}"),
            };
            let redacted = match session.redacted {
                Some(n) => format!("redacted ({n} values)"),
                None => "not redacted".to_owned(),
            };
            ui.label(
                RichText::new(format!(
                    "From {} · saved {}\n{content} · {redacted}",
                    session.source, session.saved_at
                ))
                .weak()
                .small(),
            );
        });
    ui.add_space(10.0);
}
