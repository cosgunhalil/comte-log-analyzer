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
use crate::devtools::DevHooks;
use crate::engines::discover_sources;
use crate::live::{LiveEvent, LiveTail};
use crate::loader::{LoadEvent, Loaded, Loader};
use crate::overview::{Jump, overview};
use crate::save::{
    self, ContentChoice, Prepared, SaveRequest, SessionFile, current_user_name, default_dir,
    default_file_name, discover_sessions, visible_lines,
};
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

const NOTICE_SECONDS: u64 = 8;

impl ComteApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let dev = DevHooks::from_env();
        let mut filter = Filter {
            show_noise: dev.show_noise,
            ..Filter::default()
        };
        if let Some(search) = &dev.search {
            filter.search = search.clone();
        }
        let mut app = Self {
            sources: discover_sources(),
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
            extra_redactions: String::new(),
            sessions: discover_sessions(&default_dir()),
            notice: None,
            live: None,
            live_activity: None,
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
                self.sessions = discover_sessions(&default_dir());
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
        let dir = default_dir();
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
                ui.label(RichText::new(format!("to {}", default_dir().display())).weak().small());
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
        if self.save_dialog.is_some() {
            return; // the dialog owns the keyboard (Esc closes it)
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
            self.select(None);
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
        let Some(current) = &self.current else {
            return;
        };
        let mut jump = None;
        let mut back = false;
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
                        entry_details(ui, current, entry, repeats);
                    }
                    None => jump = overview(ui, current),
                }
            });
        if back {
            self.select(None);
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
        egui::CentralPanel::default_margins().show(ui, |ui| {
            self.filter_bar(ui);
            ui.separator();
            if self.rows_dirty || self.filter != filter_before {
                self.rebuild_rows();
            }
            self.entry_list(ui);
        });

        if self.filter != filter_before || self.rows_dirty {
            self.rebuild_rows();
        }
        self.save_dialog_ui(&ctx);
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

fn entry_details(ui: &mut Ui, current: &Loaded, entry: &LogEntry, repeats: u32) {
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
                ui.label(RichText::new(culprit.to_string()).monospace());
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
        stack_trace(ui, entry);
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
}

/// User frames in bold; runs of engine/package frames folded.
fn stack_trace(ui: &mut Ui, entry: &LogEntry) {
    let frames = &entry.stack;
    let mut i = 0;
    while i < frames.len() {
        if frames[i].kind == FrameKind::User {
            let frame = &frames[i];
            ui.label(RichText::new(&frame.text).strong().monospace())
                .on_hover_text(&frame.raw);
            if let Some(location) = &frame.location {
                ui.label(
                    RichText::new(format!("    {location}"))
                        .weak()
                        .small()
                        .monospace(),
                );
            }
            i += 1;
            continue;
        }
        let run_end = frames[i..]
            .iter()
            .position(|f| f.kind == FrameKind::User)
            .map_or(frames.len(), |p| i + p);
        let run = &frames[i..run_end];
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
        };
        if run.len() == 1 {
            show_frame(ui, &run[0]);
        } else {
            egui::CollapsingHeader::new(
                RichText::new(format!("{} engine / package frames", run.len())).weak(),
            )
            .id_salt(("frames", entry.id, i))
            .default_open(false)
            .show(ui, |ui| {
                for frame in run {
                    show_frame(ui, frame);
                }
            });
        }
        i = run_end;
    }
}

fn file_title(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
