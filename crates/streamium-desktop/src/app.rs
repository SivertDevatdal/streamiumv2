//! The interface.
//!
//! Everything slow happens on a worker thread and reports back through a
//! channel; `update` never blocks. The state kept here is deliberately plain:
//! a catalogue, a guide, and the few things the user has selected.

use std::collections::HashMap;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Local, TimeZone};
use egui::{Color32, RichText};

use streamium_core::catalog::Catalog;
use streamium_core::epg::{EpgIndex, Programme};
use streamium_core::model::{Channel, MediaKind, Source};
use streamium_core::xtream::models::AccountInfo;

use crate::analyse::{self, Report, Verdict};
use crate::config::{Config, SourceEntry};
use crate::load::{self, LoadMsg, LoadStats, Loaded};
use crate::net::DEFAULT_USER_AGENT;
use crate::player::{self, Player};

/// Unix seconds, which is what the guide is indexed by.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn clock(unix: i64) -> String {
    match Local.timestamp_opt(unix, 0) {
        chrono::LocalResult::Single(t) => t.format("%H:%M").to_string(),
        _ => "--:--".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Filter {
    All,
    Favourites,
    Kind(MediaKind),
    Group(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Xtream,
    Playlist,
    Folder,
}

/// The "add a source" form.
#[derive(Debug, Clone)]
struct AddForm {
    kind: SourceKind,
    name: String,
    server: String,
    username: String,
    password: String,
    playlist_url: String,
    folder: String,
    epg_url: String,
    include_vod: bool,
    error: Option<String>,
}

impl Default for AddForm {
    fn default() -> Self {
        AddForm {
            kind: SourceKind::Xtream,
            name: String::new(),
            server: String::new(),
            username: String::new(),
            password: String::new(),
            playlist_url: String::new(),
            folder: String::new(),
            epg_url: String::new(),
            include_vod: false,
            error: None,
        }
    }
}

enum Status {
    Idle,
    Loading(String),
    Error(String),
}

enum AnalyseMsg {
    Done(Box<Report>),
    Failed(String),
}

pub struct App {
    config: Config,
    catalog: Catalog,
    epg: EpgIndex,
    account: Option<AccountInfo>,
    stats: LoadStats,
    warnings: Vec<String>,
    /// Channel id → index into `catalog.channels()`.
    index_by_id: HashMap<String, usize>,
    /// Channel id → guide channel id, resolved once per load.
    epg_ids: HashMap<String, Option<String>>,

    status: Status,
    load_rx: Option<Receiver<LoadMsg>>,

    analyse_rx: Option<Receiver<AnalyseMsg>>,
    analyse_stop: Arc<AtomicBool>,
    analysing: Option<String>,
    report: Option<Report>,

    filter: Filter,
    search: String,
    visible: Vec<usize>,
    list_dirty: bool,
    selected: Option<String>,

    players: Vec<Player>,
    children: Vec<(String, Child)>,
    log: Vec<String>,

    show_add: bool,
    show_settings: bool,
    show_report: bool,
    show_log: bool,
    show_help: bool,
    add_form: AddForm,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> App {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let config = Config::load();
        let mut app = App {
            config,
            catalog: Catalog::new(),
            epg: EpgIndex::new(),
            account: None,
            stats: LoadStats::default(),
            warnings: Vec::new(),
            index_by_id: HashMap::new(),
            epg_ids: HashMap::new(),
            status: Status::Idle,
            load_rx: None,
            analyse_rx: None,
            analyse_stop: Arc::new(AtomicBool::new(false)),
            analysing: None,
            report: None,
            filter: Filter::All,
            search: String::new(),
            visible: Vec::new(),
            list_dirty: true,
            selected: None,
            players: player::detect(),
            children: Vec::new(),
            log: Vec::new(),
            show_add: false,
            show_settings: false,
            show_report: false,
            show_log: false,
            show_help: false,
            add_form: AddForm::default(),
        };
        app.note(format!(
            "Found {} player(s): {}",
            app.players.len(),
            if app.players.is_empty() {
                "none".to_string()
            } else {
                app.players
                    .iter()
                    .map(|p| p.display_name())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        if app.config.sources.is_empty() {
            app.show_add = true;
        } else if app.config.refresh_on_launch {
            if app.config.last_source.is_none() {
                app.config.last_source = Some(0);
            }
            app.start_load(&cc.egui_ctx.clone());
        }
        app
    }

    fn note(&mut self, line: String) {
        let stamped = format!("{}  {line}", clock(now()));
        self.log.push(stamped);
        if self.log.len() > 500 {
            self.log.remove(0);
        }
    }

    fn user_agent(&self) -> String {
        DEFAULT_USER_AGENT.to_string()
    }

    /// The player the user chose, or the best one we found.
    fn player(&self) -> Option<Player> {
        let configured = self.config.player_path.trim();
        if !configured.is_empty() {
            return Some(Player::at(configured));
        }
        self.players.first().cloned()
    }

    // ---------- loading ----------

    fn start_load(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.config.selected().cloned() else {
            return;
        };
        if self.load_rx.is_some() {
            return;
        }
        let (tx, rx): (Sender<LoadMsg>, Receiver<LoadMsg>) = channel();
        self.load_rx = Some(rx);
        self.status = Status::Loading("Starting…".to_string());
        self.warnings.clear();
        self.note(format!("Loading {}", entry.name()));

        let user_agent = self.user_agent();
        let prefer_hls = self.config.prefer_hls;
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("streamium-load".into())
            .spawn(move || {
                load::load(&entry, &user_agent, prefer_hls, &tx);
                ctx.request_repaint();
            })
            .expect("spawn loader");
    }

    fn poll_workers(&mut self, ctx: &egui::Context) {
        // Drain first, act second: the receivers are fields of `self`, and
        // handling a message needs `self` mutably.
        let mut load_msgs = Vec::new();
        let mut load_finished = false;
        if let Some(rx) = &self.load_rx {
            while let Ok(msg) = rx.try_recv() {
                if matches!(msg, LoadMsg::Done(_) | LoadMsg::Failed(_)) {
                    load_finished = true;
                }
                load_msgs.push(msg);
            }
        }
        for msg in load_msgs {
            match msg {
                LoadMsg::Stage(stage) => self.status = Status::Loading(stage),
                LoadMsg::Done(loaded) => self.apply(*loaded),
                LoadMsg::Failed(e) => {
                    self.note(format!("Load failed: {e}"));
                    self.status = Status::Error(e);
                }
            }
        }
        if load_finished {
            self.load_rx = None;
        }

        let mut analyse_msgs = Vec::new();
        if let Some(rx) = &self.analyse_rx {
            while let Ok(msg) = rx.try_recv() {
                analyse_msgs.push(msg);
            }
        }
        let analysis_done = !analyse_msgs.is_empty();
        for msg in analyse_msgs {
            match msg {
                AnalyseMsg::Done(report) => {
                    self.note(format!(
                        "Analysed {}: {}",
                        report.channel,
                        report.verdict.headline()
                    ));
                    self.report = Some(*report);
                    self.show_report = true;
                }
                AnalyseMsg::Failed(e) => {
                    self.note(format!("Analysis failed: {e}"));
                    self.status = Status::Error(e);
                }
            }
        }
        if analysis_done {
            self.analyse_rx = None;
            self.analysing = None;
        }

        // Reap players that have exited so the status line stays honest.
        self.children
            .retain_mut(|(_, child)| !matches!(child.try_wait(), Ok(Some(_)) | Err(_)));

        if self.load_rx.is_some() || self.analyse_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn apply(&mut self, loaded: Loaded) {
        let Loaded {
            catalog,
            epg,
            account,
            warnings,
            stats,
        } = loaded;
        self.index_by_id = catalog
            .channels()
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();
        self.epg_ids = catalog
            .channels()
            .iter()
            .map(|c| {
                let resolved = epg.resolve(c.epg_id.as_deref(), &c.name);
                (c.id.clone(), resolved)
            })
            .collect();
        self.catalog = catalog;
        self.epg = epg;
        self.account = account;
        self.warnings = warnings;
        self.stats = stats.clone();
        self.status = Status::Idle;
        self.list_dirty = true;
        self.selected = None;
        self.note(format!(
            "Loaded {} channels in {} groups, {} guide entries, in {} ms",
            stats.channels, stats.groups, stats.programmes, stats.elapsed_ms
        ));
    }

    fn rebuild_list(&mut self) {
        let query = self.search.trim();
        self.visible = if !query.is_empty() {
            self.catalog
                .search(query, 2000)
                .into_iter()
                .filter_map(|hit| self.index_by_id.get(&hit.channel.id).copied())
                .filter(|i| self.passes_filter(*i))
                .collect()
        } else {
            (0..self.catalog.channels().len())
                .filter(|i| self.passes_filter(*i))
                .collect()
        };
        self.list_dirty = false;
    }

    fn passes_filter(&self, index: usize) -> bool {
        let Some(channel) = self.catalog.channels().get(index) else {
            return false;
        };
        match &self.filter {
            Filter::All => true,
            Filter::Favourites => self.catalog.is_favourite(&channel.id),
            Filter::Kind(kind) => channel.kind == *kind,
            Filter::Group(group) => channel.group.as_deref() == Some(group.as_str()),
        }
    }

    fn now_next(&self, channel: &Channel) -> (Option<&Programme>, Option<&Programme>) {
        match self.epg_ids.get(&channel.id).and_then(|o| o.as_deref()) {
            Some(epg_id) => self.epg.now_next(epg_id, now()),
            None => (None, None),
        }
    }

    // ---------- actions ----------

    fn play(&mut self, index: usize) {
        let Some(channel) = self.catalog.channels().get(index).cloned() else {
            return;
        };
        let Some(player) = self.player() else {
            self.status = Status::Error(
                "No video player found. Install mpv or VLC, or set the path in Settings."
                    .to_string(),
            );
            self.show_settings = true;
            return;
        };
        if !player.path.is_file() {
            self.status = Status::Error(format!(
                "{} is not a file. Check the player path in Settings.",
                player.path.display()
            ));
            self.show_settings = true;
            return;
        }
        let user_agent = self.user_agent();
        let args = player::arguments(
            player.kind,
            &channel.url,
            &channel.name,
            &channel.http,
            &user_agent,
        );
        if !channel.http.headers.is_empty() && !player.kind.supports_headers() {
            self.note(format!(
                "{} cannot take custom HTTP headers on the command line; {} sets {} that \
                 will not be sent. Use mpv if the stream refuses to open.",
                player.display_name(),
                channel.name,
                channel.http.headers.len()
            ));
        }
        match player::launch(
            &player,
            &channel.url,
            &channel.name,
            &channel.http,
            &user_agent,
        ) {
            Ok(child) => {
                self.children.push((channel.name.clone(), child));
                self.note(format!(
                    "Playing {} in {}",
                    channel.name,
                    player.display_name()
                ));
                self.status = Status::Idle;
            }
            Err(e) => {
                let line = player::command_line(&player, &args);
                self.note(format!("Launch failed: {e} — {line}"));
                self.status = Status::Error(format!(
                    "{} would not start: {e}. The command was: {line}",
                    player.display_name()
                ));
            }
        }
    }

    fn analyse(&mut self, index: usize, ctx: &egui::Context) {
        let Some(channel) = self.catalog.channels().get(index).cloned() else {
            return;
        };
        if self.analyse_rx.is_some() {
            return;
        }
        let (tx, rx) = channel_pair();
        self.analyse_rx = Some(rx);
        self.analysing = Some(channel.name.clone());
        self.analyse_stop = Arc::new(AtomicBool::new(false));
        self.note(format!("Analysing {}", channel.name));

        let stop = Arc::clone(&self.analyse_stop);
        let user_agent = self.user_agent();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("streamium-analyse".into())
            .spawn(move || {
                let result = analyse::run(
                    &channel.name,
                    &channel.url,
                    &channel.http,
                    &user_agent,
                    &stop,
                );
                let _ = match result {
                    Ok(report) => tx.send(AnalyseMsg::Done(Box::new(report))),
                    Err(e) => tx.send(AnalyseMsg::Failed(e)),
                };
                ctx.request_repaint();
            })
            .expect("spawn analyser");
    }

    fn toggle_favourite(&mut self, index: usize) {
        let Some(channel) = self.catalog.channels().get(index) else {
            return;
        };
        let id = channel.id.clone();
        let on = !self.catalog.is_favourite(&id);
        self.catalog.set_favourite(&id, on);
        if let Some(i) = self.config.last_source {
            if let Some(entry) = self.config.sources.get_mut(i) {
                entry.favourites.retain(|f| f != &id);
                if on {
                    entry.favourites.push(id);
                }
            }
        }
        self.save();
        if self.filter == Filter::Favourites {
            self.list_dirty = true;
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.config.save() {
            self.note(format!("Settings could not be saved: {e}"));
        }
    }

    fn add_source(&mut self) {
        let form = self.add_form.clone();
        let name = form.name.trim().to_string();
        let source = match form.kind {
            SourceKind::Xtream => {
                if form.server.trim().is_empty() {
                    self.add_form.error = Some("Enter the server address.".into());
                    return;
                }
                // Validate the way the core will, before storing anything.
                if let Err(e) = streamium_core::xtream::Endpoints::new(
                    form.server.trim(),
                    form.username.trim(),
                    &form.password,
                ) {
                    self.add_form.error = Some(e.to_string());
                    return;
                }
                Source::Xtream {
                    name: if name.is_empty() {
                        form.server.trim().to_string()
                    } else {
                        name
                    },
                    base_url: form.server.trim().to_string(),
                    username: form.username.trim().to_string(),
                    password: form.password.clone(),
                }
            }
            SourceKind::Playlist => {
                if form.playlist_url.trim().is_empty() {
                    self.add_form.error = Some("Enter a playlist address or file path.".into());
                    return;
                }
                Source::Playlist {
                    name: if name.is_empty() {
                        "Playlist".to_string()
                    } else {
                        name
                    },
                    url: form.playlist_url.trim().to_string(),
                    epg_url: None,
                }
            }
            SourceKind::Folder => {
                if form.folder.trim().is_empty() {
                    self.add_form.error = Some("Enter a folder path.".into());
                    return;
                }
                Source::LocalLibrary {
                    name: if name.is_empty() {
                        "My media".to_string()
                    } else {
                        name
                    },
                    path: form.folder.trim().to_string(),
                }
            }
        };

        let mut entry = SourceEntry::new(source);
        entry.epg_url = Some(form.epg_url.trim().to_string()).filter(|s| !s.is_empty());
        entry.include_vod = form.include_vod;
        self.config.sources.push(entry);
        self.config.last_source = Some(self.config.sources.len() - 1);
        self.save();
        self.add_form = AddForm::default();
        self.show_add = false;
    }
}

fn channel_pair() -> (Sender<AnalyseMsg>, Receiver<AnalyseMsg>) {
    channel()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_workers(ctx);
        if self.list_dirty {
            self.rebuild_list();
        }
        // The guide moves on its own; keep progress bars and now/next honest.
        ctx.request_repaint_after(Duration::from_secs(1));

        self.top_bar(ctx);
        self.status_bar(ctx);
        self.groups_panel(ctx);
        self.details_panel(ctx);
        self.channel_list(ctx);

        self.add_window(ctx);
        self.settings_window(ctx);
        self.report_window(ctx);
        self.log_window(ctx);
        self.help_window(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.config.save();
    }
}

// ---------- panels ----------

impl App {
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Streamium");
                ui.separator();

                let selected_name = self
                    .config
                    .selected()
                    .map(|s| s.name().to_string())
                    .unwrap_or_else(|| "No source".to_string());
                let mut chosen = self.config.last_source;
                egui::ComboBox::from_id_salt("source")
                    .selected_text(selected_name)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for (i, entry) in self.config.sources.iter().enumerate() {
                            ui.selectable_value(&mut chosen, Some(i), entry.name());
                        }
                    });
                if chosen != self.config.last_source {
                    self.config.last_source = chosen;
                    self.save();
                    self.start_load(ctx);
                }

                if ui.button("Add source").clicked() {
                    self.add_form = AddForm::default();
                    self.show_add = true;
                }
                let loading = self.load_rx.is_some();
                if ui
                    .add_enabled(
                        !loading && self.config.selected().is_some(),
                        egui::Button::new("Reload"),
                    )
                    .clicked()
                {
                    self.start_load(ctx);
                }

                ui.separator();
                ui.label("Search");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("name or channel number")
                        .desired_width(220.0),
                );
                if response.changed() {
                    self.list_dirty = true;
                }
                if !self.search.is_empty() && ui.button("Clear").clicked() {
                    self.search.clear();
                    self.list_dirty = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Help").clicked() {
                        self.show_help = true;
                    }
                    if ui.button("Log").clicked() {
                        self.show_log = true;
                    }
                    if ui.button("Settings").clicked() {
                        self.show_settings = true;
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                match &self.status {
                    Status::Loading(stage) => {
                        ui.spinner();
                        ui.label(stage.clone());
                    }
                    Status::Error(message) => {
                        ui.label(
                            RichText::new(format!("⚠ {message}")).color(Color32::from_rgb(255, 120, 100)),
                        );
                    }
                    Status::Idle => {
                        if self.stats.channels > 0 {
                            ui.label(format!(
                                "{} channels · {} groups · {} guide entries for {} channels · loaded in {} ms",
                                self.stats.channels,
                                self.stats.groups,
                                self.stats.programmes,
                                self.stats.epg_channels,
                                self.stats.elapsed_ms
                            ));
                        } else {
                            ui.label("Add a source to begin.");
                        }
                    }
                }
                if let Some(name) = &self.analysing {
                    ui.separator();
                    ui.spinner();
                    ui.label(format!("Analysing {name}…"));
                    if ui.button("Stop").clicked() {
                        self.analyse_stop.store(true, Ordering::Relaxed);
                    }
                }
                if !self.children.is_empty() {
                    ui.separator();
                    let names: Vec<&str> = self.children.iter().map(|(n, _)| n.as_str()).collect();
                    ui.label(format!("▶ {}", names.join(", ")));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.warnings.is_empty() {
                        let count = self.warnings.len();
                        if ui
                            .button(
                                RichText::new(format!("{count} warning(s)"))
                                    .color(Color32::from_rgb(240, 200, 120)),
                            )
                            .clicked()
                        {
                            self.show_log = true;
                        }
                    }
                });
            });
            ui.add_space(3.0);
        });
    }

    fn groups_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("groups")
            .default_width(230.0)
            .width_range(160.0..=420.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("LIBRARY").small().weak());
                let mut filter = self.filter.clone();
                let total = self.catalog.channels().len();
                let favourites = self.catalog.favourites().count();
                if ui
                    .selectable_label(filter == Filter::All, format!("All channels  ({total})"))
                    .clicked()
                {
                    filter = Filter::All;
                }
                if ui
                    .selectable_label(
                        filter == Filter::Favourites,
                        format!("★ Favourites  ({favourites})"),
                    )
                    .clicked()
                {
                    filter = Filter::Favourites;
                }
                for (kind, label) in [
                    (MediaKind::Live, "Live TV"),
                    (MediaKind::Movie, "Movies"),
                    (MediaKind::Radio, "Radio"),
                    (MediaKind::Personal, "My media"),
                ] {
                    let count = self.catalog.of_kind(kind).count();
                    if count == 0 {
                        continue;
                    }
                    if ui
                        .selectable_label(
                            filter == Filter::Kind(kind),
                            format!("{label}  ({count})"),
                        )
                        .clicked()
                    {
                        filter = Filter::Kind(kind);
                    }
                }

                ui.add_space(8.0);
                ui.label(RichText::new("GROUPS").small().weak());
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let groups: Vec<String> = self.catalog.groups().to_vec();
                        for group in groups {
                            let selected = matches!(&filter, Filter::Group(g) if g == &group);
                            let count = self.catalog.in_group(&group).count();
                            if ui
                                .selectable_label(selected, format!("{group}  ({count})"))
                                .clicked()
                            {
                                filter = Filter::Group(group.clone());
                            }
                        }
                    });

                if filter != self.filter {
                    self.filter = filter;
                    self.list_dirty = true;
                }
            });
    }

    fn details_panel(&mut self, ctx: &egui::Context) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let Some(index) = self.index_by_id.get(&selected).copied() else {
            return;
        };
        let Some(channel) = self.catalog.channels().get(index).cloned() else {
            return;
        };

        egui::SidePanel::right("details")
            .default_width(340.0)
            .width_range(260.0..=520.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading(&channel.name);
                ui.horizontal_wrapped(|ui| {
                    if let Some(number) = channel.number {
                        ui.label(RichText::new(format!("#{number}")).weak());
                    }
                    if let Some(group) = &channel.group {
                        ui.label(RichText::new(group).weak());
                    }
                    ui.label(RichText::new(format!("{:?}", channel.format)).weak());
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("▶ Play").clicked() {
                        self.play(index);
                    }
                    let busy = self.analyse_rx.is_some();
                    if ui
                        .add_enabled(!busy, egui::Button::new("Analyse stream"))
                        .on_hover_text(
                            "Fetch the first seconds and report what the demuxer finds: \
                             programs, codecs, timing and errors.",
                        )
                        .clicked()
                    {
                        self.analyse(index, ctx);
                    }
                    let starred = self.catalog.is_favourite(&channel.id);
                    if ui.button(if starred { "★" } else { "☆" }).clicked() {
                        self.toggle_favourite(index);
                    }
                });
                ui.add_space(6.0);
                ui.label(RichText::new(analyse::redact(&channel.url)).small().weak());
                if ui.small_button("Copy address").clicked() {
                    ui.ctx().copy_text(channel.url.clone());
                }

                ui.separator();
                let epg_id = self.epg_ids.get(&channel.id).and_then(|o| o.clone());
                match epg_id {
                    None => {
                        ui.label(RichText::new("No guide data for this channel.").weak());
                    }
                    Some(epg_id) => {
                        let at = now();
                        let upcoming = self.epg.between(&epg_id, at - 3600, at + 86_400);
                        if upcoming.is_empty() {
                            ui.label(
                                RichText::new("The guide has no entries for this channel.").weak(),
                            );
                        }
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for programme in upcoming.iter().take(200) {
                                    let live = programme.is_on_air(at);
                                    ui.horizontal(|ui| {
                                        let time =
                                            RichText::new(clock(programme.start)).monospace();
                                        ui.label(if live { time.strong() } else { time.weak() });
                                        let title = RichText::new(&programme.title);
                                        ui.label(if live { title.strong() } else { title });
                                    });
                                    if live {
                                        ui.add(
                                            egui::ProgressBar::new(programme.progress(at))
                                                .desired_height(3.0),
                                        );
                                        if let Some(description) = &programme.description {
                                            ui.label(RichText::new(description).small().weak());
                                        }
                                    }
                                }
                            });
                    }
                }
            });
    }

    fn channel_list(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.catalog.channels().is_empty() {
                ui.centered_and_justified(|ui| {
                    if matches!(self.status, Status::Loading(_)) {
                        ui.label("Loading…");
                    } else {
                        ui.label("Nothing loaded. Add a source from the toolbar.");
                    }
                });
                return;
            }
            if self.visible.is_empty() {
                ui.centered_and_justified(|ui| ui.label("Nothing matches."));
                return;
            }

            let at = now();
            let row_height = 44.0;
            let rows = self.visible.len();
            let mut play = None;
            let mut star = None;
            let mut select = None;

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, row_height, rows, |ui, range| {
                    for row in range {
                        let index = self.visible[row];
                        let Some(channel) = self.catalog.channels().get(index) else {
                            continue;
                        };
                        let selected = self.selected.as_deref() == Some(channel.id.as_str());
                        let (now_on, next_on) = self.now_next(channel);

                        let stripe = if row % 2 == 0 {
                            Color32::TRANSPARENT
                        } else {
                            ui.visuals().faint_bg_color
                        };
                        let response = ui
                            .push_id(index, |ui| {
                                egui::Frame::new()
                                    .fill(if selected {
                                        ui.visuals().selection.bg_fill
                                    } else {
                                        stripe
                                    })
                                    .inner_margin(egui::Margin::symmetric(6, 4))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            let number = channel
                                                .number
                                                .map(|n| n.to_string())
                                                .unwrap_or_default();
                                            ui.add_sized(
                                                [44.0, 18.0],
                                                egui::Label::new(
                                                    RichText::new(number).monospace().weak(),
                                                ),
                                            );
                                            ui.vertical(|ui| {
                                                ui.label(RichText::new(&channel.name).strong());
                                                match now_on {
                                                    Some(programme) => {
                                                        let next = next_on
                                                            .map(|p| {
                                                                format!(
                                                                    "   then {} {}",
                                                                    clock(p.start),
                                                                    p.title
                                                                )
                                                            })
                                                            .unwrap_or_default();
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "{} {}{next}",
                                                                clock(programme.start),
                                                                programme.title
                                                            ))
                                                            .small()
                                                            .weak(),
                                                        );
                                                        ui.add(
                                                            egui::ProgressBar::new(
                                                                programme.progress(at),
                                                            )
                                                            .desired_width(220.0)
                                                            .desired_height(2.0),
                                                        );
                                                    }
                                                    None => {
                                                        let group = channel
                                                            .group
                                                            .clone()
                                                            .unwrap_or_default();
                                                        ui.label(
                                                            RichText::new(group).small().weak(),
                                                        );
                                                    }
                                                }
                                            });
                                        });
                                    })
                                    .response
                            })
                            .inner;

                        let clicked = response.interact(egui::Sense::click());
                        if clicked.clicked() {
                            select = Some(channel.id.clone());
                        }
                        if clicked.double_clicked() {
                            play = Some(index);
                        }
                        clicked.context_menu(|ui| {
                            if ui.button("Play").clicked() {
                                play = Some(index);
                                ui.close();
                            }
                            let starred = self.catalog.is_favourite(&channel.id);
                            if ui
                                .button(if starred {
                                    "Remove from favourites"
                                } else {
                                    "Add to favourites"
                                })
                                .clicked()
                            {
                                star = Some(index);
                                ui.close();
                            }
                        });
                    }
                });

            if let Some(id) = select {
                self.selected = Some(id);
            }
            if let Some(index) = star {
                self.toggle_favourite(index);
            }
            if let Some(index) = play {
                self.play(index);
            }
        });
    }
}

// ---------- windows ----------

impl App {
    fn add_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_add;
        let mut submit = false;
        egui::Window::new("Add a source")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.add_form.kind,
                        SourceKind::Xtream,
                        "Xtream account",
                    );
                    ui.selectable_value(
                        &mut self.add_form.kind,
                        SourceKind::Playlist,
                        "M3U playlist",
                    );
                    ui.selectable_value(
                        &mut self.add_form.kind,
                        SourceKind::Folder,
                        "Folder of files",
                    );
                });
                ui.separator();
                egui::Grid::new("add_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.add_form.name)
                                .hint_text("optional")
                                .desired_width(300.0),
                        );
                        ui.end_row();

                        match self.add_form.kind {
                            SourceKind::Xtream => {
                                ui.label("Server");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.add_form.server)
                                        .hint_text("http://example.com:8080")
                                        .desired_width(300.0),
                                );
                                ui.end_row();
                                ui.label("Username");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.add_form.username)
                                        .desired_width(300.0),
                                );
                                ui.end_row();
                                ui.label("Password");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.add_form.password)
                                        .password(true)
                                        .desired_width(300.0),
                                );
                                ui.end_row();
                                ui.label("Movies");
                                ui.checkbox(
                                    &mut self.add_form.include_vod,
                                    "Also load the movie catalogue (slower)",
                                );
                                ui.end_row();
                            }
                            SourceKind::Playlist => {
                                ui.label("Playlist");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.add_form.playlist_url)
                                        .hint_text("https://… or C:\\path\\to\\playlist.m3u")
                                        .desired_width(300.0),
                                );
                                ui.end_row();
                            }
                            SourceKind::Folder => {
                                ui.label("Folder");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.add_form.folder)
                                        .hint_text("C:\\Users\\you\\Videos")
                                        .desired_width(300.0),
                                );
                                ui.end_row();
                            }
                        }

                        if self.add_form.kind != SourceKind::Folder {
                            ui.label("Guide");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.add_form.epg_url)
                                    .hint_text("optional XMLTV address")
                                    .desired_width(300.0),
                            );
                            ui.end_row();
                        }
                    });

                if let Some(error) = &self.add_form.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(error).color(Color32::from_rgb(255, 120, 100)));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Add").clicked() {
                        submit = true;
                    }
                    ui.label(
                        RichText::new(
                            "Streamium ships no content. Use only sources you are entitled to.",
                        )
                        .small()
                        .weak(),
                    );
                });
            });

        self.show_add = open;
        if submit {
            self.add_source();
            if !self.show_add {
                self.start_load(ctx);
            }
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut reload = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("PLAYER").small().weak());
                ui.label(
                    "Streamium hands the stream to a video player. mpv is recommended: it \
                     plays raw transport streams and accepts the HTTP headers providers require.",
                );
                ui.add_space(4.0);
                if self.players.is_empty() {
                    ui.label(
                        RichText::new("No player was found on this machine.")
                            .color(Color32::from_rgb(240, 200, 120)),
                    );
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Found:");
                        for p in &self.players {
                            if ui
                                .small_button(p.display_name())
                                .on_hover_text(p.path.display().to_string())
                                .clicked()
                            {
                                self.config.player_path = p.path.display().to_string();
                            }
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Path");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.config.player_path)
                            .hint_text("leave empty to use the best one found")
                            .desired_width(340.0),
                    );
                });
                if !self.config.player_path.trim().is_empty()
                    && !player::looks_executable(&self.config.player_path)
                {
                    ui.label(
                        RichText::new("That path does not point at a file.")
                            .color(Color32::from_rgb(255, 120, 100)),
                    );
                }
                if ui.button("Search again").clicked() {
                    self.players = player::detect();
                }

                ui.separator();
                ui.label(RichText::new("STREAMS").small().weak());
                if ui
                    .checkbox(
                        &mut self.config.prefer_hls,
                        "Ask Xtream servers for HLS instead of transport streams",
                    )
                    .on_hover_text(
                        "Transport streams start faster. Turn this on if your provider's \
                         raw streams stutter or will not open.",
                    )
                    .changed()
                {
                    reload = true;
                }
                ui.checkbox(
                    &mut self.config.refresh_on_launch,
                    "Reload the selected source when Streamium starts",
                );

                ui.separator();
                ui.label(RichText::new("SOURCES").small().weak());
                let mut remove = None;
                for (i, entry) in self.config.sources.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(entry.name());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    self.config.sources.remove(i);
                    if self.config.sources.is_empty() {
                        self.config.last_source = None;
                        self.catalog = Catalog::new();
                        self.epg = EpgIndex::new();
                        self.stats = LoadStats::default();
                        self.list_dirty = true;
                    } else {
                        self.config.last_source = Some(0);
                        reload = true;
                    }
                }

                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "Settings are stored in {}",
                        crate::config::config_path().display()
                    ))
                    .small()
                    .weak(),
                );
                ui.label(
                    RichText::new(
                        "That file holds your provider password in plain text. It is \
                         readable by anyone who can read your user profile.",
                    )
                    .small()
                    .weak(),
                );
            });
        if self.show_settings && !open {
            self.save();
        }
        self.show_settings = open;
        if reload {
            self.save();
            self.start_load(ctx);
        }
    }

    fn report_window(&mut self, ctx: &egui::Context) {
        let Some(report) = self.report.clone() else {
            return;
        };
        let mut open = self.show_report;
        egui::Window::new("Stream report")
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .default_height(520.0)
            .show(ctx, |ui| {
                let colour = match report.verdict {
                    Verdict::Good => Color32::from_rgb(130, 220, 140),
                    Verdict::Partial => Color32::from_rgb(240, 200, 120),
                    Verdict::NotTransportStream => Color32::from_rgb(255, 120, 100),
                };
                ui.heading(RichText::new(report.verdict.headline()).color(colour));
                ui.label(&report.channel);
                let summary = match (report.video(), report.audio()) {
                    (Some(v), Some(a)) => format!(
                        "{} video + {} audio",
                        analyse::codec_name(v.codec),
                        analyse::codec_name(a.codec)
                    ),
                    (Some(v), None) => format!("{} video, no audio", analyse::codec_name(v.codec)),
                    (None, Some(a)) => format!("{} audio only", analyse::codec_name(a.codec)),
                    (None, None) => "no elementary streams".to_string(),
                };
                ui.label(RichText::new(summary).weak());
                ui.add_space(6.0);

                egui::Grid::new("report_grid")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("First byte");
                        ui.label(format!("{} ms", report.ttfb_ms));
                        ui.end_row();
                        ui.label("Downloaded");
                        ui.label(format!(
                            "{:.1} MB in {} ms ({:.2} Mbit/s)",
                            report.bytes as f64 / 1_048_576.0,
                            report.elapsed_ms,
                            report.bitrate_mbps()
                        ));
                        ui.end_row();
                        ui.label("First key frame");
                        match report.first_keyframe_ms {
                            Some(ms) => ui.label(format!("{ms} ms")),
                            None => ui.label(RichText::new("never").color(colour)),
                        };
                        ui.end_row();
                        ui.label("TS packets");
                        ui.label(format!(
                            "{} ({} PCR samples)",
                            report.packets, report.pcr_samples
                        ));
                        ui.end_row();
                        ui.label("Programs");
                        ui.label(
                            report
                                .programs
                                .iter()
                                .map(|p| {
                                    format!(
                                        "#{} (PMT 0x{:04x}, PCR 0x{:04x}, {} streams)",
                                        p.number,
                                        p.pmt_pid,
                                        p.pcr_pid,
                                        p.stream_pids.len()
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(", "),
                        );
                        ui.end_row();
                        ui.label("Errors");
                        ui.label(format!(
                            "{} discontinuities · {} bytes lost sync · {} CRC",
                            report.discontinuities, report.sync_lost_bytes, report.crc_errors
                        ));
                        ui.end_row();
                        if let Some(ct) = &report.content_type {
                            ui.label("Content type");
                            ui.label(ct);
                            ui.end_row();
                        }
                        if report.followed_manifest {
                            ui.label("Segment");
                            ui.label(RichText::new(analyse::redact(&report.analysed_url)).small());
                            ui.end_row();
                        }
                    });

                ui.separator();
                ui.label(RichText::new("ELEMENTARY STREAMS").small().weak());
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("streams_grid")
                            .num_columns(6)
                            .spacing([14.0, 3.0])
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label(RichText::new("PID").small().weak());
                                ui.label(RichText::new("Codec").small().weak());
                                ui.label(RichText::new("Lang").small().weak());
                                ui.label(RichText::new("Units").small().weak());
                                ui.label(RichText::new("Key frames").small().weak());
                                ui.label(RichText::new("PTS span").small().weak());
                                ui.end_row();
                                for s in &report.streams {
                                    ui.monospace(format!("0x{:04x}", s.pid));
                                    ui.label(analyse::codec_name(s.codec));
                                    ui.label(s.language.clone().unwrap_or_else(|| "—".into()));
                                    ui.label(format!("{}", s.units));
                                    ui.label(format!("{}", s.keyframes));
                                    ui.label(
                                        s.pts_span_ms()
                                            .map(|ms| format!("{ms} ms"))
                                            .unwrap_or_else(|| "—".into()),
                                    );
                                    ui.end_row();
                                }
                            });
                    });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Copy report").clicked() {
                        ui.ctx().copy_text(report.to_text());
                    }
                    ui.label(
                        RichText::new("The copy has the account details removed.")
                            .small()
                            .weak(),
                    );
                });
            });
        self.show_report = open;
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_log;
        egui::Window::new("Log")
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .default_height(400.0)
            .show(ctx, |ui| {
                if !self.warnings.is_empty() {
                    ui.label(RichText::new("WARNINGS FROM THE LAST LOAD").small().weak());
                    for warning in &self.warnings {
                        ui.label(
                            RichText::new(format!("• {warning}"))
                                .color(Color32::from_rgb(240, 200, 120)),
                        );
                    }
                    ui.separator();
                }
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.label(RichText::new(line).monospace().small());
                        }
                    });
                ui.separator();
                if ui.button("Copy log").clicked() {
                    let mut text = self.warnings.join("\n");
                    text.push('\n');
                    text.push_str(&self.log.join("\n"));
                    ui.ctx().copy_text(text);
                }
            });
        self.show_log = open;
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        egui::Window::new("About this build")
            .open(&mut open)
            .resizable(true)
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!("Streamium {}", env!("CARGO_PKG_VERSION"))).heading(),
                );
                ui.add_space(4.0);
                ui.label(
                    "This is a test build of the Windows shell. The Rust core does the \
                     playlists, the Xtream protocol and the guide; the demuxer inspects \
                     the streams. Video is handed to mpv or VLC — rendering inside the \
                     window comes later.",
                );
                ui.add_space(8.0);
                ui.label(RichText::new("WHAT IS WORTH TESTING").small().weak());
                ui.label("• Does your provider load, and how long does it take?");
                ui.label("• Are the channel names, numbers, groups and guide right?");
                ui.label("• Does search find what you expect?");
                ui.label("• Pick a channel and press Analyse stream, then Copy report.");
                ui.label("• Does Play open the stream, and how quickly?");
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Streamium ships no content. Every source is one you add and are \
                         entitled to use.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "Report problems with the Log window (Copy log) and the stream report.\n\
                         Settings file: {}",
                        crate::config::config_path().display()
                    ))
                    .small()
                    .weak(),
                );
            });
        self.show_help = open;
    }
}
