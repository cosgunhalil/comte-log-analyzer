//! The Comte Logger window (DESIGN §9).
//!
//! ```text
//! ┌ toolbar ─────────────────────────────────────────────────────────┐
//! │ sources │ filter bar                               │ details     │
//! │         │ entry list (virtualized, grouped by      │ (entry or   │
//! │         │ timeline sections)                       │  info card) │
//! └ status bar ──────────────────────────────────────────────────────┘
//! ```

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use comte_core::{
    Category, FrameKind, LogEntry, LogKind, LogSource, Section, Severity, SourceRole,
    format_duration,
};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Layout, Modifiers, RichText, Sense, TextStyle, Ui,
    vec2,
};

use crate::cli::{age, human_size};
use crate::compare_view::{CompareAction, CompareView, compare_ui, previous_run};
use crate::devtools::DevHooks;
use crate::engines::discover_sources;
use crate::ide::{
    self, DetectedIde, IdeChoice, Launch, custom_launch, detect_ides, launch_for, resolve_source,
    system_launch,
};
use crate::live::{LiveEvent, LiveTail};
use crate::loader::{LoadEvent, Loaded, Loader};
use crate::overview::{Jump, overview};
use crate::report_export::{BuiltReport, ReportOptions, build_report, report_file_name};
use crate::save::{
    self, ContentChoice, Prepared, SaveRequest, SessionFile, current_user_name, default_dir,
    default_file_name, discover_sessions, visible_lines,
};
use crate::settings::{Settings, ThemeChoice};
use crate::theme::{severity_color, severity_icon};
use crate::view::{
    Filter, Row, build_rows, culprit_label, default_collapsed, find_problem, row_of_entry,
    section_path,
};

const SEARCH_ID: &str = "comte-search";
const INDENT: f32 = 16.0;

pub struct ComteApp {
    sources: Vec<LogSource>,
    loader: Loader,
    current: Option<Box<Loaded>>,
    load_error: Option<String>,
    filter: Filter,
    collapsed: HashSet<u32>,
    rows: Vec<Row>,
    rows_dirty: bool,
    selected: Option<u64>,
    scroll_to_selected: bool,
    /// Scroll the list to this section header (after a timeline click).
    scroll_to_section: Option<u32>,
    focus_search: bool,
    save_dialog: Option<SaveDialog>,
    /// "Also redact" words, kept between saves.
    extra_redactions: String,
    sessions: Vec<SessionFile>,
    /// A short-lived message for the status bar ("Saved to …").
    notice: Option<(String, Instant)>,
    /// Following the current file as it grows.
    live: Option<LiveTail>,
    settings: Settings,
    /// The settings as last written to disk, to save only on change.
    settings_saved: Settings,
    settings_open: bool,
    /// The comparison report, when comparing the open log (B) with another (A).
    compare: Option<CompareView>,
    report_dialog: Option<ReportDialog>,
    /// Editors found on this machine; detected on first use.
    ides: Option<Vec<DetectedIde>>,
    /// Lines added by the last live update, and when.
    live_activity: Option<(usize, Instant)>,
    dev: DevHooks,
}

/// State of the "Save session" dialog.
struct SaveDialog {
    request: SaveRequest,
    preview: Prepared,
    /// Inputs the preview was computed from; recomputed when they change.
    preview_inputs: (ContentChoice, bool, String),
    full_lines: usize,
    visible_lines: usize,
    error: Option<String>,
}

/// State of the "Report" dialog.
struct ReportDialog {
    options: ReportOptions,
    built: BuiltReport,
    /// Options (and "Also redact" words) the text was built from.
    built_from: (ReportOptions, String),
}

const NOTICE_SECONDS: u64 = 8;

impl ComteApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let dev = DevHooks::from_env();
        let settings = Settings::load();
        apply_theme(&cc.egui_ctx, settings.theme);
        let mut filter = Filter {
            show_noise: settings.show_noise || dev.show_noise,
            collapse_duplicates: settings.collapse_duplicates,
            ..Filter::default()
        };
        if let Some(search) = &dev.search {
            filter.search = search.clone();
        }
        let mut app = Self {
            sources: if dev.no_discovery {
                Vec::new()
            } else {
                discover_sources()
            },
            loader: Loader::default(),
            current: None,
            load_error: None,
            filter,
            collapsed: HashSet::new(),
            rows: Vec::new(),
            rows_dirty: false,
            selected: None,
            scroll_to_selected: false,
            scroll_to_section: None,
            focus_search: false,
            save_dialog: None,
            extra_redactions: settings.also_redact.clone(),
            sessions: if dev.no_discovery {
                Vec::new()
            } else {
                discover_sessions(&sessions_dir(&settings))
            },
            notice: None,
            live: None,
            live_activity: None,
            settings_saved: settings.clone(),
            settings,
            settings_open: dev.settings_open,
            compare: None,
            report_dialog: None,
            ides: None,
            dev,
        };
        // Open what was asked for, or the Editor log: reading it is the main job.
        let initial = open
            .map(|path| {
                let title = file_title(&path);
                (path, title)
            })
            .or_else(|| {
                app.sources
                    .iter()
                    .find(|s| {
                        s.kind == LogKind::Editor && s.role == SourceRole::Current && s.exists()
                    })
                    .map(|s| (s.path.clone(), s.title.clone()))
            });
        if let Some((path, title)) = initial {
            app.loader.start(path, title, &cc.egui_ctx);
        }
        app
    }

    fn open(&mut self, path: PathBuf, title: String, ctx: &egui::Context) {
        self.load_error = None;
        self.stop_live();
        self.loader.start(path, title, ctx);
    }

    // -- live tail ------------------------------------------------------------

    /// Live tail works on real logs; a saved session never changes.
    fn can_go_live(&self) -> bool {
        self.current.as_ref().is_some_and(|c| c.session.is_none())
    }

    fn start_live(&mut self, ctx: &egui::Context) {
        if let Some(current) = &self.current
            && current.session.is_none()
        {
            self.live = Some(LiveTail::start(
                current.path.clone(),
                current.title.clone(),
                ctx,
            ));
            self.live_activity = None;
        }
    }

    fn stop_live(&mut self) {
        self.live = None;
        self.live_activity = None;
    }

    fn poll_live(&mut self, ctx: &egui::Context) {
        let Some(live) = &self.live else {
            return;
        };
        for event in live.poll() {
            match event {
                LiveEvent::Update { loaded, added } => {
                    self.apply_live_snapshot(loaded);
                    if added > 0 {
                        self.live_activity = Some((added, Instant::now()));
                    }
                }
                LiveEvent::Restarted(loaded) => {
                    self.collapsed = default_collapsed(&loaded.doc);
                    self.current = Some(loaded);
                    self.selected = None;
                    self.rows_dirty = true;
                    self.notice = Some((
                        "Log restarted (Unity was restarted); showing the new log".to_owned(),
                        Instant::now(),
                    ));
                }
                LiveEvent::Failed(error) => {
                    self.load_error = Some(format!("Live tail stopped: {error}"));
                    self.stop_live();
                    return;
                }
            }
        }
        // Keep the "+n lines, 3 s ago" text fresh.
        if self.live.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    /// Shows a newer version of the same log, keeping what the user set up:
    /// selection, collapsed sections, filters. New quiet sections start
    /// collapsed like on open.
    fn apply_live_snapshot(&mut self, loaded: Box<Loaded>) {
        let known_sections = self.current.as_ref().map_or(0, |c| c.doc.sections().len());
        for id in default_collapsed(&loaded.doc) {
            if id as usize >= known_sections {
                self.collapsed.insert(id);
            }
        }
        if let Some(id) = self.selected
            && loaded.doc.entry(id).is_none()
        {
            self.selected = None;
        }
        self.current = Some(loaded);
        self.rows_dirty = true;
        if let Some(compare) = &mut self.compare {
            compare.invalidate();
        }
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open a log file")
            .add_filter("Log files", &["log", "txt"])
            .add_filter("All files", &["*"])
            .pick_file()
        {
            let title = file_title(&path);
            self.open(path, title, ctx);
        }
    }

    // -- saving ---------------------------------------------------------------

    fn open_save_dialog(&mut self) {
        let Some(current) = &self.current else {
            return;
        };
        // Saving a reopened session again keeps its note and tags.
        let mut request = match &current.session {
            Some(session) => SaveRequest {
                note: session.note.clone(),
                tags: session.tags.join(", "),
                ..SaveRequest::default()
            },
            None => SaveRequest::default(),
        };
        if let Some(note) = &self.dev.save_note {
            request.note.clone_from(note);
        }
        if let Some(tags) = &self.dev.save_tags {
            request.tags.clone_from(tags);
        }
        if let Some(extra) = &self.dev.save_extra {
            self.extra_redactions.clone_from(extra);
        }
        self.save_dialog = Some(self.new_save_dialog(request));
        if let Some(path) = self.dev.save_to.take() {
            self.write_session(path);
        }
    }

    fn write_session(&mut self, path: PathBuf) {
        let Some(dialog) = &self.save_dialog else {
            return;
        };
        // Recompute so the timestamp is the moment of saving.
        let prepared = self.prepare(&dialog.request);
        match save::write(&path, &prepared) {
            Ok(()) => {
                self.notice = Some((
                    format!("Saved session to {}", path.display()),
                    Instant::now(),
                ));
                self.save_dialog = None;
                self.sessions = discover_sessions(&self.sessions_dir());
            }
            Err(err) => {
                if let Some(dialog) = &mut self.save_dialog {
                    dialog.error = Some(format!("Could not save: {err}"));
                }
            }
        }
    }

    fn new_save_dialog(&self, request: SaveRequest) -> SaveDialog {
        let current = self.current.as_ref().expect("a log is open");
        let preview = self.prepare(&request);
        SaveDialog {
            preview_inputs: (
                request.content,
                request.redact,
                self.extra_redactions.clone(),
            ),
            full_lines: current.raw.len(),
            visible_lines: visible_lines(current, &self.filter).len(),
            request,
            preview,
            error: None,
        }
    }

    fn prepare(&self, request: &SaveRequest) -> Prepared {
        let current = self.current.as_ref().expect("a log is open");
        let user = current_user_name();
        let mut names: Vec<&str> = user.iter().map(String::as_str).collect();
        names.extend(
            self.extra_redactions
                .split(',')
                .map(str::trim)
                .filter(|w| !w.is_empty()),
        );
        let saved_at = chrono::Local::now()
            .format("%Y-%m-%dT%H:%M:%S%:z")
            .to_string();
        save::prepare(current, &self.filter, request, saved_at, &names)
    }

    /// Asks where to save, then writes the session file.
    fn save_to_file(&mut self) {
        let (Some(current), Some(dialog)) = (&self.current, &self.save_dialog) else {
            return;
        };
        let title = current
            .session
            .as_ref()
            .map_or(current.title.as_str(), |s| s.source.as_str());
        let name = default_file_name(title, &chrono::Local::now());
        let dir = self.sessions_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = dialog;
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Save session")
            .set_directory(&dir)
            .set_file_name(&name)
            .add_filter("Log files", &["log"])
            .save_file()
        {
            self.write_session(path);
        } // cancelled: keep the dialog open
    }

    fn save_dialog_ui(&mut self, ctx: &egui::Context) {
        let sessions_folder = self.sessions_dir();
        let Some(dialog) = &mut self.save_dialog else {
            return;
        };
        let mut save_clicked = false;
        let mut cancel = false;
        let extra = &mut self.extra_redactions;
        let response = egui::Modal::new(egui::Id::unique("save-session")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading("Save session");
            ui.label(
                RichText::new("Writes a plain .log file: a header with your note and tags, then the log lines.")
                    .weak(),
            );
            ui.add_space(8.0);

            ui.label(RichText::new("Note").strong());
            ui.add(
                egui::TextEdit::multiline(&mut dialog.request.note)
                    .hint_text("What happened? How can someone reproduce it?")
                    .desired_rows(5)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(6.0);
            ui.label(RichText::new("Tags").strong());
            ui.add(
                egui::TextEdit::singleline(&mut dialog.request.tags)
                    .hint_text("crash, perf, build 1.2.3")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(10.0);
            ui.label(RichText::new("Content").strong());
            ui.radio_value(
                &mut dialog.request.content,
                ContentChoice::Full,
                format!("Whole log ({} lines)", dialog.full_lines),
            );
            ui.radio_value(
                &mut dialog.request.content,
                ContentChoice::Visible,
                format!("Only entries matching the current filters ({} lines)", dialog.visible_lines),
            );

            ui.add_space(10.0);
            ui.checkbox(&mut dialog.request.redact, RichText::new("Redact sensitive values").strong())
                .on_hover_text("User names, tokens, licensing ids, player names and ids, emails, IPs, machine names");
            ui.add_enabled_ui(dialog.request.redact, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Also redact:");
                    ui.add(
                        egui::TextEdit::singleline(extra)
                            .hint_text("your display name, studio name, …")
                            .desired_width(f32::INFINITY),
                    );
                });
                if let Some(report) = &dialog.preview.report {
                    let text = if report.total == 0 {
                        "Nothing sensitive found.".to_owned()
                    } else {
                        format!("{} values will be replaced: {}.", report.total, report.describe())
                    };
                    ui.label(RichText::new(text).weak());
                }
            });

            if let Some(error) = &dialog.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("💾 Save…").strong()).clicked() {
                    save_clicked = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                ui.label(RichText::new(format!("to {}", sessions_folder.display())).weak().small());
            });
        });
        if response.should_close() || cancel {
            self.save_dialog = None;
            return;
        }
        // Keep the redaction preview in step with the choices.
        let inputs = (
            dialog.request.content,
            dialog.request.redact,
            self.extra_redactions.clone(),
        );
        if inputs != dialog.preview_inputs {
            let request = dialog.request.clone();
            let preview = self.prepare(&request);
            if let Some(dialog) = &mut self.save_dialog {
                dialog.preview = preview;
                dialog.preview_inputs = inputs;
            }
        }
        if save_clicked {
            self.save_to_file();
        }
    }

    fn reload(&mut self, ctx: &egui::Context) {
        if self.live.is_some() {
            self.start_live(ctx); // read again from the start, keep following
            return;
        }
        if let Some(current) = &self.current {
            let (path, title) = (current.path.clone(), current.title.clone());
            self.open(path, title, ctx);
        }
    }

    fn on_loaded(&mut self, event: LoadEvent, ctx: &egui::Context) {
        match event {
            LoadEvent::Done(loaded) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "Comte Logger — {}",
                    loaded.title
                )));
                self.collapsed = default_collapsed(&loaded.doc);
                self.current = Some(loaded);
                self.selected = None;
                self.rows_dirty = true;
                self.load_error = None;
                if let Some(compare) = &mut self.compare {
                    compare.invalidate();
                }
                if self.dev.select_problem {
                    self.rebuild_rows();
                    self.jump_to_problem(true);
                }
                if self.dev.select_verdict
                    && let Some(id) = self.current.as_ref().and_then(|c| c.insights.verdict.entry)
                {
                    self.rebuild_rows();
                    self.reveal(id);
                }
                if self.dev.open_save {
                    self.open_save_dialog();
                }
                if self.dev.open_selected {
                    self.open_selected();
                }
                if self.dev.report {
                    self.open_report_dialog();
                }
                if self.dev.compare && self.compare.is_none() {
                    self.start_compare(ctx);
                }
                let is_editor_log = self.current.as_ref().is_some_and(|c| {
                    c.session.is_none() && c.path.file_name().is_some_and(|n| n == "Editor.log")
                });
                if self.settings.follow_editor_log && is_editor_log && self.live.is_none() {
                    self.start_live(ctx);
                }
                if self.dev.live && self.live.is_none() {
                    self.start_live(ctx);
                }
            }
            LoadEvent::Failed { path, error } => {
                self.load_error = Some(format!("Could not open {}: {error}", path.display()));
            }
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows = match &self.current {
            Some(c) => build_rows(&c.doc, &c.index, &self.filter, &self.collapsed),
            None => Vec::new(),
        };
        self.rows_dirty = false;
    }

    /// Selects the next/previous warning or error, expanding its sections.
    fn jump_to_problem(&mut self, forward: bool) {
        let Some(current) = &self.current else {
            return;
        };
        let Some(id) = find_problem(&current.doc, &self.filter, self.selected, forward) else {
            return;
        };
        self.reveal(id);
    }

    /// Selects entry `id`, opening the sections around it, and scrolls to it.
    fn reveal(&mut self, id: u64) {
        let Some(current) = &self.current else {
            return;
        };
        let section = current.doc.entry(id).and_then(|e| e.section_id);
        for ancestor in section_path(&current.doc, section) {
            if self.collapsed.remove(&ancestor) {
                self.rows_dirty = true;
            }
        }
        // A jump target hidden by the filters (e.g. engine noise) is shown anyway.
        if let Some(entry) = current.doc.entry(id) {
            let needle = self.filter.search.trim().to_lowercase();
            if !self.filter.matches(entry, &needle) {
                self.filter = Filter {
                    show_noise: true,
                    ..Filter::default()
                };
                self.rows_dirty = true;
            }
        }
        self.select(Some(id));
    }

    /// Opens section `id` (and its parents) and scrolls to its header.
    fn reveal_section(&mut self, id: u32) {
        let Some(current) = &self.current else {
            return;
        };
        for ancestor in section_path(&current.doc, Some(id)) {
            self.collapsed.remove(&ancestor);
        }
        self.rows_dirty = true;
        self.scroll_to_section = Some(id);
    }

    // -- report ----------------------------------------------------------------

    fn open_report_dialog(&mut self) {
        let Some(current) = &self.current else {
            return;
        };
        let mut options = ReportOptions::default();
        if let Some(session) = &current.session {
            options.note.clone_from(&session.note);
            options.tags = session.tags.join(", ");
        }
        if self.dev.report_slack {
            options.style = comte_core::ReportStyle::Slack;
        }
        let built = self.build_report(&options);
        self.report_dialog = Some(ReportDialog {
            built_from: (options.clone(), self.extra_redactions.clone()),
            options,
            built,
        });
    }

    fn build_report(&self, options: &ReportOptions) -> BuiltReport {
        let current = self.current.as_ref().expect("a log is open");
        let user = current_user_name();
        let mut names: Vec<&str> = user.iter().map(String::as_str).collect();
        names.extend(
            self.extra_redactions
                .split(',')
                .map(str::trim)
                .filter(|w| !w.is_empty()),
        );
        let comparison = self
            .compare
            .as_ref()
            .and_then(|c| c.result.as_ref().map(|r| (c.a_title.as_str(), r)));
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        build_report(current, comparison, options, &now, &names)
    }

    fn report_dialog_ui(&mut self, ctx: &egui::Context) {
        let has_comparison = self.compare.as_ref().is_some_and(|c| c.result.is_some());
        let Some(dialog) = &mut self.report_dialog else {
            return;
        };
        let mut close = false;
        let mut copy = false;
        let mut save = false;
        let extra = &mut self.extra_redactions;
        let response = egui::Modal::new(egui::Id::unique("report")).show(ctx, |ui| {
            ui.set_width(720.0);
            ui.heading("Report");
            ui.label(
                RichText::new("A short summary to paste into Jira, Slack or a pull request.")
                    .weak(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Format:");
                for style in [
                    comte_core::ReportStyle::Markdown,
                    comte_core::ReportStyle::Slack,
                ] {
                    ui.radio_value(&mut dialog.options.style, style, style.label());
                }
            });
            ui.horizontal(|ui| {
                ui.label("Top problems:");
                ui.add(egui::DragValue::new(&mut dialog.options.max_problems).range(1..=20));
                if has_comparison {
                    ui.checkbox(
                        &mut dialog.options.include_comparison,
                        "Include the comparison",
                    );
                }
            });
            ui.label(RichText::new("Note").strong());
            ui.add(
                egui::TextEdit::multiline(&mut dialog.options.note)
                    .hint_text("What happened? How can someone reproduce it?")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                ui.label("Tags:");
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.options.tags)
                        .hint_text("crash, perf, build 1.2.3")
                        .desired_width(f32::INFINITY),
                );
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut dialog.options.redact, "Redact sensitive values");
                ui.add_enabled(
                    dialog.options.redact,
                    egui::TextEdit::singleline(extra)
                        .hint_text("also redact: display name, studio name, …")
                        .desired_width(f32::INFINITY),
                );
            });
            if let Some(redaction) = &dialog.built.redaction {
                ui.label(RichText::new(format!("Redacted: {}.", redaction.describe())).weak());
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(330.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut dialog.built.text.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("📋 Copy").strong()).clicked() {
                    copy = true;
                }
                if ui.button("💾 Save .md…").clicked() {
                    save = true;
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
        if response.should_close() || close {
            self.report_dialog = None;
            return;
        }
        let inputs = (dialog.options.clone(), self.extra_redactions.clone());
        if inputs != dialog.built_from {
            let built = self.build_report(&inputs.0);
            if let Some(dialog) = &mut self.report_dialog {
                dialog.built = built;
                dialog.built_from = inputs;
            }
        }
        let Some(dialog) = &self.report_dialog else {
            return;
        };
        if copy {
            ctx.copy_text(dialog.built.text.clone());
            self.notice = Some(("Report copied to the clipboard".to_owned(), Instant::now()));
            self.report_dialog = None;
        } else if save {
            let title = self.current.as_ref().map_or("log", |c| c.title.as_str());
            let name = report_file_name(title, &chrono::Local::now());
            let dir = self.sessions_dir().with_file_name("reports");
            let _ = std::fs::create_dir_all(&dir);
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Save report")
                .set_directory(&dir)
                .set_file_name(&name)
                .add_filter("Markdown", &["md"])
                .save_file()
            {
                self.notice = Some(match std::fs::write(&path, &dialog.built.text) {
                    Ok(()) => (
                        format!("Saved report to {}", path.display()),
                        Instant::now(),
                    ),
                    Err(err) => (format!("Could not save the report: {err}"), Instant::now()),
                });
                self.report_dialog = None;
            }
        }
    }

    // -- compare ---------------------------------------------------------------

    /// Compares the open log (B) with the previous run, or a file the user picks.
    fn start_compare(&mut self, ctx: &egui::Context) {
        let Some(current) = &self.current else {
            return;
        };
        let a = previous_run(&current.path).or_else(|| {
            rfd::FileDialog::new()
                .set_title("Compare with… (log A, before)")
                .add_filter("Log files", &["log", "txt"])
                .pick_file()
        });
        if let Some(path) = a {
            let title = file_title(&path);
            self.compare = Some(CompareView::start(path, title, ctx));
        }
    }

    fn on_compare_action(&mut self, action: CompareAction, ctx: &egui::Context) {
        match action {
            CompareAction::ShowInB(id) => {
                self.compare = None;
                self.reveal(id);
            }
            CompareAction::ShowInA(id) => {
                if let Some(a) = self.compare.take().and_then(|c| c.a) {
                    self.show_loaded(a, ctx);
                    self.reveal(id);
                }
            }
            CompareAction::PickA { path, title } => {
                if let Some(compare) = &mut self.compare {
                    compare.pick(path, title, ctx);
                }
            }
            CompareAction::BrowseA => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Compare with… (log A, before)")
                    .add_filter("Log files", &["log", "txt"])
                    .pick_file()
                    && let Some(compare) = &mut self.compare
                {
                    let title = file_title(&path);
                    compare.pick(path, title, ctx);
                }
            }
            CompareAction::Swap => {
                let a = self.compare.take().and_then(|c| c.a);
                if let (Some(a), Some(b)) = (a, self.current.take()) {
                    self.show_loaded(a, ctx);
                    self.compare = Some(CompareView::with_loaded(b));
                }
            }
            CompareAction::Close => self.compare = None,
        }
    }

    /// Shows an already loaded log (from a comparison) in the viewer.
    fn show_loaded(&mut self, loaded: Box<Loaded>, ctx: &egui::Context) {
        self.stop_live();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "Comte Logger — {}",
            loaded.title
        )));
        self.collapsed = default_collapsed(&loaded.doc);
        self.current = Some(loaded);
        self.selected = None;
        self.rows_dirty = true;
    }

    // -- open in IDE -----------------------------------------------------------

    /// Folders that relative paths like `Assets/Scripts/A.cs` are tried in.
    fn project_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self.settings.project_root.iter().cloned().collect();
        if let Some(path) = self
            .current
            .as_ref()
            .and_then(|c| c.doc.fact(&comte_core::FactKey::ProjectPath))
        {
            let path = PathBuf::from(path);
            if path.is_dir() {
                roots.push(path);
            }
        }
        roots
    }

    fn detected_ides(&mut self) -> &[DetectedIde] {
        self.ides.get_or_insert_with(detect_ides)
    }

    /// The editor files open in, by name, for buttons and tooltips.
    fn ide_name(&mut self) -> String {
        let choice = self.settings.ide;
        match choice {
            IdeChoice::System => "the default app".to_owned(),
            IdeChoice::Custom => "your editor".to_owned(),
            _ => self
                .chosen_ide(choice)
                .map_or_else(|| "the default app".to_owned(), |ide| ide.name()),
        }
    }

    fn chosen_ide(&mut self, choice: IdeChoice) -> Option<DetectedIde> {
        let wanted = match choice {
            IdeChoice::Rider => Some(ide::IdeKind::Rider),
            IdeChoice::VsCode => Some(ide::IdeKind::VsCode),
            IdeChoice::VisualStudio => Some(ide::IdeKind::VisualStudio),
            IdeChoice::Auto => None,
            IdeChoice::System | IdeChoice::Custom => return None,
        };
        self.detected_ides()
            .iter()
            .find(|ide| wanted.is_none_or(|kind| ide.kind == kind))
            .cloned()
    }

    fn launch_for(
        &mut self,
        file: &Path,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Option<Launch> {
        match self.settings.ide {
            IdeChoice::System => Some(system_launch(file)),
            IdeChoice::Custom => custom_launch(&self.settings.custom_command, file, line, column),
            choice => Some(match self.chosen_ide(choice) {
                Some(ide) => launch_for(&ide.exe, file, line, column),
                None => system_launch(file),
            }),
        }
    }

    /// Opens a source location from the log in the chosen editor.
    fn open_location(&mut self, location: &comte_core::SourceRef) {
        let roots = self.project_roots();
        let Some(file) = resolve_source(&location.path, &roots) else {
            self.notice = Some((
                format!(
                    "Can't find {} — choose the Unity project folder in ⚙",
                    location.path
                ),
                Instant::now(),
            ));
            return;
        };
        let Some(launch) = self.launch_for(&file, location.line, location.column) else {
            self.notice = Some((
                "The custom editor command is empty — set it in ⚙".to_owned(),
                Instant::now(),
            ));
            return;
        };
        let name = self.ide_name();
        if self.dev.ide_dry_run {
            println!("{} {}", launch.program.display(), launch.args.join(" "));
            self.notice = Some((
                format!(
                    "Would run: {} {}",
                    launch.program.display(),
                    launch.args.join(" ")
                ),
                Instant::now(),
            ));
            return;
        }
        self.notice = Some(match ide::spawn(&launch) {
            Ok(()) => (
                format!("Opened {} in {name}", location.short()),
                Instant::now(),
            ),
            Err(err) => (
                format!("Could not start {}: {err}", launch.program.display()),
                Instant::now(),
            ),
        });
    }

    /// Opens the selected entry's culprit (Ctrl+E, double-click).
    fn open_selected(&mut self) {
        let location = self
            .selected
            .and_then(|id| self.current.as_ref()?.doc.entry(id))
            .and_then(|e| e.culprit().cloned());
        if let Some(location) = location {
            self.open_location(&location);
        }
    }

    fn sessions_dir(&self) -> PathBuf {
        sessions_dir(&self.settings)
    }

    /// Writes the settings when something changed since the last save.
    fn persist_settings(&mut self) {
        self.settings.also_redact.clone_from(&self.extra_redactions);
        if self.settings == self.settings_saved {
            return;
        }
        match self.settings.save() {
            Ok(()) => self.settings_saved = self.settings.clone(),
            Err(err) => {
                self.notice = Some((format!("Could not save settings: {err}"), Instant::now()));
                self.settings_saved = self.settings.clone(); // don't retry every frame
            }
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        let theme_before = self.settings.theme;
        let sessions_before = self.settings.sessions_dir.clone();
        let unity_choice = self
            .detected_ides()
            .iter()
            .find(|i| i.from_unity)
            .map(DetectedIde::name);
        let detected_project = self
            .current
            .as_ref()
            .and_then(|c| c.doc.fact(&comte_core::FactKey::ProjectPath))
            .map(str::to_owned);
        let default_sessions = default_dir();
        let settings = &mut self.settings;
        let extra = &mut self.extra_redactions;
        let top_center = ctx.content_rect().center_top() + egui::vec2(0.0, 48.0);
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .pivot(Align2::CENTER_TOP)
            .default_pos(top_center)
            .show(ctx, |ui| {
                ui.label(RichText::new("Open code in").strong());
                for choice in IdeChoice::ALL {
                    let label = match (choice, &unity_choice) {
                        (IdeChoice::Auto, Some(name)) => format!("Auto: {name}, like Unity"),
                        _ => choice.label().to_owned(),
                    };
                    ui.radio_value(&mut settings.ide, choice, label);
                }
                if settings.ide == IdeChoice::Custom {
                    ui.add(
                        egui::TextEdit::singleline(&mut settings.custom_command)
                            .hint_text(r#""C:\path\editor.exe" {file}:{line}"#)
                            .desired_width(f32::INFINITY),
                    );
                }

                ui.separator();
                ui.label(RichText::new("Unity project folder").strong());
                match (&settings.project_root, &detected_project) {
                    (Some(root), _) => ui.label(root.display().to_string()),
                    (None, Some(path)) => {
                        ui.label(RichText::new(format!("From the log: {path}")).weak())
                    }
                    (None, None) => {
                        ui.label(RichText::new("Not set (Player logs don't name it)").weak())
                    }
                };
                ui.horizontal(|ui| {
                    if ui.button("Choose…").clicked()
                        && let Some(folder) = rfd::FileDialog::new()
                            .set_title("Unity project folder (the one containing Assets)")
                            .pick_folder()
                    {
                        settings.project_root = Some(folder);
                    }
                    if settings.project_root.is_some() && ui.button("Clear").clicked() {
                        settings.project_root = None;
                    }
                });

                ui.separator();
                ui.label(RichText::new("Saved sessions folder").strong());
                let current = settings
                    .sessions_dir
                    .clone()
                    .unwrap_or_else(|| default_sessions.clone());
                ui.label(current.display().to_string());
                ui.horizontal(|ui| {
                    if ui.button("Choose…").clicked()
                        && let Some(folder) = rfd::FileDialog::new()
                            .set_title("Folder for saved sessions")
                            .pick_folder()
                    {
                        settings.sessions_dir = Some(folder);
                    }
                    if settings.sessions_dir.is_some() && ui.button("Use the default").clicked() {
                        settings.sessions_dir = None;
                    }
                });

                ui.separator();
                ui.label(RichText::new("Redaction").strong());
                ui.label(RichText::new("Always also redact these words (comma-separated):").weak());
                ui.add(
                    egui::TextEdit::singleline(extra)
                        .hint_text("your display name, studio name, …")
                        .desired_width(f32::INFINITY),
                );

                ui.separator();
                ui.label(RichText::new("When a log opens").strong());
                ui.checkbox(&mut settings.collapse_duplicates, "Collapse duplicates");
                ui.checkbox(&mut settings.show_noise, "Show engine noise");
                ui.checkbox(&mut settings.follow_editor_log, "Follow Editor.log live");

                ui.separator();
                ui.label(RichText::new("Appearance").strong());
                ui.horizontal(|ui| {
                    for theme in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
                        ui.radio_value(&mut settings.theme, theme, theme.label());
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Comte Logger {}", env!("CARGO_PKG_VERSION"))).weak(),
                    );
                    if let Some(path) = Settings::path() {
                        ui.label(
                            RichText::new(format!("· {}", path.display()))
                                .weak()
                                .small(),
                        )
                        .on_hover_text("Where these settings are stored");
                    }
                });
            });
        self.settings_open = open;
        if self.settings.theme != theme_before {
            apply_theme(ctx, self.settings.theme);
        }
        if self.settings.sessions_dir != sessions_before {
            self.sessions = discover_sessions(&self.sessions_dir());
        }
    }

    fn select(&mut self, id: Option<u64>) {
        self.selected = id;
        self.scroll_to_selected = id.is_some();
    }

    /// Moves the selection to the next/previous entry row.
    fn step_selection(&mut self, forward: bool) {
        let next = {
            let Some(current) = &self.current else {
                return;
            };
            let at = self
                .selected
                .and_then(|id| row_of_entry(&self.rows, &current.doc, id));
            let entry_id = |row: &Row| match row {
                Row::Entry { id, .. } => Some(*id),
                Row::Section { .. } => None,
            };
            match (at, forward) {
                (Some(i), true) => self.rows[i + 1..].iter().find_map(entry_id),
                (Some(i), false) => self.rows[..i].iter().rev().find_map(entry_id),
                (None, true) => self.rows.iter().find_map(entry_id),
                (None, false) => self.rows.iter().rev().find_map(entry_id),
            }
        };
        if next.is_some() {
            self.select(next);
        }
    }

    // -- input ----------------------------------------------------------------

    fn handle_input(&mut self, ctx: &egui::Context) {
        if self.save_dialog.is_some() || self.report_dialog.is_some() {
            return; // the dialog owns the keyboard (Esc closes it)
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::E)) {
            self.open_selected();
        }
        let save = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S));
        if save {
            self.open_save_dialog();
        }
        let (open, reload, find, next, prev, escape, down, up) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::O),
                i.consume_key(Modifiers::NONE, Key::F5),
                i.consume_key(Modifiers::COMMAND, Key::F),
                i.consume_key(Modifiers::NONE, Key::F8),
                i.consume_key(Modifiers::SHIFT, Key::F8),
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
            )
        });
        let typing = ctx.memory(|m| m.focused().is_some());
        if open {
            self.open_dialog(ctx);
        }
        if reload {
            self.reload(ctx);
        }
        if find {
            self.focus_search = true;
        }
        if next {
            self.jump_to_problem(true);
        }
        if prev {
            self.jump_to_problem(false);
        }
        if escape && !typing {
            if self.compare.is_some() {
                self.compare = None;
            } else {
                self.select(None);
            }
        }
        if !typing {
            if down {
                self.step_selection(true);
            }
            if up {
                self.step_selection(false);
            }
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(path) = dropped.into_iter().next() {
            let title = file_title(&path);
            self.open(path, title, ctx);
        }
    }

    // -- panels ---------------------------------------------------------------

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Comte Logger").strong().size(16.0));
            ui.separator();
            if ui
                .button("📂 Open…")
                .on_hover_text("Open a log file (Ctrl+O)")
                .clicked()
            {
                self.open_dialog(ui.ctx());
            }
            if ui
                .selectable_label(self.settings_open, "⚙")
                .on_hover_text("Settings")
                .clicked()
            {
                self.settings_open = !self.settings_open;
            }
            let can_reload = self.current.is_some();
            if ui
                .add_enabled(can_reload, egui::Button::new("⟳ Reload"))
                .on_hover_text("Read the file again (F5)")
                .clicked()
            {
                self.reload(ui.ctx());
            }
            if ui
                .add_enabled(can_reload, egui::Button::new("💾 Save session…"))
                .on_hover_text("Save this log with a note and tags (Ctrl+S)")
                .clicked()
            {
                self.open_save_dialog();
            }
            if ui
                .add_enabled(can_reload, egui::Button::new("⇪ Report…"))
                .on_hover_text("A short Markdown or Slack summary to paste or save")
                .clicked()
            {
                self.open_report_dialog();
            }
            if ui
                .add_enabled(
                    can_reload,
                    egui::Button::selectable(self.compare.is_some(), "⇄ Compare…"),
                )
                .on_hover_text("Compare this log with the previous run or another log")
                .clicked()
            {
                if self.compare.is_some() {
                    self.compare = None;
                } else {
                    self.start_compare(ui.ctx());
                }
            }
            let live = self.live.is_some();
            let live_text = if live {
                RichText::new("● Live")
                    .color(ui.visuals().error_fg_color)
                    .strong()
            } else {
                RichText::new("● Live")
            };
            let response = ui
                .add_enabled(
                    self.can_go_live(),
                    egui::Button::selectable(live, live_text),
                )
                .on_hover_text(
                    "Follow the file as Unity writes it; new lines appear within a second",
                )
                .on_disabled_hover_text("Saved sessions do not change");
            if response.clicked() {
                if live {
                    self.stop_live();
                } else {
                    self.start_live(ui.ctx());
                }
            }
            ui.separator();
            match &self.current {
                Some(c) => {
                    ui.label(RichText::new(&c.title).strong());
                    ui.label(RichText::new(&c.kind_label).weak());
                }
                None => {
                    ui.label(RichText::new("No log open").weak());
                }
            }
            if self.loader.pending().is_some() {
                ui.spinner();
            }
        });
    }

    fn status_bar(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(error) = &self.load_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return;
            }
            if let Some((notice, at)) = &self.notice
                && at.elapsed() < Duration::from_secs(NOTICE_SECONDS)
            {
                ui.label(RichText::new(format!("✔ {notice}")).strong());
                ui.ctx().request_repaint_after(Duration::from_secs(1));
                return;
            }
            if let Some(path) = self.loader.pending() {
                ui.spinner();
                ui.label(format!("Reading {}…", path.display()));
                return;
            }
            let Some(c) = &self.current else {
                ui.label("Open a log from the left, with Ctrl+O, or drop a file onto the window.");
                return;
            };
            let counts = c.doc.counts();
            let entry_rows = self
                .rows
                .iter()
                .filter(|r| matches!(r, Row::Entry { .. }))
                .count();
            ui.label(format!(
                "{} lines · {} entries · {} shown · {} errors · {} warnings",
                c.doc.line_count(),
                c.doc.entries().len(),
                entry_rows,
                counts.errors(),
                counts.warning
            ));
            if let Some(version) = c.doc.fact(&comte_core::FactKey::EngineVersion) {
                ui.separator();
                ui.label(format!("Unity {version}"));
            }
            ui.separator();
            if self.live.is_some() {
                let text = match &self.live_activity {
                    Some((added, at)) => {
                        format!("● Live · +{added} lines {} s ago", at.elapsed().as_secs())
                    }
                    None => "● Live · waiting for new lines".to_owned(),
                };
                ui.label(RichText::new(text).color(ui.visuals().error_fg_color));
            } else {
                ui.label(format!("parsed in {}", format_duration(c.elapsed)));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new("F8 next problem · Ctrl+F search · ↑↓ move")
                        .weak()
                        .small(),
                );
            });
        });
    }

    fn sources_panel(&mut self, ui: &mut Ui) {
        let current_path = self.current.as_ref().map(|c| c.path.clone());
        let mut to_open: Option<(PathBuf, String)> = None;

        egui::ScrollArea::vertical()
            .id_salt("sources")
            .show(ui, |ui| {
                ui.add_space(4.0);
                for (heading, kind) in [
                    ("UNITY EDITOR", LogKind::Editor),
                    ("UNITY PLAYERS", LogKind::Player),
                ] {
                    ui.label(RichText::new(heading).small().strong().weak());
                    let sources: Vec<&LogSource> =
                        self.sources.iter().filter(|s| s.kind == kind).collect();
                    if sources.is_empty() {
                        ui.label(RichText::new("none found").weak().italics());
                    }
                    for source in sources {
                        let selected = current_path.as_deref() == Some(source.path.as_path());
                        let label = match source.role {
                            SourceRole::Previous => format!("{}  (previous)", source.title),
                            _ => source.title.clone(),
                        };
                        let response = ui.add_enabled(
                            source.exists(),
                            egui::Button::selectable(selected, label).truncate(),
                        );
                        let tip = match (source.size, source.modified) {
                            (Some(size), Some(modified)) => format!(
                                "{}\n{} · {}",
                                source.path.display(),
                                human_size(size),
                                age(modified)
                            ),
                            _ => format!("{}\nnot found", source.path.display()),
                        };
                        if response.on_hover_text(tip).clicked() {
                            to_open = Some((source.path.clone(), source.title.clone()));
                        }
                    }
                    ui.add_space(8.0);
                }

                if let Some(c) = &self.current
                    && !self.sources.iter().any(|s| s.path == c.path)
                {
                    ui.label(RichText::new("OPENED FILE").small().strong().weak());
                    let _ = ui.add(egui::Button::selectable(true, &c.title).truncate());
                    ui.add_space(8.0);
                }

                ui.label(RichText::new("SAVED SESSIONS").small().strong().weak());
                if self.sessions.is_empty() {
                    ui.label(
                        RichText::new("none yet (Ctrl+S saves one)")
                            .weak()
                            .italics(),
                    );
                }
                for session in &self.sessions {
                    let selected = current_path.as_deref() == Some(session.path.as_path());
                    let response =
                        ui.add(egui::Button::selectable(selected, &session.title).truncate());
                    let mut tip = session.path.display().to_string();
                    if !session.tags.is_empty() {
                        tip.push_str(&format!("\ntags: {}", session.tags.join(", ")));
                    }
                    if let Some(first) = session.note.lines().next() {
                        tip.push_str(&format!("\n{first}"));
                    }
                    if response.on_hover_text(tip).clicked() {
                        to_open = Some((session.path.clone(), session.title.clone()));
                    }
                }
                ui.add_space(8.0);

                if ui.button("📂 Open file…").clicked() {
                    self.open_dialog(ui.ctx());
                }
                if ui
                    .button("⟳ Rescan")
                    .on_hover_text("Look for new logs")
                    .clicked()
                {
                    self.sources = discover_sources();
                }

                if let Some(c) = &self.current {
                    ui.add_space(12.0);
                    ui.label(RichText::new("CATEGORIES").small().strong().weak());
                    let mut per_category: BTreeMap<Category, usize> = BTreeMap::new();
                    for entry in c.doc.entries() {
                        *per_category.entry(entry.category).or_default() += 1;
                    }
                    for (category, count) in per_category {
                        let mut visible = !self.filter.hidden_categories.contains(&category);
                        let label = if category.is_noise() {
                            format!("{category} ({count}) · noise")
                        } else {
                            format!("{category} ({count})")
                        };
                        if ui.checkbox(&mut visible, label).changed() {
                            if visible {
                                self.filter.hidden_categories.remove(&category);
                            } else {
                                self.filter.hidden_categories.insert(category);
                            }
                        }
                    }
                }
            });

        if let Some((path, title)) = to_open {
            self.open(path, title, ui.ctx());
        }
    }

    fn filter_bar(&mut self, ui: &mut Ui) {
        let Some(c) = &self.current else {
            return;
        };
        let counts = *c.doc.counts();
        ui.horizontal_wrapped(|ui| {
            let search = egui::TextEdit::singleline(&mut self.filter.search)
                .id(egui::Id::unique(SEARCH_ID))
                .hint_text("🔍 Search messages, files, stack frames…")
                .desired_width(320.0);
            let response = ui.add(search);
            if self.focus_search {
                response.request_focus();
                self.focus_search = false;
            }
            if !self.filter.search.is_empty()
                && ui.small_button("✖").on_hover_text("Clear search").clicked()
            {
                self.filter.search.clear();
            }
            ui.separator();
            let visuals = ui.visuals().clone();
            for (flag, severity, count, tip) in [
                (
                    &mut self.filter.show_errors,
                    Severity::Error,
                    counts.errors(),
                    "errors, exceptions and crashes",
                ),
                (
                    &mut self.filter.show_warnings,
                    Severity::Warning,
                    counts.warning,
                    "warnings",
                ),
                (
                    &mut self.filter.show_info,
                    Severity::Info,
                    counts.info,
                    "info messages",
                ),
            ] {
                let text = RichText::new(format!("{} {count}", severity_icon(severity)))
                    .color(severity_color(&visuals, severity));
                ui.toggle_value(flag, text)
                    .on_hover_text(format!("Show {tip}"));
            }
            ui.separator();
            ui.checkbox(&mut self.filter.collapse_duplicates, "Collapse duplicates")
                .on_hover_text("Merge repeated messages within a section (×N)");
            ui.checkbox(&mut self.filter.show_noise, "Engine noise")
                .on_hover_text("Show licensing, package manager, Mono and other engine chatter");
        });
    }

    fn entry_list(&mut self, ui: &mut Ui) {
        let Some(current) = &self.current else {
            self.empty_state(ui);
            return;
        };
        if self.rows.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Nothing matches the current filters.").weak());
            });
            return;
        }

        let row_height = ui.text_style_height(&TextStyle::Body) + 8.0;
        let spacing = ui.spacing().item_spacing.y;
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("entries")
            .auto_shrink([false, false])
            // While live, stay at the bottom as lines arrive (unless scrolled up).
            .stick_to_bottom(self.live.is_some());
        if let Some(section) = self.scroll_to_section.take()
            && let Some(row) = self
                .rows
                .iter()
                .position(|r| matches!(r, Row::Section { id, .. } if *id == section))
        {
            let target = row as f32 * (row_height + spacing) - ui.available_height() / 4.0;
            scroll = scroll.vertical_scroll_offset(target.max(0.0));
        }
        if self.scroll_to_selected {
            self.scroll_to_selected = false;
            if let Some(row) = self
                .selected
                .and_then(|id| row_of_entry(&self.rows, &current.doc, id))
            {
                let target = row as f32 * (row_height + spacing) - ui.available_height() / 3.0;
                scroll = scroll.vertical_scroll_offset(target.max(0.0));
            }
        }

        let mut clicked_entry: Option<u64> = None;
        let mut double_clicked: Option<u64> = None;
        let mut toggled_section: Option<u32> = None;
        let selected = self.selected;
        let selected_fingerprint = selected
            .and_then(|id| current.doc.entry(id))
            .map(|e| (e.fingerprint, e.section_id));

        scroll.show_rows(ui, row_height, self.rows.len(), |ui, range| {
            for row in &self.rows[range] {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), row_height), Sense::click());
                match *row {
                    Row::Section { id, depth } => {
                        let Some(section) = current.doc.section(id) else {
                            continue;
                        };
                        paint_section_row(
                            ui,
                            rect,
                            &response,
                            section,
                            depth,
                            self.collapsed.contains(&id),
                        );
                        if response.clicked() {
                            toggled_section = Some(id);
                        }
                    }
                    Row::Entry { id, depth, repeats } => {
                        let Some(entry) = current.doc.entry(id) else {
                            continue;
                        };
                        let is_selected = selected == Some(id)
                            || (repeats > 1
                                && selected_fingerprint
                                    == Some((entry.fingerprint, entry.section_id)));
                        paint_entry_row(ui, rect, &response, entry, depth, repeats, is_selected);
                        if response.double_clicked() {
                            double_clicked = Some(id);
                        }
                        if response.clicked() {
                            clicked_entry = Some(id);
                        }
                    }
                }
            }
        });

        if let Some(id) = toggled_section {
            if !self.collapsed.remove(&id) {
                self.collapsed.insert(id);
            }
            self.rows_dirty = true;
        }
        if let Some(id) = clicked_entry {
            self.selected = Some(id);
        }
        if let Some(id) = double_clicked {
            self.selected = Some(id);
            self.open_selected();
        }
    }

    fn empty_state(&self, ui: &mut Ui) {
        ui.add_space(80.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Comte Logger").size(28.0).strong());
            ui.label(RichText::new("« Savoir pour prévoir »").italics().weak());
            ui.add_space(16.0);
            if self.loader.pending().is_some() {
                ui.spinner();
            } else {
                ui.label("Pick a log on the left, press Ctrl+O, or drop a log file here.");
            }
        });
    }

    fn details_panel(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            return;
        }
        let mut jump = None;
        let mut back = false;
        let mut open: Option<comte_core::SourceRef> = None;
        let links = Links {
            roots: self.project_roots(),
            ide: self.ide_name(),
        };
        let Some(current) = &self.current else {
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("details")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                match self.selected.and_then(|id| current.doc.entry(id)) {
                    Some(entry) => {
                        let repeats = row_of_entry(&self.rows, &current.doc, entry.id)
                            .and_then(|row| match self.rows[row] {
                                Row::Entry { repeats, .. } => Some(repeats),
                                Row::Section { .. } => None,
                            })
                            .unwrap_or(1);
                        if ui
                            .small_button("← Overview")
                            .on_hover_text("Back to the overview (Esc)")
                            .clicked()
                        {
                            back = true;
                        }
                        ui.add_space(4.0);
                        open = entry_details(ui, current, entry, repeats, &links);
                    }
                    None => jump = overview(ui, current),
                }
            });
        if back {
            self.select(None);
        }
        if let Some(location) = open {
            self.open_location(&location);
        }
        match jump {
            Some(Jump::Entry(id)) => self.reveal(id),
            Some(Jump::Section(id)) => self.reveal_section(id),
            None => {}
        }
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("drop-overlay"),
        ));
        let rect = ctx.content_rect();
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(170));
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Drop a log file to open it",
            FontId::proportional(24.0),
            Color32::WHITE,
        );
    }
}

impl eframe::App for ComteApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_live(&ctx);
        if let Some(event) = self.loader.poll() {
            self.on_loaded(event, &ctx);
        }
        self.handle_input(&ctx);
        let filter_before = self.filter.clone();

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        // Panels remember the width their content used last frame; claiming
        // the full width keeps them from shrinking while nothing is loaded.
        egui::Panel::left("sources")
            .resizable(true)
            .default_size(250.0)
            .min_size(180.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                self.sources_panel(ui);
            });
        egui::Panel::right("details")
            .resizable(true)
            .default_size(440.0)
            .min_size(300.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                self.details_panel(ui);
            });
        let mut compare_action = None;
        if let (Some(compare), Some(current)) = (&mut self.compare, &self.current) {
            compare.update(current);
            egui::CentralPanel::default_margins().show(ui, |ui| {
                compare_action = compare_ui(ui, compare, current, &self.sources, &self.sessions);
            });
            if compare.loading() || compare.result.is_none() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        } else {
            egui::CentralPanel::default_margins().show(ui, |ui| {
                self.filter_bar(ui);
                ui.separator();
                if self.rows_dirty || self.filter != filter_before {
                    self.rebuild_rows();
                }
                self.entry_list(ui);
            });
        }
        if let Some(action) = compare_action {
            self.on_compare_action(action, &ctx);
        }

        if self.filter != filter_before || self.rows_dirty {
            self.rebuild_rows();
        }
        self.save_dialog_ui(&ctx);
        self.settings_window(&ctx);
        self.persist_settings();
        self.report_dialog_ui(&ctx);
        self.drop_overlay(&ctx);
        self.dev.after_frame(
            &ctx,
            self.current.is_some() && self.loader.pending().is_none(),
        );
    }
}

// -- row painting ------------------------------------------------------------

fn paint_section_row(
    ui: &Ui,
    rect: egui::Rect,
    response: &egui::Response,
    section: &Section,
    depth: u16,
    collapsed: bool,
) {
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect);
    let fill = if response.hovered() {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        visuals.faint_bg_color
    };
    painter.rect_filled(rect, 3.0, fill);

    let font = FontId::proportional(14.0);
    let mut x = rect.left() + 6.0 + f32::from(depth) * INDENT;
    let y = rect.center().y;
    let arrow = if collapsed { "▶" } else { "▼" };
    painter.text(
        egui::pos2(x, y),
        Align2::LEFT_CENTER,
        arrow,
        FontId::proportional(10.0),
        visuals.weak_text_color(),
    );
    x += 16.0;
    let title = painter.text(
        egui::pos2(x, y),
        Align2::LEFT_CENTER,
        &section.title,
        font.clone(),
        visuals.strong_text_color(),
    );
    x = title.right() + 10.0;
    if let Some(duration) = section.duration {
        let d = painter.text(
            egui::pos2(x, y),
            Align2::LEFT_CENTER,
            format_duration(duration),
            FontId::proportional(12.5),
            visuals.weak_text_color(),
        );
        x = d.right() + 10.0;
    }
    for (severity, count) in [
        (Severity::Error, section.counts.errors()),
        (Severity::Warning, section.counts.warning),
    ] {
        if count > 0 {
            let r = painter.text(
                egui::pos2(x, y),
                Align2::LEFT_CENTER,
                format!("{} {count}", severity_icon(severity)),
                FontId::proportional(12.5),
                severity_color(visuals, severity),
            );
            x = r.right() + 8.0;
        }
    }
    painter.text(
        egui::pos2(rect.right() - 8.0, y),
        Align2::RIGHT_CENTER,
        format!(
            "L{}–{}",
            section.line_range.start,
            section.line_range.end.saturating_sub(1)
        ),
        FontId::proportional(11.5),
        visuals.weak_text_color(),
    );
}

fn paint_entry_row(
    ui: &Ui,
    rect: egui::Rect,
    response: &egui::Response,
    entry: &LogEntry,
    depth: u16,
    repeats: u32,
    selected: bool,
) {
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 3.0, visuals.selection.bg_fill);
    } else if response.hovered() {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }

    let y = rect.center().y;
    let x = rect.left() + 6.0 + f32::from(depth) * INDENT;
    painter.text(
        egui::pos2(x, y),
        Align2::LEFT_CENTER,
        severity_icon(entry.severity),
        FontId::proportional(13.0),
        severity_color(visuals, entry.severity),
    );

    // Right side, drawn first so the message can be clipped before it.
    let mut right = rect.right() - 8.0;
    if let Some(culprit) = culprit_label(entry) {
        let r = painter.text(
            egui::pos2(right, y),
            Align2::RIGHT_CENTER,
            culprit,
            FontId::proportional(12.0),
            visuals.weak_text_color(),
        );
        right = r.left() - 10.0;
    }
    if repeats > 1 {
        let galley = painter.layout_no_wrap(
            format!("×{repeats}"),
            FontId::proportional(12.0),
            visuals.strong_text_color(),
        );
        let badge = egui::Rect::from_min_size(
            egui::pos2(
                right - galley.size().x - 10.0,
                y - galley.size().y / 2.0 - 1.0,
            ),
            galley.size() + vec2(10.0, 2.0),
        );
        painter.rect_filled(badge, 8.0, visuals.widgets.inactive.bg_fill);
        painter.galley(
            badge.min + vec2(5.0, 1.0),
            galley,
            visuals.strong_text_color(),
        );
        right = badge.left() - 8.0;
    }

    let text_color = if entry.severity >= Severity::Warning || entry.category == Category::UserLog {
        visuals.strong_text_color()
    } else if entry.is_engine_chatter() {
        visuals.weak_text_color()
    } else {
        visuals.text_color()
    };
    let message_rect = egui::Rect::from_min_max(
        egui::pos2(x + 22.0, rect.top()),
        egui::pos2(right.max(x + 60.0), rect.bottom()),
    );
    painter.with_clip_rect(message_rect).text(
        egui::pos2(message_rect.left(), y),
        Align2::LEFT_CENTER,
        &entry.message,
        FontId::proportional(13.5),
        text_color,
    );
}

// -- details -----------------------------------------------------------------

/// What details need to turn source locations into "open in IDE" links.
struct Links {
    roots: Vec<PathBuf>,
    /// "Rider", "VS Code", …
    ide: String,
}

impl Links {
    /// A location as a link when its file exists in the project, else as
    /// plain text. Returns `true` when clicked.
    fn location(&self, ui: &mut Ui, location: &comte_core::SourceRef, text: String) -> bool {
        if resolve_source(&location.path, &self.roots).is_some() {
            ui.link(RichText::new(text).monospace())
                .on_hover_text(format!("Open in {} (Ctrl+E)", self.ide))
                .clicked()
        } else {
            ui.label(RichText::new(text).monospace().weak())
                .on_hover_text("Not found in the Unity project folder (choose it in ⚙)");
            false
        }
    }
}

/// Entry details. Returns a source location the user asked to open.
fn entry_details(
    ui: &mut Ui,
    current: &Loaded,
    entry: &LogEntry,
    repeats: u32,
    links: &Links,
) -> Option<comte_core::SourceRef> {
    let mut open = None;
    let color = severity_color(ui.visuals(), entry.severity);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(severity_icon(entry.severity))
                .color(color)
                .size(18.0),
        );
        ui.label(RichText::new(entry.severity.label()).color(color).strong());
        ui.label(RichText::new(format!("· {}", entry.category)).weak());
    });
    ui.add_space(4.0);
    ui.label(RichText::new(&entry.message).size(15.0).strong());
    if let Some(culprit) = entry.culprit()
        && resolve_source(&culprit.path, &links.roots).is_some()
        && ui
            .button(format!("↗ Open {} in {}", culprit.short(), links.ide))
            .on_hover_text("Ctrl+E, or double-click the entry")
            .clicked()
    {
        open = Some(culprit.clone());
    }
    if repeats > 1 {
        ui.label(RichText::new(format!("Repeated {repeats} times in this section")).weak());
    }
    ui.add_space(8.0);

    egui::Grid::new("entry-facts")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            if let Some(culprit) = entry.culprit() {
                ui.label(RichText::new("Where").weak());
                if links.location(ui, culprit, culprit.to_string()) {
                    open = Some(culprit.clone());
                }
                ui.end_row();
            }
            if let Some(source) = &entry.source
                && Some(source) != entry.culprit()
            {
                ui.label(RichText::new("Logged at").weak());
                ui.label(RichText::new(source.to_string()).monospace());
                ui.end_row();
            }
            let path: Vec<String> = section_path(&current.doc, entry.section_id)
                .into_iter()
                .filter_map(|id| current.doc.section(id).map(|s| s.title.clone()))
                .collect();
            if !path.is_empty() {
                ui.label(RichText::new("During").weak());
                ui.label(path.join(" › "));
                ui.end_row();
            }
            ui.label(RichText::new("Lines").weak());
            ui.label(format!(
                "{}–{}",
                entry.line_range.start,
                entry.line_range.end.saturating_sub(1)
            ));
            ui.end_row();
        });

    if !entry.details.is_empty() {
        ui.add_space(8.0);
        egui::CollapsingHeader::new(format!("Details ({} lines)", entry.details.len()))
            .id_salt(("details", entry.id))
            .default_open(entry.details.len() <= 12)
            .show(ui, |ui| {
                for line in entry.details.iter().take(300) {
                    ui.label(RichText::new(line).monospace().small());
                }
            });
    }

    if !entry.stack.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Stack trace").strong());
        if let Some(location) = stack_trace(ui, entry, links) {
            open = Some(location);
        }
    }

    ui.add_space(8.0);
    egui::CollapsingHeader::new("Original log lines")
        .id_salt(("raw", entry.id))
        .default_open(entry.stack.is_empty() && entry.details.is_empty())
        .show(ui, |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                let lines = current.raw_lines(entry.line_range.clone());
                for (offset, line) in lines.iter().take(500).enumerate() {
                    ui.label(
                        RichText::new(format!("{:>6}  {line}", entry.line_range.start + offset))
                            .monospace()
                            .small(),
                    );
                }
            });
        });
    open
}

/// User frames in bold; runs of engine/package frames folded. Locations of
/// files in the project are links. Returns a location the user clicked.
fn stack_trace(ui: &mut Ui, entry: &LogEntry, links: &Links) -> Option<comte_core::SourceRef> {
    let mut open = None;
    let frames = &entry.stack;
    let mut i = 0;
    while i < frames.len() {
        if frames[i].kind == FrameKind::User {
            let frame = &frames[i];
            ui.label(RichText::new(&frame.text).strong().monospace())
                .on_hover_text(&frame.raw);
            if let Some(location) = &frame.location {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    if links.location(ui, location, location.to_string()) {
                        open = Some(location.clone());
                    }
                });
            }
            i += 1;
            continue;
        }
        let run_end = frames[i..]
            .iter()
            .position(|f| f.kind == FrameKind::User)
            .map_or(frames.len(), |p| i + p);
        let run = &frames[i..run_end];
        // Package frames (e.g. Library/PackageCache/…) can be opened too.
        let show_frame = |ui: &mut Ui, frame: &comte_core::StackFrame| {
            let kind = match frame.kind {
                FrameKind::Package => "pkg",
                FrameKind::Native => "native",
                _ => "engine",
            };
            ui.label(
                RichText::new(format!("{}  ({kind})", frame.text))
                    .weak()
                    .monospace(),
            )
            .on_hover_text(&frame.raw);
            let location = frame
                .location
                .as_ref()
                .filter(|_| frame.kind == FrameKind::Package)?;
            let clicked = ui
                .horizontal(|ui| {
                    ui.add_space(24.0);
                    links.location(ui, location, location.to_string())
                })
                .inner;
            clicked.then(|| location.clone())
        };
        if run.len() == 1 {
            if let Some(location) = show_frame(ui, &run[0]) {
                open = Some(location);
            }
        } else {
            let clicked = egui::CollapsingHeader::new(
                RichText::new(format!("{} engine / package frames", run.len())).weak(),
            )
            .id_salt(("frames", entry.id, i))
            .default_open(false)
            .show(ui, |ui| run.iter().filter_map(|f| show_frame(ui, f)).last())
            .body_returned
            .flatten();
            if clicked.is_some() {
                open = clicked;
            }
        }
        i = run_end;
    }
    open
}

fn file_title(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `Documents\ComteLogger\sessions`, or the folder chosen in settings.
fn sessions_dir(settings: &Settings) -> PathBuf {
    settings.sessions_dir.clone().unwrap_or_else(default_dir)
}

fn apply_theme(ctx: &egui::Context, theme: ThemeChoice) {
    ctx.set_theme(match theme {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
    });
}
