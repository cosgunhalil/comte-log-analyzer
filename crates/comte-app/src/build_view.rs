//! The build view (DESIGN §5.1, "Builds"): one player build from the log, shown
//! like everything else in the app: grouped, sorted and searchable.
//!
//! Which build it is (target, date, result, duration), Unity's size per
//! category, the build's steps, and every asset in the size report, as a list
//! or grouped by file type or folder. A log can hold several builds; the
//! picker at the top switches between them.
//!
//! Builds can be saved to the build history (DESIGN §5.2) and compared with a
//! saved build or another build in the log: what grew, what shrank, what is
//! new and what is gone, headed by "Your build grew by X".

use std::path::PathBuf;
use std::time::Duration;

use comte_core::{
    BuildComparison, BuildReport, SizeChange, SizeRow, format_bytes, format_delta, format_duration,
};
use eframe::egui::{self, Color32, RichText, Sense, Ui, vec2};

use crate::build_history::{SavedBuild, find_saved, project_name};
use crate::loader::Loaded;

/// How the asset list is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grouping {
    /// Every asset, one row each.
    Assets,
    /// Totals per kind of file (Textures, Audio, …).
    ByType,
    /// Totals per top folder (`Assets/Art`, `Packages/com.unity.ugui`).
    ByFolder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Largest,
    Smallest,
    Path,
}

/// The build the shown one is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Against {
    /// Another build in the same log (index into `doc.build_reports()`).
    InLog(usize),
    /// A build from the history.
    Saved(PathBuf),
}

/// Which changes the comparison lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeList {
    /// The changes that go the way the build went.
    Responsible,
    Grew,
    Shrank,
    Added,
    Removed,
}

/// State of the build view.
#[derive(Debug, Clone)]
pub struct BuildView {
    /// Which build of `doc.build_reports()`.
    pub index: usize,
    pub search: String,
    pub grouping: Grouping,
    pub order: Order,
    /// Only the assets of this group (type or folder), after clicking it.
    pub group: Option<(Grouping, String)>,
    pub against: Option<Against>,
    pub changes: ChangeList,
    /// The comparison of `index` with `against`, computed once.
    comparison: Option<(usize, Against, BuildComparison)>,
    /// Note for the next save to the history.
    pub note: String,
}

impl BuildView {
    /// Opens build `index`, compared with the most recent saved build of the
    /// same project (or the previous build in the log) when there is one.
    pub fn new(index: usize, current: &Loaded, history: &[SavedBuild]) -> Self {
        let project = project_name(&current.doc);
        Self {
            index,
            search: String::new(),
            grouping: Grouping::Assets,
            order: Order::Largest,
            group: None,
            against: default_against(
                index,
                current.doc.build_reports(),
                history,
                project.as_deref(),
            ),
            changes: ChangeList::Responsible,
            comparison: None,
            note: String::new(),
        }
    }

    /// Shows build `index`, keeping the comparison choice when it still fits.
    fn show_build(&mut self, index: usize, builds: &[BuildReport], history: &[SavedBuild]) {
        self.index = index;
        self.group = None;
        if self.against == Some(Against::InLog(index)) {
            self.against = default_against(index, builds, history, None);
        }
    }
}

/// What to compare build `index` with by default: the build of the same
/// project saved before it (the newest saved one when it isn't saved yet:
/// it's the build just made), else the last earlier build in the log that
/// has sizes.
pub fn default_against(
    index: usize,
    builds: &[BuildReport],
    history: &[SavedBuild],
    project: Option<&str>,
) -> Option<Against> {
    let current = builds.get(index)?;
    if !current.has_sizes() {
        return None;
    }
    let identity = current.identity();
    // History is newest first: look only at builds saved before this one.
    let older = history
        .iter()
        .position(|s| s.build.identity() == identity)
        .map_or(0, |i| i + 1);
    history[older..]
        .iter()
        .find(|s| {
            s.build.has_sizes()
                && s.build.identity() != identity
                && (project.is_none() || s.project.as_deref() == project)
        })
        .map(|s| Against::Saved(s.path.clone()))
        .or_else(|| {
            builds[..index]
                .iter()
                .rposition(BuildReport::has_sizes)
                .map(Against::InLog)
        })
}

/// The build `against` points at, and how to name it.
fn against_build<'a>(
    against: &Against,
    builds: &'a [BuildReport],
    history: &'a [SavedBuild],
) -> Option<(String, &'a BuildReport)> {
    match against {
        Against::InLog(i) => builds
            .get(*i)
            .map(|b| (format!("{} (this log)", b.label()), b)),
        Against::Saved(path) => history
            .iter()
            .find(|s| &s.path == path)
            .map(|s| (s.label(), &s.build)),
    }
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildAction {
    Close,
    /// Go to this line in the log.
    ShowInLog(usize),
    /// Save build `index` to the history with this note.
    Save {
        index: usize,
        note: String,
    },
    /// Copy the comparison of build `after` with `before` as Markdown
    /// (redacted by the app).
    CopyMarkdown {
        before: Against,
        after: usize,
    },
    /// Open a saved build in the viewer.
    OpenSaved(PathBuf),
    /// Show the history folder in the file manager.
    OpenHistoryFolder,
}

/// The kind of file, from its extension.
pub fn asset_kind(path: &str) -> &'static str {
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "tga" | "psd" | "tif" | "tiff" | "exr" | "hdr" | "bmp" | "gif"
        | "spriteatlas" | "spriteatlasv2" | "rendertexture" | "cubemap" => "Textures",
        "wav" | "mp3" | "ogg" | "aif" | "aiff" | "flac" | "mixer" => "Audio",
        "mp4" | "mov" | "webm" | "avi" => "Video",
        "fbx" | "obj" | "blend" | "dae" | "3ds" | "max" | "mesh" => "Models",
        "anim" | "controller" | "overridecontroller" | "playable" => "Animations",
        "mat" | "physicmaterial" | "physicsmaterial2d" => "Materials",
        "shader" | "shadergraph" | "shadervariants" | "compute" | "cginc" | "hlsl" => "Shaders",
        "prefab" => "Prefabs",
        "unity" => "Scenes",
        "ttf" | "otf" | "fontsettings" => "Fonts",
        "cs" | "dll" | "asmdef" => "Scripts",
        "asset" | "json" | "xml" | "txt" | "bytes" | "csv" | "yaml" => "Data",
        _ => "Other",
    }
}

/// The top folder of an asset: `Assets/Art/…` → `Assets/Art`,
/// `Packages/com.unity.ugui/…` → `Packages/com.unity.ugui`.
pub fn top_folder(path: &str) -> &str {
    let mut ends = path.match_indices('/').map(|(i, _)| i);
    match (ends.next(), ends.next()) {
        (Some(_), Some(second)) => &path[..second],
        (Some(first), None) => &path[..first],
        _ => path,
    }
}

fn group_name(grouping: Grouping, path: &str) -> &str {
    match grouping {
        Grouping::ByType => asset_kind(path),
        Grouping::ByFolder => top_folder(path),
        Grouping::Assets => path,
    }
}

/// The assets that match `search` (every word, any case) and the chosen
/// group, in the chosen order.
pub fn visible_assets<'a>(
    assets: &'a [SizeRow],
    search: &str,
    group: Option<&(Grouping, String)>,
    order: Order,
) -> Vec<&'a SizeRow> {
    let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
    let mut rows: Vec<&SizeRow> = assets
        .iter()
        .filter(|a| {
            let lower = a.name.to_lowercase();
            words.iter().all(|w| lower.contains(w.as_str()))
        })
        .filter(|a| group.is_none_or(|(grouping, name)| group_name(*grouping, &a.name) == name))
        .collect();
    match order {
        Order::Largest => rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name))),
        Order::Smallest => rows.sort_by(|a, b| a.bytes.cmp(&b.bytes).then(a.name.cmp(&b.name))),
        Order::Path => rows.sort_by(|a, b| a.name.cmp(&b.name)),
    }
    rows
}

/// One group of assets and its totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub count: usize,
    pub bytes: u64,
}

/// Totals per group of the given assets, largest first.
pub fn groups(assets: &[&SizeRow], grouping: Grouping) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for asset in assets {
        let name = group_name(grouping, &asset.name);
        match groups.iter_mut().find(|g| g.name == name) {
            Some(group) => {
                group.count += 1;
                group.bytes += asset.bytes;
            }
            None => groups.push(Group {
                name: name.to_owned(),
                count: 1,
                bytes: asset.bytes,
            }),
        }
    }
    groups.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));
    groups
}

/// Share of `bytes` in `of`, as Unity prints it.
fn percent(bytes: u64, of: Option<u64>) -> String {
    match of {
        Some(total) if total > 0 => format!("{:.1}%", bytes as f64 * 100.0 / total as f64),
        _ => String::new(),
    }
}

/// Unity gives a category's share of your assets ("Total User Assets")…
fn category_share(build: &BuildReport, bytes: u64) -> String {
    percent(bytes, build.user_assets.or(build.total))
}

/// …and an asset's share of the complete build.
fn share(build: &BuildReport, bytes: u64) -> String {
    percent(bytes, build.total.or(build.user_assets))
}

/// Label | bar | value, one row.
fn bar(ui: &mut Ui, label: &str, fraction: f32, value: &str) -> egui::Response {
    let visuals = ui.visuals().clone();
    let height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let label_width = (rect.width() * 0.45).min(320.0);
    let value_galley = painter.layout_no_wrap(
        value.to_owned(),
        egui::FontId::proportional(12.5),
        visuals.strong_text_color(),
    );
    let value_width = value_galley.size().x + 16.0;
    let y = rect.center().y;
    painter
        .with_clip_rect(egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + label_width - 6.0, rect.bottom()),
        ))
        .text(
            egui::pos2(rect.left() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            visuals.text_color(),
        );
    let room = (rect.width() - label_width - value_width).max(20.0);
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.left() + label_width, y - 5.0),
            vec2((room * fraction.clamp(0.0, 1.0)).max(2.0), 10.0),
        ),
        3.0,
        visuals.selection.bg_fill,
    );
    painter.galley(
        egui::pos2(
            rect.right() - 4.0 - value_galley.size().x,
            y - value_galley.size().y / 2.0,
        ),
        value_galley,
        visuals.strong_text_color(),
    );
    response
}

pub fn build_ui(
    ui: &mut Ui,
    view: &mut BuildView,
    current: &Loaded,
    history: &[SavedBuild],
) -> Option<BuildAction> {
    let builds = current.doc.build_reports();
    if builds.is_empty() {
        return Some(BuildAction::Close);
    }
    view.index = view.index.min(builds.len() - 1);
    let mut action = None;

    // Which build.
    ui.horizontal(|ui| {
        ui.label(RichText::new("🏗 Build").size(17.0).strong());
        let before = view.index;
        egui::ComboBox::from_id_salt("build-picker")
            .width(ui.available_width() - 210.0)
            .selected_text(builds[view.index].label())
            .show_ui(ui, |ui| {
                for (i, build) in builds.iter().enumerate() {
                    ui.selectable_value(
                        &mut view.index,
                        i,
                        format!("{}  ·  line {}", build.label(), build.line),
                    );
                }
            });
        if view.index != before {
            let index = view.index;
            view.index = before;
            view.show_build(index, builds, history);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("✕ Close")
                .on_hover_text("Back to the log (Esc)")
                .clicked()
            {
                action = Some(BuildAction::Close);
            }
            if ui.button("Show in log").clicked() {
                action = Some(BuildAction::ShowInLog(builds[view.index].line));
            }
        });
    });
    let build = &builds[view.index];
    ui.label(
        RichText::new(if builds.len() > 1 {
            format!("Build {} of {} in this log", view.index + 1, builds.len())
        } else {
            "The only build in this log".to_owned()
        })
        .weak()
        .small(),
    );
    ui.horizontal_wrapped(|ui| {
        compare_picker(ui, view, builds, history);
        if build.has_sizes() {
            ui.separator();
        }
        if let Some(saved) = find_saved(history, build) {
            ui.label(RichText::new("✓ In the build history").weak())
                .on_hover_text(saved.path.display().to_string());
        } else if build.has_sizes() {
            ui.add(
                egui::TextEdit::singleline(&mut view.note)
                    .hint_text("note, e.g. release 1.4")
                    .desired_width(160.0),
            );
            if ui
                .button("💾 Save to history")
                .on_hover_text("Keep this build's sizes (redacted) to compare later builds with")
                .clicked()
            {
                action = Some(BuildAction::Save {
                    index: view.index,
                    note: view.note.trim().to_owned(),
                });
                view.note.clear();
            }
        }
    });
    ui.add_space(6.0);

    egui::ScrollArea::vertical()
        .id_salt("build-view")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Totals.
            egui::Grid::new("build-totals")
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    let mut row = |label: &str, value: String| {
                        ui.label(RichText::new(label).weak());
                        ui.label(RichText::new(value).strong());
                        ui.end_row();
                    };
                    if let Some(total) = build.total {
                        row("Complete build size", format_bytes(total));
                    }
                    if let Some(user) = build.user_assets {
                        row("Your assets", format_bytes(user));
                    }
                    if !build.assets.is_empty() {
                        row("Assets listed", build.assets.len().to_string());
                    }
                    row(
                        "Result",
                        build
                            .result
                            .clone()
                            .unwrap_or_else(|| "not finished".to_owned()),
                    );
                    if let Some(d) = build.duration {
                        row("Took", format_duration(d));
                    }
                    if let (Some(from), Some(to)) = (&build.started_at, &build.finished_at) {
                        row("When", format!("{from} → {to}"));
                    }
                });

            if !build.has_sizes() {
                ui.add_space(8.0);
                let warn = ui.visuals().warn_fg_color;
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("No sizes for this build")
                            .strong()
                            .color(warn),
                    );
                    ui.label(if build.sizes_unavailable {
                        "Unity lists sizes only after a clean build: this build didn't rebuild \
                         player data. Make a clean build to see them."
                    } else {
                        "The log has no size report for this build."
                    });
                    if let Some(clean) = builds.iter().rposition(BuildReport::has_sizes)
                        && ui
                            .button(format!(
                                "Show the last clean build: {}",
                                builds[clean].label()
                            ))
                            .clicked()
                    {
                        view.show_build(clean, builds, history);
                        view.against = default_against(clean, builds, history, None);
                    }
                });
            }

            if let Some(against) = view.against.clone()
                && let Some((label, other)) = against_build(&against, builds, history)
            {
                let stale = !matches!(&view.comparison, Some((i, a, _)) if *i == view.index && *a == against);
                if stale {
                    view.comparison = Some((
                        view.index,
                        against.clone(),
                        BuildComparison::of(other, build),
                    ));
                }
                if let Some((_, _, comparison)) = view.comparison.take() {
                    ui.add_space(10.0);
                    if let Some(asked) = comparison_ui(ui, view, &comparison, (&against, &label)) {
                        action = Some(asked);
                    }
                    view.comparison = Some((view.index, against, comparison));
                }
            }

            if !history.is_empty() {
                ui.add_space(10.0);
                if let Some(asked) = history_ui(ui, view, current, history, build) {
                    action = Some(asked);
                }
            }

            if !build.categories.is_empty() {
                ui.add_space(12.0);
                ui.label(RichText::new("By type (Unity's categories)").strong());
                let mut categories: Vec<&SizeRow> = build.categories.iter().collect();
                categories.sort_by_key(|c| std::cmp::Reverse(c.bytes));
                let largest = categories.first().map_or(1, |c| c.bytes.max(1));
                for category in categories {
                    bar(
                        ui,
                        &category.name,
                        category.bytes as f32 / largest as f32,
                        &format!(
                            "{}  {}",
                            format_bytes(category.bytes),
                            category_share(build, category.bytes)
                        ),
                    );
                }
            }

            if !build.steps.is_empty() {
                ui.add_space(8.0);
                egui::CollapsingHeader::new(RichText::new("Build steps").strong())
                    .default_open(false)
                    .show(ui, |ui| {
                        let longest = build
                            .steps
                            .iter()
                            .map(|s| s.duration)
                            .max()
                            .unwrap_or(Duration::from_millis(1))
                            .max(Duration::from_millis(1));
                        for step in &build.steps {
                            bar(
                                ui,
                                &step.name,
                                step.duration.as_secs_f32() / longest.as_secs_f32(),
                                &format_duration(step.duration),
                            );
                        }
                    });
            }

            if !build.assets.is_empty() {
                ui.add_space(12.0);
                assets_ui(ui, view, build);
            }
        });
    action
}

/// "Compare with …": nothing, another build in the log, or a saved build.
fn compare_picker(
    ui: &mut Ui,
    view: &mut BuildView,
    builds: &[BuildReport],
    history: &[SavedBuild],
) {
    let current = &builds[view.index];
    if !current.has_sizes() {
        return;
    }
    let identity = current.identity();
    ui.label(RichText::new("Compare with").weak());
    let selected = view
        .against
        .as_ref()
        .and_then(|a| against_build(a, builds, history))
        .map_or_else(|| "nothing".to_owned(), |(label, _)| label);
    egui::ComboBox::from_id_salt("build-against")
        .width(380.0_f32.min(ui.available_width() - 20.0).max(160.0))
        .selected_text(selected)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut view.against, None, "Nothing");
            for (i, build) in builds.iter().enumerate() {
                if i != view.index && build.has_sizes() {
                    ui.selectable_value(
                        &mut view.against,
                        Some(Against::InLog(i)),
                        format!("This log: {}", build.label()),
                    );
                }
            }
            for saved in history {
                if saved.build.has_sizes() && saved.build.identity() != identity {
                    ui.selectable_value(
                        &mut view.against,
                        Some(Against::Saved(saved.path.clone())),
                        format!("Saved: {}", saved.label()),
                    );
                }
            }
        });
}

/// Red for growth, green for shrinking.
fn delta_color(ui: &Ui, delta: i64) -> Color32 {
    match delta {
        d if d > 0 => ui.visuals().warn_fg_color,
        d if d < 0 => Color32::from_rgb(90, 180, 110),
        _ => ui.visuals().weak_text_color(),
    }
}

/// The change list rows that match the search.
fn matching<'a>(rows: Vec<&'a SizeChange>, search: &str) -> Vec<&'a SizeChange> {
    let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
    rows.into_iter()
        .filter(|c| {
            let lower = c.name.to_lowercase();
            words.iter().all(|w| lower.contains(w.as_str()))
        })
        .collect()
}

fn comparison_ui(
    ui: &mut Ui,
    view: &mut BuildView,
    comparison: &BuildComparison,
    (against_key, against): (&Against, &str),
) -> Option<BuildAction> {
    let mut action = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let growth = comparison.growth().map_or(0, |(d, _)| d);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(comparison.headline())
                    .size(17.0)
                    .strong()
                    .color(if comparison.comparable() {
                        delta_color(ui, growth)
                    } else {
                        ui.visuals().weak_text_color()
                    }),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("📋 Copy as Markdown")
                    .on_hover_text("The comparison for a pull request or a chat, redacted")
                    .clicked()
                {
                    action = Some(BuildAction::CopyMarkdown {
                        before: against_key.clone(),
                        after: view.index,
                    });
                }
            });
        });
        ui.label(RichText::new(format!("Compared with {against}")).weak());
        let details = comparison.details();
        if !details.is_empty() {
            ui.label(RichText::new(details).small());
        }
        if !comparison.comparable() {
            return;
        }
        if let Some((_, measure)) = comparison.growth()
            && measure == "your assets"
            && comparison.total.0.is_some()
        {
            ui.label(
                RichText::new(
                    "Measured on your assets: Unity prints the complete build size only to 0.1 GB.",
                )
                .weak()
                .small(),
            );
        }
        ui.add_space(6.0);

        let responsible = comparison.responsible();
        ui.horizontal_wrapped(|ui| {
            for (list, name, count) in [
                (ChangeList::Responsible, "Responsible", responsible.len()),
                (ChangeList::Grew, "Grew", comparison.grew.len()),
                (ChangeList::Shrank, "Shrank", comparison.shrank.len()),
                (ChangeList::Added, "New", comparison.added.len()),
                (ChangeList::Removed, "Gone", comparison.removed.len()),
            ] {
                ui.selectable_value(&mut view.changes, list, format!("{name} ({count})"));
            }
            ui.label(
                RichText::new(format!("· {} unchanged", comparison.unchanged))
                    .weak()
                    .small(),
            );
        });
        let rows: Vec<&SizeChange> = match view.changes {
            ChangeList::Responsible => responsible,
            ChangeList::Grew => comparison.grew.iter().collect(),
            ChangeList::Shrank => comparison.shrank.iter().collect(),
            ChangeList::Added => comparison.added.iter().collect(),
            ChangeList::Removed => comparison.removed.iter().collect(),
        };
        let rows = matching(rows, &view.search);
        if view.changes == ChangeList::Responsible
            && let Some((growth, measure)) = comparison.growth()
            && growth != 0
            && !rows.is_empty()
        {
            let top: i64 = rows.iter().take(10).map(|c| c.delta()).sum();
            ui.label(
                RichText::new(format!(
                    "The top {} account for {} of the {} change in {measure}.",
                    rows.len().min(10),
                    format_delta(top),
                    format_delta(growth)
                ))
                .small(),
            );
        }
        if rows.is_empty() {
            ui.label(RichText::new("Nothing here.").weak());
            return;
        }
        let height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        egui::ScrollArea::vertical()
            .id_salt("build-changes")
            .max_height(height * 12.0)
            .auto_shrink([false, true])
            .show_rows(ui, height, rows.len(), |ui, range| {
                let visuals = ui.visuals().clone();
                for change in &rows[range] {
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
                    let painter = ui.painter_at(rect);
                    if response.hovered() {
                        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
                    }
                    let y = rect.center().y;
                    painter.text(
                        egui::pos2(rect.left() + 86.0, y),
                        egui::Align2::RIGHT_CENTER,
                        format_delta(change.delta()),
                        egui::FontId::proportional(13.0),
                        delta_color(ui, change.delta()),
                    );
                    painter.text(
                        egui::pos2(rect.left() + 96.0, y),
                        egui::Align2::LEFT_CENTER,
                        format!(
                            "{} → {}",
                            change.before.map_or("—".to_owned(), format_bytes),
                            change.after.map_or("—".to_owned(), format_bytes)
                        ),
                        egui::FontId::proportional(12.0),
                        visuals.weak_text_color(),
                    );
                    painter
                        .with_clip_rect(egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 250.0, rect.top()),
                            rect.max,
                        ))
                        .text(
                            egui::pos2(rect.left() + 250.0, y),
                            egui::Align2::LEFT_CENTER,
                            &change.name,
                            egui::FontId::proportional(13.0),
                            visuals.text_color(),
                        );
                    response.on_hover_text(&change.name);
                }
            });
        let categories: Vec<&SizeChange> = comparison
            .categories
            .iter()
            .filter(|c| c.delta() != 0)
            .collect();
        if !categories.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("By type").weak());
            ui.horizontal_wrapped(|ui| {
                for c in categories {
                    ui.label(
                        RichText::new(format!("{} {}", c.name, format_delta(c.delta())))
                            .color(delta_color(ui, c.delta())),
                    )
                    .on_hover_text(format!(
                        "{} → {}",
                        c.before.map_or("—".to_owned(), format_bytes),
                        c.after.map_or("—".to_owned(), format_bytes)
                    ));
                }
            });
        }
    });
    action
}

/// Saved builds of this project (all of them when the log names no
/// project), newest first, with your assets' size and the change from the
/// build saved before.
fn history_ui(
    ui: &mut Ui,
    view: &mut BuildView,
    current: &Loaded,
    history: &[SavedBuild],
    build: &BuildReport,
) -> Option<BuildAction> {
    let mut action = None;
    let project = project_name(&current.doc);
    let saved: Vec<&SavedBuild> = history
        .iter()
        .filter(|s| project.is_none() || s.project == project)
        .collect();
    let title = match &project {
        Some(p) => format!("Build history of {p} ({})", saved.len()),
        None => format!("Build history ({})", saved.len()),
    };
    let identity = build.identity();
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .id_salt("build-history")
        .default_open(true)
        .show(ui, |ui| {
            if ui.small_button("📂 Open the history folder").clicked() {
                action = Some(BuildAction::OpenHistoryFolder);
            }
            if saved.is_empty() {
                ui.label(RichText::new("No saved builds of this project yet.").weak());
                return;
            }
            let largest = saved
                .iter()
                .filter_map(|s| s.build.user_assets.or(s.build.total))
                .max()
                .unwrap_or(1)
                .max(1);
            egui::Grid::new("build-history-rows")
                .num_columns(5)
                .spacing([12.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for (i, s) in saved.iter().enumerate() {
                        let size = s.build.user_assets.or(s.build.total);
                        let this = s.build.identity() == identity;
                        ui.label(RichText::new(&s.saved_at).strong())
                            .on_hover_text(s.build.label());
                        ui.label(if s.note.is_empty() {
                            RichText::new(s.build.target.as_deref().unwrap_or("build")).weak()
                        } else {
                            RichText::new(&s.note)
                        });
                        let (rect, _) = ui.allocate_exact_size(vec2(160.0, 12.0), Sense::hover());
                        if let Some(size) = size {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    rect.min,
                                    vec2(160.0 * size as f32 / largest as f32, 10.0),
                                ),
                                3.0,
                                ui.visuals().selection.bg_fill,
                            );
                        }
                        let older = saved[i + 1..]
                            .iter()
                            .find_map(|o| o.build.user_assets.or(o.build.total));
                        ui.label(match (size, older) {
                            (Some(size), Some(older)) => {
                                let delta = size as i64 - older as i64;
                                RichText::new(format!(
                                    "{}  {}",
                                    format_bytes(size),
                                    format_delta(delta)
                                ))
                                .color(delta_color(ui, delta))
                            }
                            (Some(size), None) => RichText::new(format_bytes(size)),
                            _ => RichText::new("no sizes").weak(),
                        });
                        ui.horizontal(|ui| {
                            if this {
                                ui.label(RichText::new("this build").weak());
                            } else if s.build.has_sizes()
                                && build.has_sizes()
                                && ui.small_button("Compare").clicked()
                            {
                                view.against = Some(Against::Saved(s.path.clone()));
                                view.changes = ChangeList::Responsible;
                            }
                            if ui.small_button("Open").clicked() {
                                action = Some(BuildAction::OpenSaved(s.path.clone()));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    action
}

fn assets_ui(ui: &mut Ui, view: &mut BuildView, build: &BuildReport) {
    ui.label(RichText::new("Assets").strong());
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut view.search)
                .hint_text("🔍 search paths")
                .desired_width(220.0),
        );
        ui.label(RichText::new("Show").weak());
        ui.selectable_value(&mut view.grouping, Grouping::Assets, "Every asset");
        ui.selectable_value(&mut view.grouping, Grouping::ByType, "By file type");
        ui.selectable_value(&mut view.grouping, Grouping::ByFolder, "By folder");
        if view.grouping == Grouping::Assets {
            ui.separator();
            ui.label(RichText::new("Order").weak());
            ui.selectable_value(&mut view.order, Order::Largest, "Largest");
            ui.selectable_value(&mut view.order, Order::Smallest, "Smallest");
            ui.selectable_value(&mut view.order, Order::Path, "Path");
        }
    });
    if let Some((_, name)) = view.group.clone() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Only {name}")).strong());
            if ui.small_button("✕ all assets").clicked() {
                view.group = None;
            }
        });
    }

    let rows = visible_assets(&build.assets, &view.search, view.group.as_ref(), view.order);
    let total: u64 = rows.iter().map(|a| a.bytes).sum();
    ui.label(
        RichText::new(format!(
            "{} of {} assets · {}",
            rows.len(),
            build.assets.len(),
            format_bytes(total)
        ))
        .weak()
        .small(),
    );
    ui.add_space(4.0);

    if view.grouping != Grouping::Assets {
        let largest = groups(&rows, view.grouping);
        let top = largest.first().map_or(1, |g| g.bytes.max(1));
        for group in &largest {
            let response = bar(
                ui,
                &group.name,
                group.bytes as f32 / top as f32,
                &format!(
                    "{}  {}  ·  {} {}",
                    format_bytes(group.bytes),
                    share(build, group.bytes),
                    group.count,
                    if group.count == 1 { "asset" } else { "assets" }
                ),
            )
            .on_hover_text("Click to list its assets");
            if response.clicked() {
                view.group = Some((view.grouping, group.name.clone()));
                view.grouping = Grouping::Assets;
            }
        }
        return;
    }

    let height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    egui::ScrollArea::vertical()
        .id_salt("build-assets")
        .max_height(ui.available_height().max(320.0))
        .auto_shrink([false, true])
        .show_rows(ui, height, rows.len(), |ui, range| {
            let visuals = ui.visuals().clone();
            for asset in &rows[range] {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
                let painter = ui.painter_at(rect);
                if response.hovered() {
                    painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
                }
                let y = rect.center().y;
                painter.text(
                    egui::pos2(rect.left() + 80.0, y),
                    egui::Align2::RIGHT_CENTER,
                    format_bytes(asset.bytes),
                    egui::FontId::proportional(13.0),
                    visuals.strong_text_color(),
                );
                painter.text(
                    egui::pos2(rect.left() + 136.0, y),
                    egui::Align2::RIGHT_CENTER,
                    share(build, asset.bytes),
                    egui::FontId::proportional(12.0),
                    visuals.weak_text_color(),
                );
                painter
                    .with_clip_rect(egui::Rect::from_min_max(
                        egui::pos2(rect.left() + 150.0, rect.top()),
                        rect.max,
                    ))
                    .text(
                        egui::pos2(rect.left() + 150.0, y),
                        egui::Align2::LEFT_CENTER,
                        &asset.name,
                        egui::FontId::proportional(13.0),
                        visuals.text_color(),
                    );
                response.on_hover_text(&asset.name);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, bytes: u64) -> SizeRow {
        SizeRow {
            name: name.to_owned(),
            bytes,
        }
    }

    fn assets() -> Vec<SizeRow> {
        vec![
            row("Assets/Art/Hero.png", 4_000),
            row("Assets/Art/Sky Box.PNG", 9_000),
            row("Assets/Audio/Theme.wav", 7_000),
            row("Packages/com.unity.ugui/Runtime/UI.dll", 1_000),
            row("Assets/Resources/Config.json", 10),
        ]
    }

    #[test]
    fn kinds_and_folders() {
        assert_eq!(asset_kind("Assets/Art/Sky Box.PNG"), "Textures");
        assert_eq!(asset_kind("Assets/Audio/Theme.wav"), "Audio");
        assert_eq!(asset_kind("Assets/README"), "Other");
        assert_eq!(
            asset_kind("Assets/v1.2/noext"),
            "Other",
            "a dot in a folder is no extension"
        );
        assert_eq!(top_folder("Assets/Art/Hero.png"), "Assets/Art");
        assert_eq!(
            top_folder("Packages/com.unity.ugui/Runtime/UI.dll"),
            "Packages/com.unity.ugui"
        );
        assert_eq!(top_folder("Assets/Hero.png"), "Assets");
    }

    #[test]
    fn search_group_and_order() {
        let assets = assets();
        let names = |rows: Vec<&SizeRow>| rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
        assert_eq!(
            names(visible_assets(&assets, "", None, Order::Largest))[..2],
            ["Assets/Art/Sky Box.PNG", "Assets/Audio/Theme.wav"]
        );
        assert_eq!(
            names(visible_assets(&assets, "art PNG", None, Order::Path)),
            ["Assets/Art/Hero.png", "Assets/Art/Sky Box.PNG"],
            "every word, any case"
        );
        let textures = (Grouping::ByType, "Textures".to_owned());
        assert_eq!(
            visible_assets(&assets, "", Some(&textures), Order::Smallest)[0].bytes,
            4_000
        );

        let all = visible_assets(&assets, "", None, Order::Largest);
        let by_type = groups(&all, Grouping::ByType);
        assert_eq!(
            by_type[0],
            Group {
                name: "Textures".to_owned(),
                count: 2,
                bytes: 13_000
            }
        );
        let by_folder = groups(&all, Grouping::ByFolder);
        assert_eq!(by_folder.len(), 4);
        assert!(
            by_folder
                .iter()
                .any(|g| g.name == "Packages/com.unity.ugui")
        );
    }

    #[test]
    fn the_default_comparison_is_the_build_saved_before() {
        let sized = |mb: u64| BuildReport {
            user_assets: Some(mb),
            assets: vec![row("Assets/A.png", mb)],
            ..BuildReport::default()
        };
        let saved = |name: &str, build: BuildReport| SavedBuild {
            path: PathBuf::from(name),
            saved_at: String::new(),
            modified: None,
            project: Some("Game".to_owned()),
            note: String::new(),
            build,
        };
        // Newest first.
        let history = vec![
            saved("c.log", sized(30)),
            saved("b.log", sized(20)),
            saved("a.log", sized(10)),
        ];
        let log = vec![sized(5), BuildReport::default(), sized(20)];
        // A saved build is compared with the one saved before it…
        assert_eq!(
            default_against(2, &log, &history, Some("Game")),
            Some(Against::Saved(PathBuf::from("a.log")))
        );
        // …a new build with the newest saved one…
        let new = vec![sized(40)];
        assert_eq!(
            default_against(0, &new, &history, Some("Game")),
            Some(Against::Saved(PathBuf::from("c.log")))
        );
        // …of the same project only, else the previous build in the log.
        assert_eq!(
            default_against(2, &log, &history, Some("Other")),
            Some(Against::InLog(0))
        );
        assert_eq!(default_against(1, &log, &history, None), None, "no sizes");
    }
}
