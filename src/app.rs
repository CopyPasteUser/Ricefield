use crate::{
    clipboard,
    config::{self, Config},
    filesystem::{self, Action, Entry, Progress},
    journal::{self, Record as JournalRecord, State as JournalState},
    operation_queue::OperationQueue,
    preview,
    theme::{
        BUILTIN_IDS, ChromeComponent, Layout, ShellKind, SidebarPosition, SidebarSection, Theme,
    },
};
use eframe::egui::{
    self, Align2, Color32, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);

struct Tab {
    id: u64,
    path: PathBuf,
    address: String,
    query: String,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    entries: Vec<Entry>,
    folded_names: Vec<String>,
    visible: Vec<usize>,
    selected: BTreeSet<PathBuf>,
    last_selected: Option<usize>,
    generation: u64,
    loading: bool,
    error: Option<String>,
    dir_mtime: Option<SystemTime>,
    recursive_search: bool,
    search_pending: Option<Instant>,
}

impl Tab {
    fn new(id: u64, path: PathBuf) -> Self {
        Self {
            id,
            address: path.display().to_string(),
            path,
            query: String::new(),
            back: Vec::new(),
            forward: Vec::new(),
            entries: Vec::new(),
            folded_names: Vec::new(),
            visible: Vec::new(),
            selected: BTreeSet::new(),
            last_selected: None,
            generation: 0,
            loading: false,
            error: None,
            dir_mtime: None,
            recursive_search: false,
            search_pending: None,
        }
    }
}

fn next_search_repaint_delay(tabs: &[Tab], now: Instant) -> Option<Duration> {
    tabs.iter()
        .filter_map(|tab| tab.search_pending)
        .map(|started| SEARCH_DEBOUNCE.saturating_sub(now.saturating_duration_since(started)))
        .min()
}

struct ScanResult {
    tab: u64,
    generation: u64,
    result: std::io::Result<Vec<Entry>>,
    dir_mtime: Option<SystemTime>,
}

struct RefreshResult {
    folders: Vec<(u64, PathBuf, Option<SystemTime>)>,
    volumes: Vec<PathBuf>,
}

struct PreviewResult {
    path: PathBuf,
    result: std::io::Result<preview::Preview>,
}

enum PreviewContent {
    Image(egui::TextureHandle),
    Text(String),
    Info(String),
}

struct OperationResult {
    id: u64,
    result: std::io::Result<String>,
    moved_paths: Option<Vec<PathBuf>>,
}

enum OperationEvent {
    Progress(Progress),
    Finished(OperationResult),
}

#[derive(Clone)]
struct Clipboard {
    paths: Vec<PathBuf>,
    cut: bool,
}

enum NameMode {
    Folder(PathBuf),
    File(PathBuf),
    Rename(PathBuf),
}

impl NameMode {
    fn action(&self, name: String) -> Action {
        match self {
            Self::Folder(parent) => Action::NewFolder {
                parent: parent.clone(),
                name,
            },
            Self::File(parent) => Action::NewFile {
                parent: parent.clone(),
                name,
            },
            Self::Rename(source) => Action::Rename {
                source: source.clone(),
                name,
            },
        }
    }
}

enum Dialog {
    Name { mode: NameMode, value: String },
    Trash(Vec<PathBuf>),
    Delete(Vec<PathBuf>),
    Properties { path: PathBuf, mode: String },
}

pub struct Explorer {
    config: Config,
    theme: Theme,
    tabs: Vec<Tab>,
    active: usize,
    next_tab: u64,
    scan_sender: Sender<ScanResult>,
    scan_receiver: Receiver<ScanResult>,
    refresh_sender: Sender<RefreshResult>,
    refresh_receiver: Receiver<RefreshResult>,
    preview_sender: Sender<PreviewResult>,
    preview_receiver: Receiver<PreviewResult>,
    op_sender: Sender<OperationEvent>,
    op_receiver: Receiver<OperationEvent>,
    operation_queue: OperationQueue,
    active_operation: Option<(u64, String)>,
    queue_paused: bool,
    show_operations: bool,
    recovery_sender: Sender<(u64, std::io::Result<Vec<JournalRecord>>)>,
    recovery_receiver: Receiver<(u64, std::io::Result<Vec<JournalRecord>>)>,
    recovery_generation: u64,
    recovery_records: Vec<JournalRecord>,
    recovery_error: Option<String>,
    recovery_loading: bool,
    recovery_open: bool,
    clipboard: Option<Clipboard>,
    dialog: Option<Dialog>,
    status: Option<(String, bool)>,
    busy: bool,
    progress: Option<Progress>,
    cancel: Arc<AtomicBool>,
    volumes: Vec<PathBuf>,
    last_refresh_check: Instant,
    refresh_in_flight: bool,
    preview_path: Option<PathBuf>,
    preview_loading: bool,
    preview_content: Option<PreviewContent>,
    dirty_config: bool,
    focus_address: bool,
    focus_search: bool,
}

impl Explorer {
    pub fn new(
        context: &egui::Context,
        selected_theme: Option<String>,
        theme_file: Option<PathBuf>,
        locations: Vec<PathBuf>,
    ) -> Self {
        let mut config = Config::load();
        if let Some(name) = selected_theme {
            config.theme = name;
        }
        let theme_result = if let Some(path) = theme_file {
            Theme::from_file(&path)
        } else {
            Theme::builtin(&config.theme).ok_or_else(|| format!("Unknown theme: {}", config.theme))
        };
        let (theme, status) = match theme_result {
            Ok(theme) => (theme, None),
            Err(error) => (
                Theme::builtin("win7").expect("bundled theme"),
                Some((error, true)),
            ),
        };
        theme.apply(context);

        let (scan_sender, scan_receiver) = mpsc::channel();
        let (refresh_sender, refresh_receiver) = mpsc::channel();
        let (preview_sender, preview_receiver) = mpsc::channel();
        let (op_sender, op_receiver) = mpsc::channel();
        let (recovery_sender, recovery_receiver) = mpsc::channel();
        let mut explorer = Self {
            config,
            theme,
            tabs: Vec::new(),
            active: 0,
            next_tab: 1,
            scan_sender,
            scan_receiver,
            refresh_sender,
            refresh_receiver,
            preview_sender,
            preview_receiver,
            op_sender,
            op_receiver,
            operation_queue: OperationQueue::default(),
            active_operation: None,
            queue_paused: false,
            show_operations: false,
            recovery_sender,
            recovery_receiver,
            recovery_generation: 0,
            recovery_records: Vec::new(),
            recovery_error: None,
            recovery_loading: false,
            recovery_open: false,
            clipboard: None,
            dialog: None,
            status,
            busy: false,
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            volumes: vec![PathBuf::from("/")],
            last_refresh_check: Instant::now() - Duration::from_secs(3),
            refresh_in_flight: false,
            preview_path: None,
            preview_loading: false,
            preview_content: None,
            dirty_config: false,
            focus_address: false,
            focus_search: false,
        };
        if locations.is_empty() {
            explorer.new_tab(config::home());
        } else {
            for location in locations {
                let path = if location.is_file() {
                    location.parent().unwrap_or(Path::new("/")).to_path_buf()
                } else {
                    location
                };
                explorer.new_tab(path);
            }
            explorer.active = 0;
        }
        explorer.check_unfinished_moves();
        explorer
    }

    /// Journal inspection is read-only and never blocks the UI thread.
    fn check_unfinished_moves(&mut self) {
        self.recovery_generation = self.recovery_generation.wrapping_add(1);
        let generation = self.recovery_generation;
        self.recovery_loading = true;
        let sender = self.recovery_sender.clone();
        thread::spawn(move || {
            let result = journal::scan_incomplete();
            let _ = sender.send((generation, result));
        });
    }

    fn current(&self) -> &Tab {
        &self.tabs[self.active]
    }
    fn current_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    fn new_tab(&mut self, path: PathBuf) {
        let id = self.next_tab;
        self.next_tab += 1;
        self.tabs.push(Tab::new(id, path));
        self.active = self.tabs.len() - 1;
        self.scan(self.active);
    }

    fn close_tab(&mut self, index: usize) {
        if self.tabs.len() == 1 {
            self.navigate(config::home());
            return;
        }
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
    }

    fn scan(&mut self, index: usize) {
        let tab = &mut self.tabs[index];
        tab.generation += 1;
        tab.loading = true;
        tab.error = None;
        let task = (tab.id, tab.generation, tab.path.clone());
        let recursive = tab.recursive_search && !tab.query.is_empty();
        let query = tab.query.clone();
        let show_hidden = self.config.show_hidden;
        let sender = self.scan_sender.clone();
        thread::spawn(move || {
            let _ = sender.send(ScanResult {
                tab: task.0,
                generation: task.1,
                result: if recursive {
                    filesystem::search(&task.2, &query, show_hidden)
                } else {
                    filesystem::scan(&task.2)
                },
                dir_mtime: std::fs::metadata(&task.2)
                    .and_then(|meta| meta.modified())
                    .ok(),
            });
        });
    }

    fn refresh_all(&mut self) {
        for index in 0..self.tabs.len() {
            self.scan(index);
        }
    }

    fn navigate(&mut self, path: PathBuf) {
        let path = if path.is_file() {
            path.parent().map(Path::to_path_buf).unwrap_or(path)
        } else {
            path
        };
        if path == self.current().path {
            self.scan(self.active);
            return;
        }
        let tab = self.current_mut();
        tab.back.push(tab.path.clone());
        if tab.back.len() > 100 {
            tab.back.remove(0);
        }
        tab.forward.clear();
        tab.path = path;
        tab.address = tab.path.display().to_string();
        tab.query.clear();
        tab.search_pending = None;
        tab.selected.clear();
        tab.last_selected = None;
        self.scan(self.active);
    }

    fn history(&mut self, backwards: bool) {
        let tab = self.current_mut();
        let destination = if backwards {
            tab.back.pop()
        } else {
            tab.forward.pop()
        };
        if let Some(destination) = destination {
            if backwards {
                tab.forward.push(tab.path.clone());
            } else {
                tab.back.push(tab.path.clone());
            }
            tab.path = destination;
            tab.address = tab.path.display().to_string();
            tab.query.clear();
            tab.search_pending = None;
            tab.selected.clear();
            tab.last_selected = None;
            self.scan(self.active);
        }
    }

    fn parent(&mut self) {
        if let Some(parent) = self.current().path.parent().map(Path::to_path_buf) {
            self.navigate(parent);
        }
    }

    fn rebuild_visible(&mut self, index: usize) {
        let config = &self.config;
        let tab = &mut self.tabs[index];
        let search = tab.query.to_lowercase();
        tab.visible = (0..tab.entries.len())
            .filter(|&index| {
                let entry = &tab.entries[index];
                (config.show_hidden || !entry.name.starts_with('.'))
                    && tab.folded_names[index].contains(&search)
            })
            .collect();
        let sort = config.sort_by.as_str();
        tab.visible.sort_by(|left, right| {
            let a = &tab.entries[*left];
            let b = &tab.entries[*right];
            let folder_order = b.is_dir.cmp(&a.is_dir);
            if !folder_order.is_eq() {
                return folder_order;
            }
            let comparison = match sort {
                "size" => a.size.cmp(&b.size),
                "type" => a.kind().cmp(b.kind()),
                "modified" => a.modified.cmp(&b.modified),
                _ => tab.folded_names[*left].cmp(&tab.folded_names[*right]),
            };
            let ordered = if config.descending {
                comparison.reverse()
            } else {
                comparison
            };
            ordered.then_with(|| tab.folded_names[*left].cmp(&tab.folded_names[*right]))
        });
        tab.last_selected = None;
    }

    fn poll_background(&mut self, context: &egui::Context) {
        while let Ok((generation, result)) = self.recovery_receiver.try_recv() {
            if generation == self.recovery_generation {
                self.recovery_loading = false;
                match result {
                    Ok(records) => {
                        self.recovery_records = records;
                        self.recovery_error = None;
                    }
                    Err(error) => {
                        // Keep the last known records visible if a later scan fails.
                        self.recovery_error = Some(error.to_string());
                    }
                }
            }
        }
        while let Ok(message) = self.preview_receiver.try_recv() {
            if self.preview_path.as_ref() == Some(&message.path) {
                self.preview_loading = false;
                self.preview_content = Some(match message.result {
                    Ok(preview::Preview::Image {
                        rgba,
                        width,
                        height,
                    }) => {
                        let image =
                            egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                        PreviewContent::Image(context.load_texture(
                            "rice-preview",
                            image,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                    Ok(preview::Preview::Text(text)) => PreviewContent::Text(text),
                    Ok(preview::Preview::Info(text)) => PreviewContent::Info(text),
                    Err(error) => PreviewContent::Info(format!("Preview unavailable: {error}")),
                });
            }
        }
        while let Ok(result) = self.refresh_receiver.try_recv() {
            self.refresh_in_flight = false;
            self.volumes = result.volumes;
            for (tab_id, path, modified) in result.folders {
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.id == tab_id && tab.path == path)
                    && !self.tabs[index].loading
                    && self.tabs[index].dir_mtime != modified
                {
                    self.scan(index);
                }
            }
        }
        while let Ok(message) = self.scan_receiver.try_recv() {
            if let Some(index) = self
                .tabs
                .iter()
                .position(|tab| tab.id == message.tab && tab.generation == message.generation)
            {
                let tab = &mut self.tabs[index];
                tab.loading = false;
                tab.dir_mtime = message.dir_mtime;
                match message.result {
                    Ok(entries) => {
                        tab.folded_names = entries
                            .iter()
                            .map(|entry| entry.name.to_lowercase())
                            .collect();
                        tab.entries = entries;
                        tab.error = None;
                        tab.selected
                            .retain(|path| tab.entries.iter().any(|entry| &entry.path == path));
                        self.rebuild_visible(index);
                    }
                    Err(error) => {
                        tab.entries.clear();
                        tab.folded_names.clear();
                        tab.visible.clear();
                        tab.error = Some(error.to_string());
                    }
                }
            }
        }
        while let Ok(event) = self.op_receiver.try_recv() {
            match event {
                OperationEvent::Progress(progress) => self.progress = Some(progress),
                OperationEvent::Finished(message) => {
                    self.busy = false;
                    self.active_operation = None;
                    self.progress = None;
                    match message.result {
                        Ok(success) => {
                            self.status = Some((format!("#{}: {success}", message.id), false));
                            if message.moved_paths.as_ref().is_some_and(|moved| {
                                self.clipboard.as_ref().is_some_and(|clipboard| {
                                    clipboard.cut && &clipboard.paths == moved
                                })
                            }) {
                                self.clipboard = None;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                            self.queue_paused = !self.operation_queue.is_empty();
                            self.status = Some((
                                if self.queue_paused {
                                    "Cancelled. Completed items remain; waiting copies are paused."
                                } else {
                                    "Cancelled. Previously completed items remain in place."
                                }
                                .into(),
                                false,
                            ));
                        }
                        Err(error) => {
                            self.queue_paused = !self.operation_queue.is_empty();
                            self.status = Some((format!("Operation failed: {error}"), true))
                        }
                    }
                    self.refresh_all();
                    self.check_unfinished_moves();
                    self.start_next_operation();
                }
            }
        }
    }

    fn recovery_banner(&mut self, ui: &mut egui::Ui) {
        if self.recovery_records.is_empty() && self.recovery_error.is_none() {
            return;
        }
        egui::Panel::top("unfinished_move_notice")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(87, 58, 31))
                    .inner_margin(egui::Margin::symmetric(12, 7)),
            )
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        Color32::WHITE,
                        if self.recovery_error.is_some() {
                            "File move recovery records need attention".to_owned()
                        } else {
                            format!(
                                "{} unfinished cross-device file move(s) — source and destination may both exist",
                                self.recovery_records.len()
                            )
                        },
                    );
                    if ui.button("Review safely…").clicked() {
                        self.recovery_open = true;
                    }
                });
            });
    }

    fn recovery_dialog(&mut self, context: &egui::Context) {
        if !self.recovery_open {
            return;
        }
        let mut open = true;
        let mut rescan = false;
        egui::Window::new("Unfinished cross-device moves")
            .open(&mut open)
            .default_width(610.0)
            .default_height(340.0)
            .resizable(true)
            .show(context, |ui| {
                ui.label("Read-only inspection. RiceExplorer will not delete, resume, or overwrite either side automatically.");
                ui.label("Check the actual files, sizes and contents before manually deciding what to keep.");
                if let Some(error) = &self.recovery_error {
                    ui.colored_label(Color32::LIGHT_RED, format!("Cannot read all journal records: {error}"));
                }
                if self.recovery_loading {
                    ui.spinner();
                    ui.label("Checking operation records…");
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(330.0).show(ui, |ui| {
                    for record in &self.recovery_records {
                        ui.group(|ui| {
                            ui.strong(format!("{} — {:?}", record.id, record.state));
                            ui.label(format!("Source: {}", record.source.display()));
                            ui.label(format!("Destination: {}", record.destination.display()));
                            if let Some(staging) = &record.staging {
                                ui.label(format!("Staging: {}", staging.display()));
                            }
                            let guidance = match record.state {
                                JournalState::Prepared => "Copy may be incomplete or may not have started.",
                                JournalState::StagedVerified => "Staged data was verified; final publication may have occurred.",
                                JournalState::Published => "Destination was published; removing the source may be incomplete.",
                                JournalState::Completed => "Move completed.",
                            };
                            ui.small(guidance);
                        });
                    }
                });
                ui.separator();
                if ui.button("Rescan records").clicked() {
                    rescan = true;
                }
            });
        self.recovery_open = open;
        if rescan {
            self.check_unfinished_moves();
        }
    }

    /// One shared transfer surface for every rice, including shells without
    /// a status bar. It intentionally reserves a stable row while idle.
    fn transfer_strip(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        egui::Panel::top("transfer_status")
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.background())
                    .inner_margin(egui::Margin::symmetric(10, 3)),
            )
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let count = self.operation_queue.len() + usize::from(self.busy);
                    if ui.small_button(format!("Transfers ({count})")).clicked() {
                        self.show_operations = true;
                    }
                    if let Some((id, label)) = &self.active_operation {
                        ui.label(format!("#{id} {label}"));
                        if let Some(progress) = &self.progress {
                            if progress.total > 0 {
                                ui.add_sized(
                                    [110.0, 14.0],
                                    egui::ProgressBar::new(
                                        (progress.transferred as f32 / progress.total as f32)
                                            .clamp(0.0, 1.0),
                                    ),
                                );
                            } else {
                                ui.spinner();
                            }
                        }
                    }
                    if !self.operation_queue.is_empty() {
                        ui.label(format!("{} waiting", self.operation_queue.len()));
                        if self.queue_paused {
                            ui.colored_label(Color32::from_rgb(165, 83, 30), "Paused after error");
                        }
                    }
                });
            });
    }

    fn transfer_dialog(&mut self, context: &egui::Context) {
        if !self.show_operations {
            return;
        }
        let mut open = true;
        let mut remove = None;
        egui::Window::new("Transfers")
            .open(&mut open)
            .default_width(440.0)
            .resizable(true)
            .show(context, |ui| {
                if let Some((id, label)) = &self.active_operation {
                    ui.strong(format!("Running #{id}: {label}"));
                    if let Some(progress) = &self.progress {
                        ui.label(format!("{}: {}", progress.phase, progress.current.display()));
                    }
                    if ui.button("Cancel running transfer").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                    }
                    ui.small("Cancellation stops future work; already completed items are not undone.");
                } else {
                    ui.label("No transfer is running.");
                }
                ui.separator();
                ui.strong(format!("Waiting ({})", self.operation_queue.len()));
                ui.small("Only copy requests can wait. Removing a waiting job makes no filesystem changes.");
                if self.queue_paused && ui.button("Resume waiting copies").clicked() {
                    self.queue_paused = false;
                }
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for job in self.operation_queue.iter() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("#{} {}", job.id, job.label));
                            if ui.button("Remove").clicked() {
                                remove = Some(job.id);
                            }
                        });
                    }
                });
            });
        self.show_operations = open;
        if let Some(id) = remove {
            if self.operation_queue.remove(id) {
                self.status = Some((format!("Removed waiting copy #{id}"), false));
                if self.operation_queue.is_empty() {
                    self.queue_paused = false;
                }
            }
        }
        self.start_next_operation();
    }

    fn run_action(&mut self, action: Action) {
        if self.queue_paused && !matches!(&action, Action::Copy { .. }) {
            self.status = Some((
                "Waiting copies are paused after an error. Resume or remove them first.".into(),
                true,
            ));
            return;
        }
        if self.busy && !matches!(&action, Action::Copy { .. }) {
            self.status = Some((
                "A transfer is running. Only Copy can be queued; retry this action when it finishes."
                    .into(),
                true,
            ));
            return;
        }
        match self.operation_queue.enqueue(action) {
            Ok(id) => {
                if self.busy {
                    self.status = Some((format!("Copy #{id} queued"), false));
                }
                self.start_next_operation();
            }
            Err(error) => {
                self.status = Some((format!("Cannot queue transfer: {error}"), true));
            }
        }
    }

    fn start_next_operation(&mut self) {
        if self.busy || self.queue_paused {
            return;
        }
        let Some(job) = self.operation_queue.dequeue() else {
            return;
        };
        self.busy = true;
        self.active_operation = Some((job.id, job.label));
        self.progress = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&self.cancel);
        let moved_paths = match &job.action {
            Action::Move { sources, .. } => Some(sources.clone()),
            _ => None,
        };
        let sender = self.op_sender.clone();
        thread::spawn(move || {
            let result = filesystem::perform_with_progress(&job.action, &cancel, &mut |progress| {
                let _ = sender.send(OperationEvent::Progress(progress));
            });
            let _ = sender.send(OperationEvent::Finished(OperationResult {
                id: job.id,
                result,
                moved_paths,
            }));
        });
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.current().selected.iter().cloned().collect()
    }

    fn copy_selection(&mut self, cut: bool) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let desktop_clipboard = clipboard::copy_files(&paths, cut);
        self.clipboard = Some(Clipboard {
            paths: paths.clone(),
            cut,
        });
        self.status = Some((
            format!(
                "{} {} item(s){}",
                if cut { "Cut" } else { "Copied" },
                paths.len(),
                if desktop_clipboard.is_ok() {
                    ""
                } else {
                    " (internal clipboard)"
                }
            ),
            false,
        ));
    }

    fn copy_selected_paths(&mut self, context: &egui::Context) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        context.copy_text(text);
        self.status = Some((format!("Copied {} path(s) as text", paths.len()), false));
    }

    fn paste(&mut self) {
        let available = clipboard::paste_files()
            .map(|files| Clipboard {
                paths: files.paths,
                cut: files.cut,
            })
            .or_else(|| self.clipboard.clone());
        if let Some(clipboard) = available {
            if clipboard.cut {
                self.run_action(Action::Move {
                    sources: clipboard.paths,
                    destination: self.current().path.clone(),
                });
            } else {
                self.run_action(Action::Copy {
                    sources: clipboard.paths,
                    destination: self.current().path.clone(),
                });
            }
        }
    }

    fn activate(&mut self, entry: &Entry) {
        if entry.path.is_dir() {
            self.navigate(entry.path.clone());
        } else {
            #[cfg(target_os = "linux")]
            let output = Command::new("xdg-open").arg(&entry.path).spawn();
            #[cfg(target_os = "macos")]
            let output = Command::new("open").arg(&entry.path).spawn();
            #[cfg(target_os = "windows")]
            let output = Command::new("explorer").arg(&entry.path).spawn();
            if let Err(error) = output {
                self.status = Some((format!("Cannot open {}: {error}", entry.name), true));
            }
        }
    }

    fn select_entry(&mut self, entry: &Entry, visible_index: usize, modifiers: Modifiers) {
        let tab = self.current_mut();
        if modifiers.shift {
            if let Some(anchor) = tab.last_selected {
                if !modifiers.command {
                    tab.selected.clear();
                }
                for visible in anchor.min(visible_index)..=anchor.max(visible_index) {
                    if let Some(entry) = tab
                        .visible
                        .get(visible)
                        .and_then(|index| tab.entries.get(*index))
                    {
                        tab.selected.insert(entry.path.clone());
                    }
                }
            } else {
                tab.selected.insert(entry.path.clone());
            }
        } else if modifiers.command {
            if !tab.selected.remove(&entry.path) {
                tab.selected.insert(entry.path.clone());
            }
            tab.last_selected = Some(visible_index);
        } else {
            tab.selected.clear();
            tab.selected.insert(entry.path.clone());
            tab.last_selected = Some(visible_index);
        }
    }

    fn toggle_bookmark(&mut self) {
        let current = self.current().path.clone();
        if let Some(index) = self
            .config
            .bookmarks
            .iter()
            .position(|path| path == &current)
        {
            self.config.bookmarks.remove(index);
            self.status = Some(("Bookmark removed".into(), false));
        } else {
            self.config.bookmarks.push(current);
            self.status = Some(("Folder bookmarked".into(), false));
        }
        self.dirty_config = true;
    }

    fn set_theme(&mut self, context: &egui::Context, id: &str) {
        if let Some(theme) = Theme::builtin(id) {
            self.theme = theme;
            self.config.theme = id.to_owned();
            self.dirty_config = true;
            self.theme.apply(context);
        }
    }

    fn keyboard(&mut self, context: &egui::Context) {
        let keys = context.input(|input| {
            (
                input.modifiers.command,
                input.modifiers.alt,
                input.key_pressed(Key::L),
                input.key_pressed(Key::F),
                input.key_pressed(Key::T),
                input.key_pressed(Key::W),
                input.key_pressed(Key::C),
                input.key_pressed(Key::X),
                input.key_pressed(Key::V),
                input.key_pressed(Key::H),
                input.key_pressed(Key::R),
                input.key_pressed(Key::N),
                input.key_pressed(Key::F2),
                input.key_pressed(Key::Delete),
                input.key_pressed(Key::ArrowLeft),
                input.key_pressed(Key::ArrowRight),
                input.key_pressed(Key::ArrowUp),
                input.key_pressed(Key::A),
                input.key_pressed(Key::D),
                input.key_pressed(Key::Enter),
                input.key_pressed(Key::Space),
            )
        });
        let (
            ctrl,
            alt,
            l,
            f,
            t,
            w,
            c,
            x,
            v,
            h,
            r,
            n,
            f2,
            delete,
            left,
            right,
            up,
            a,
            d,
            enter,
            space,
        ) = keys;
        if ctrl && l {
            self.focus_address = true;
        }
        if ctrl && f {
            self.focus_search = true;
        }
        if ctrl && t {
            self.new_tab(self.current().path.clone());
        }
        if ctrl && w {
            self.close_tab(self.active);
        }
        // Text inputs must keep their normal copy/cut/paste shortcuts.
        let typing = context.egui_wants_keyboard_input();
        if !typing {
            if ctrl && a {
                let paths: Vec<_> = self
                    .current()
                    .visible
                    .iter()
                    .map(|&index| self.current().entries[index].path.clone())
                    .collect();
                self.current_mut().selected = paths.into_iter().collect();
            }
            if ctrl && c {
                if context.input(|input| input.modifiers.shift) {
                    self.copy_selected_paths(context);
                } else {
                    self.copy_selection(false);
                }
            }
            if ctrl && x {
                self.copy_selection(true);
            }
            if ctrl && v {
                self.paste();
            }
            if ctrl && d {
                let sources = self.selected_paths();
                if !sources.is_empty() {
                    self.run_action(Action::Copy {
                        sources,
                        destination: self.current().path.clone(),
                    });
                }
            }
            if space
                && self.current().selected.len() == 1
                && let Some(path) = self.selected_paths().first()
            {
                self.open_preview(path.clone());
            }
            if alt
                && enter
                && self.current().selected.len() == 1
                && let Some(path) = self.selected_paths().first()
            {
                self.open_properties(path.clone());
            }
            if ctrl && n {
                self.dialog = Some(Dialog::Name {
                    mode: NameMode::Folder(self.current().path.clone()),
                    value: String::new(),
                });
            }
            if f2 {
                self.rename_selected();
            }
            if delete {
                if context.input(|input| input.modifiers.shift) {
                    self.ask_delete();
                } else {
                    self.ask_trash();
                }
            }
            if enter && !alt && self.current().selected.len() == 1 {
                let selected = self.selected_paths();
                if let Some(entry) = self
                    .current()
                    .entries
                    .iter()
                    .find(|entry| selected.first() == Some(&entry.path))
                    .cloned()
                {
                    self.activate(&entry);
                }
            }
        }
        if ctrl && h {
            self.config.show_hidden = !self.config.show_hidden;
            self.dirty_config = true;
            for index in 0..self.tabs.len() {
                if self.tabs[index].recursive_search {
                    self.scan(index);
                } else {
                    self.rebuild_visible(index);
                }
            }
        }
        if ctrl && r {
            self.scan(self.active);
        }
        if alt && left {
            self.history(true);
        }
        if alt && right {
            self.history(false);
        }
        if alt && up {
            self.parent();
        }
    }

    fn rename_selected(&mut self) {
        if let Some(path) = self.selected_paths().first() {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            self.dialog = Some(Dialog::Name {
                mode: NameMode::Rename(path.clone()),
                value: name,
            });
        }
    }

    fn ask_trash(&mut self) {
        let selected = self.selected_paths();
        if !selected.is_empty() {
            self.dialog = Some(Dialog::Trash(selected));
        }
    }

    fn ask_delete(&mut self) {
        let selected = self.selected_paths();
        if !selected.is_empty() {
            self.dialog = Some(Dialog::Delete(selected));
        }
    }

    fn open_properties(&mut self, path: PathBuf) {
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::symlink_metadata(&path)
                .map(|metadata| format!("{:03o}", metadata.permissions().mode() & 0o777))
                .unwrap_or_default()
        };
        #[cfg(not(unix))]
        let mode = String::new();
        self.dialog = Some(Dialog::Properties { path, mode });
    }

    fn open_preview(&mut self, path: PathBuf) {
        self.preview_path = Some(path.clone());
        self.preview_content = None;
        self.preview_loading = true;
        let sender = self.preview_sender.clone();
        thread::spawn(move || {
            let result = preview::load(&path);
            let _ = sender.send(PreviewResult { path, result });
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let components = theme.top_components();
        egui::Panel::top("chrome")
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.chrome())
                    .inner_margin(egui::Margin::same(7)),
            )
            .show(ui, |ui| {
                for (index, component) in components.iter().enumerate() {
                    if index != 0 {
                        ui.add_space(3.0);
                    }
                    match component {
                        ChromeComponent::MenuBar => self.menu_bar(ui),
                        ChromeComponent::Tabs => self.tabs_bar(ui),
                        ChromeComponent::Navigation => self.navigation_bar(ui),
                        ChromeComponent::CommandBar => self.command_bar(ui, &theme),
                    }
                }
            });
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("RiceExplorer")
                    .color(Color32::WHITE)
                    .strong(),
            );
            ui.separator();
            ui.menu_button(egui::RichText::new("File").color(Color32::WHITE), |ui| {
                if ui.button("New folder  Ctrl+N").clicked() {
                    self.dialog = Some(Dialog::Name {
                        mode: NameMode::Folder(self.current().path.clone()),
                        value: String::new(),
                    });
                    ui.close();
                }
                if ui.button("New empty file").clicked() {
                    self.dialog = Some(Dialog::Name {
                        mode: NameMode::File(self.current().path.clone()),
                        value: String::new(),
                    });
                    ui.close();
                }
                if ui.button("New tab  Ctrl+T").clicked() {
                    self.new_tab(self.current().path.clone());
                    ui.close();
                }
            });
            ui.menu_button(egui::RichText::new("Edit").color(Color32::WHITE), |ui| {
                if ui.button("Copy  Ctrl+C").clicked() {
                    self.copy_selection(false);
                    ui.close();
                }
                if ui.button("Cut  Ctrl+X").clicked() {
                    self.copy_selection(true);
                    ui.close();
                }
                if ui.button("Paste  Ctrl+V").clicked() {
                    self.paste();
                    ui.close();
                }
                if ui.button("Rename  F2").clicked() {
                    self.rename_selected();
                    ui.close();
                }
                if ui.button("Move to Trash  Delete").clicked() {
                    self.ask_trash();
                    ui.close();
                }
            });
            ui.menu_button(egui::RichText::new("View").color(Color32::WHITE), |ui| {
                if ui.button("List").clicked() {
                    self.config.view = "list".into();
                    self.dirty_config = true;
                    ui.close();
                }
                if ui.button("Icons").clicked() {
                    self.config.view = "grid".into();
                    self.dirty_config = true;
                    ui.close();
                }
                if ui.button("Show hidden files").clicked() {
                    self.config.show_hidden = !self.config.show_hidden;
                    for index in 0..self.tabs.len() {
                        if self.tabs[index].recursive_search {
                            self.scan(index);
                        } else {
                            self.rebuild_visible(index);
                        }
                    }
                    self.dirty_config = true;
                    ui.close();
                }
            });
        });
        ui.separator();
    }

    fn command_bar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let has_selection = !self.current().selected.is_empty();
        ui.horizontal_wrapped(|ui| match theme.shell_kind() {
            ShellKind::Ribbon | ShellKind::Finder => {
                let finder = theme.shell_kind() == ShellKind::Finder;
                ui.label(
                    egui::RichText::new(if finder { "FINDER" } else { "HOME" })
                        .small()
                        .strong()
                        .color(theme.palette.text()),
                );
                ui.separator();
                if ui.small_button("New folder").clicked() {
                    self.dialog = Some(Dialog::Name {
                        mode: NameMode::Folder(self.current().path.clone()),
                        value: String::new(),
                    });
                }
                if ui
                    .add_enabled(has_selection, egui::Button::new("Copy"))
                    .clicked()
                {
                    self.copy_selection(false);
                }
                if !finder
                    && ui
                        .add_enabled(has_selection, egui::Button::new("Cut"))
                        .clicked()
                {
                    self.copy_selection(true);
                }
                if ui.small_button("Paste").clicked() {
                    self.paste();
                }
                if ui
                    .add_enabled(has_selection, egui::Button::new("Rename"))
                    .clicked()
                {
                    self.rename_selected();
                }
                if finder {
                    if ui
                        .add_enabled(has_selection, egui::Button::new("Info"))
                        .clicked()
                        && let Some(path) = self.selected_paths().first().cloned()
                    {
                        self.open_properties(path);
                    }
                } else if ui
                    .add_enabled(has_selection, egui::Button::new("Trash"))
                    .clicked()
                {
                    self.ask_trash();
                }
            }
            _ => {
                ui.label(
                    egui::RichText::new("ORGANIZE")
                        .small()
                        .strong()
                        .color(theme.palette.text()),
                );
                ui.separator();
                if ui.small_button("New folder").clicked() {
                    self.dialog = Some(Dialog::Name {
                        mode: NameMode::Folder(self.current().path.clone()),
                        value: String::new(),
                    });
                }
                if ui.small_button("Add to favorites").clicked() {
                    self.toggle_bookmark();
                }
                if ui.small_button("Refresh").clicked() {
                    self.scan(self.active);
                }
            }
        });
    }

    fn tabs_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let mut action = None;
            for index in 0..self.tabs.len() {
                let tab = &self.tabs[index];
                let label = tab
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "/".into());
                let selected = self.active == index;
                if ui
                    .add(egui::Button::new(label).selected(selected))
                    .clicked()
                {
                    action = Some((false, index));
                }
                if selected && ui.small_button("×").on_hover_text("Close tab").clicked() {
                    action = Some((true, index));
                }
            }
            if ui
                .small_button("+")
                .on_hover_text("New tab (Ctrl+T)")
                .clicked()
            {
                self.new_tab(self.current().path.clone());
            }
            if let Some((close, index)) = action {
                if close {
                    self.close_tab(index);
                } else {
                    self.active = index;
                }
            }
        });
    }

    fn navigation_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.current().back.is_empty(), egui::Button::new("<"))
                .on_hover_text("Back (Alt+Left)")
                .clicked()
            {
                self.history(true);
            }
            if ui
                .add_enabled(!self.current().forward.is_empty(), egui::Button::new(">"))
                .on_hover_text("Forward (Alt+Right)")
                .clicked()
            {
                self.history(false);
            }
            if ui
                .button("Up")
                .on_hover_text("Parent folder (Alt+Up)")
                .clicked()
            {
                self.parent();
            }
            if ui.button("Home").on_hover_text("Home").clicked() {
                self.navigate(config::home());
            }

            let search_width = if ui.available_width() < 620.0 {
                100.0
            } else {
                190.0
            };
            let address_width = (ui.available_width() - search_width - 268.0).max(90.0);
            let address = ui.add_sized(
                [address_width, 28.0],
                egui::TextEdit::singleline(&mut self.current_mut().address)
                    .hint_text("Folder path (Ctrl+L)"),
            );
            if self.focus_address {
                ui.memory_mut(|memory| memory.request_focus(address.id));
                self.focus_address = false;
            }
            if address.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                let value = self.current().address.clone();
                self.navigate(PathBuf::from(value));
            }
            if ui
                .button("Reload")
                .on_hover_text("Refresh (Ctrl+R)")
                .clicked()
            {
                self.scan(self.active);
            }

            let search = ui.add_sized(
                [search_width, 28.0],
                egui::TextEdit::singleline(&mut self.current_mut().query)
                    .hint_text("Search here… (Ctrl+F)"),
            );
            if self.focus_search {
                ui.memory_mut(|memory| memory.request_focus(search.id));
                self.focus_search = false;
            }
            if search.changed() {
                self.current_mut().selected.clear();
                if self.current().recursive_search {
                    self.current_mut().search_pending = Some(Instant::now());
                } else {
                    self.rebuild_visible(self.active);
                }
            }
            if !self.current().query.is_empty()
                && ui.small_button("×").on_hover_text("Clear search").clicked()
            {
                self.current_mut().query.clear();
                if self.current().recursive_search {
                    self.scan(self.active);
                } else {
                    self.rebuild_visible(self.active);
                }
                ui.memory_mut(|memory| memory.request_focus(search.id));
            }
            if ui
                .add(egui::Button::new("Subfolders").selected(self.current().recursive_search))
                .on_hover_text("Search filenames recursively (maximum 20,000 matches)")
                .clicked()
            {
                let next = !self.current().recursive_search;
                self.current_mut().recursive_search = next;
                self.current_mut().search_pending = None;
                self.scan(self.active);
            }
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        if !self.config.show_sidebar {
            return;
        }
        let theme = self.theme.clone();
        let panel = match theme.sidebar_position() {
            SidebarPosition::Left => egui::Panel::left("places"),
            SidebarPosition::Right => egui::Panel::right("places"),
        };
        panel
            .resizable(true)
            .default_size(theme.sidebar_width())
            .min_size(132.0)
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.sidebar())
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let sections = theme.sidebar_sections();
                    for (index, section) in sections.into_iter().enumerate() {
                        if index > 0 {
                            ui.add_space(14.0);
                            ui.separator();
                        }
                        let title = match section {
                            SidebarSection::Places => "PLACES",
                            SidebarSection::Favorites => "FAVORITES",
                            SidebarSection::Volumes => "VOLUMES",
                        };
                        ui.label(
                            egui::RichText::new(title)
                                .small()
                                .strong()
                                .color(theme.palette.muted()),
                        );
                        match section {
                            SidebarSection::Places => {
                                ui.add_space(8.0);
                                let home = config::home();
                                for (label, path) in [
                                    ("Home", home.clone()),
                                    ("Desktop", home.join("Desktop")),
                                    ("Documents", home.join("Documents")),
                                    ("Downloads", home.join("Downloads")),
                                    ("Music", home.join("Music")),
                                    ("Pictures", home.join("Pictures")),
                                    ("Videos", home.join("Videos")),
                                    ("Filesystem", PathBuf::from("/")),
                                ] {
                                    if path.is_dir()
                                        && ui
                                            .add_sized(
                                                [ui.available_width(), 29.0],
                                                egui::Button::new(label)
                                                    .selected(self.current().path == path),
                                            )
                                            .clicked()
                                    {
                                        self.navigate(path);
                                    }
                                }
                            }
                            SidebarSection::Favorites => {
                                for path in self.config.bookmarks.clone() {
                                    let label =
                                        path.file_name().unwrap_or_default().to_string_lossy();
                                    if ui
                                        .add_sized(
                                            [ui.available_width(), 29.0],
                                            egui::Button::new(format!("Favorite: {label}"))
                                                .selected(self.current().path == path),
                                        )
                                        .clicked()
                                    {
                                        self.navigate(path);
                                    }
                                }
                                if ui.small_button("+  Bookmark current folder").clicked() {
                                    self.toggle_bookmark();
                                }
                            }
                            SidebarSection::Volumes => {
                                for path in self.volumes.clone() {
                                    let label = if path == Path::new("/") {
                                        "System disk".to_owned()
                                    } else {
                                        format!(
                                            "Volume: {}",
                                            path.file_name().unwrap_or_default().to_string_lossy()
                                        )
                                    };
                                    if ui
                                        .add_sized(
                                            [ui.available_width(), 29.0],
                                            egui::Button::new(label)
                                                .selected(self.current().path == path),
                                        )
                                        .on_hover_text(path.display().to_string())
                                        .clicked()
                                    {
                                        self.navigate(path);
                                    }
                                }
                            }
                        }
                    }
                });
            });
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.preview_path.clone() else {
            return;
        };
        egui::Panel::right("preview")
            .resizable(true)
            .default_size(300.0)
            .min_size(210.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(shorten(
                        &path.file_name().unwrap_or_default().to_string_lossy(),
                        27,
                    ));
                    if ui
                        .small_button("×")
                        .on_hover_text("Close preview")
                        .clicked()
                    {
                        self.preview_path = None;
                        self.preview_content = None;
                    }
                });
                ui.separator();
                if self.preview_loading {
                    ui.add(egui::Spinner::new());
                    ui.label("Loading preview…");
                } else if let Some(content) = &self.preview_content {
                    match content {
                        PreviewContent::Image(texture) => {
                            let size = texture.size_vec2();
                            let ratio = ((ui.available_width() - 8.0) / size.x).min(1.0);
                            ui.add(
                                egui::Image::new((texture.id(), size))
                                    .fit_to_exact_size(size * ratio.max(0.1)),
                            );
                        }
                        PreviewContent::Text(contents) => {
                            egui::ScrollArea::vertical()
                                .max_height(480.0)
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(contents).monospace());
                                });
                        }
                        PreviewContent::Info(contents) => {
                            ui.label(contents);
                        }
                    }
                }
                ui.separator();
                if ui.button("Properties").clicked() {
                    self.open_properties(path.clone());
                }
                if ui.button("Open").clicked() {
                    if path.is_dir() {
                        self.navigate(path.clone());
                        self.preview_path = None;
                    } else if let Err(error) = Command::new("xdg-open").arg(&path).spawn() {
                        self.status = Some((format!("Cannot open previewed file: {error}"), true));
                    }
                }
            });
    }

    fn main_panel(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.panel())
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let title = self
                        .current()
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Computer".into());
                    ui.label(egui::RichText::new(title).size(20.0).strong());
                    ui.add_space(8.0);
                    let count = self.current().visible.len();
                    ui.label(
                        egui::RichText::new(format!("{count} items")).color(theme.palette.muted()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("Preview")
                            .on_hover_text("Preview selected item (Space)")
                            .clicked()
                            && let Some(path) = self.selected_paths().first()
                        {
                            self.open_preview(path.clone());
                        }
                        if ui
                            .small_button("Places")
                            .on_hover_text("Toggle sidebar")
                            .clicked()
                        {
                            self.config.show_sidebar = !self.config.show_sidebar;
                            self.dirty_config = true;
                        }
                        if ui
                            .add(egui::Button::new("Grid").selected(self.config.view == "grid"))
                            .on_hover_text("Icon view")
                            .clicked()
                        {
                            self.config.view = "grid".into();
                            self.dirty_config = true;
                        }
                        if ui
                            .add(egui::Button::new("List").selected(self.config.view == "list"))
                            .on_hover_text("List view")
                            .clicked()
                        {
                            self.config.view = "list".into();
                            self.dirty_config = true;
                        }
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.menu_button("New", |ui| {
                        if ui.button("New folder  Ctrl+N").clicked() {
                            self.dialog = Some(Dialog::Name {
                                mode: NameMode::Folder(self.current().path.clone()),
                                value: String::new(),
                            });
                            ui.close();
                        }
                        if ui.button("Empty file").clicked() {
                            self.dialog = Some(Dialog::Name {
                                mode: NameMode::File(self.current().path.clone()),
                                value: String::new(),
                            });
                            ui.close();
                        }
                    });
                    ui.menu_button("Actions", |ui| {
                        let has_selection = !self.current().selected.is_empty();
                        if ui
                            .add_enabled(has_selection, egui::Button::new("Copy  Ctrl+C"))
                            .clicked()
                        {
                            self.copy_selection(false);
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                has_selection,
                                egui::Button::new("Copy path  Ctrl+Shift+C"),
                            )
                            .clicked()
                        {
                            self.copy_selected_paths(ui.ctx());
                            ui.close();
                        }
                        if ui
                            .add_enabled(has_selection, egui::Button::new("Cut  Ctrl+X"))
                            .clicked()
                        {
                            self.copy_selection(true);
                            ui.close();
                        }
                        if ui.button("Paste  Ctrl+V").clicked() {
                            self.paste();
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(has_selection, egui::Button::new("Rename  F2"))
                            .clicked()
                        {
                            self.rename_selected();
                            ui.close();
                        }
                        if ui
                            .add_enabled(has_selection, egui::Button::new("Duplicate  Ctrl+D"))
                            .clicked()
                        {
                            self.run_action(Action::Copy {
                                sources: self.selected_paths(),
                                destination: self.current().path.clone(),
                            });
                            ui.close();
                        }
                        if ui
                            .add_enabled(has_selection, egui::Button::new("Move to Trash  Delete"))
                            .clicked()
                        {
                            self.ask_trash();
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                has_selection,
                                egui::Button::new("Delete permanently…  Shift+Delete"),
                            )
                            .clicked()
                        {
                            self.ask_delete();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Select all  Ctrl+A").clicked() {
                            let selected: std::collections::BTreeSet<_> = self
                                .current()
                                .visible
                                .iter()
                                .map(|&index| self.current().entries[index].path.clone())
                                .collect();
                            self.current_mut().selected = selected;
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                self.current().selected.len() == 1,
                                egui::Button::new("Properties  Alt+Enter"),
                            )
                            .clicked()
                        {
                            if let Some(path) = self.selected_paths().first() {
                                self.open_properties(path.clone());
                            }
                            ui.close();
                        }
                    });
                    ui.separator();
                    ui.label(egui::RichText::new("Sort:").color(theme.palette.muted()));
                    let old_sort = self.config.sort_by.clone();
                    egui::ComboBox::from_id_salt("sort_by")
                        .selected_text(&self.config.sort_by)
                        .show_ui(ui, |ui| {
                            for option in ["name", "type", "size", "modified"] {
                                ui.selectable_value(
                                    &mut self.config.sort_by,
                                    option.into(),
                                    option,
                                );
                            }
                        });
                    if old_sort != self.config.sort_by {
                        self.dirty_config = true;
                        for i in 0..self.tabs.len() {
                            self.rebuild_visible(i);
                        }
                    }
                    if ui
                        .small_button(if self.config.descending {
                            "Desc"
                        } else {
                            "Asc"
                        })
                        .on_hover_text("Reverse sort")
                        .clicked()
                    {
                        self.config.descending = !self.config.descending;
                        self.dirty_config = true;
                        for i in 0..self.tabs.len() {
                            self.rebuild_visible(i);
                        }
                    }
                    ui.separator();
                    if ui
                        .add(egui::Button::new("Hidden").selected(self.config.show_hidden))
                        .on_hover_text("Toggle hidden files (Ctrl+H)")
                        .clicked()
                    {
                        self.config.show_hidden = !self.config.show_hidden;
                        self.dirty_config = true;
                        for i in 0..self.tabs.len() {
                            if self.tabs[i].recursive_search {
                                self.scan(i);
                            } else {
                                self.rebuild_visible(i);
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let name = self.theme.name.clone();
                        egui::ComboBox::from_id_salt("theme_selector")
                            .selected_text(name)
                            .show_ui(ui, |ui| {
                                for id in BUILTIN_IDS {
                                    let profile = Theme::builtin(id).expect("bundled theme");
                                    if ui
                                        .selectable_label(self.theme.id == *id, profile.name)
                                        .clicked()
                                    {
                                        self.set_theme(ui.ctx(), id);
                                    }
                                }
                            });
                    });
                });
                ui.separator();

                if let Some(error) = self.current().error.as_ref() {
                    ui.colored_label(
                        Color32::from_rgb(177, 43, 37),
                        format!("Cannot open folder: {error}"),
                    );
                    if ui.button("Retry").clicked() {
                        self.scan(self.active);
                    }
                } else if self.current().loading && self.current().entries.is_empty() {
                    ui.add(egui::Spinner::new());
                    ui.label("Loading folder…");
                } else if self.current().visible.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(64.0);
                        ui.label(
                            egui::RichText::new(if self.current().query.is_empty() {
                                "This folder is empty"
                            } else {
                                "No matching files"
                            })
                            .size(18.0),
                        );
                        ui.label(
                            egui::RichText::new("Try another folder or clear the search.")
                                .color(theme.palette.muted()),
                        );
                    });
                } else {
                    self.file_listing(ui);
                }
            });
    }

    fn file_listing(&mut self, ui: &mut egui::Ui) {
        let grid = self.config.view == "grid";
        let width = ui.available_width();
        let tile = self.theme.metrics.tile_width;
        let columns = if grid {
            ((width / (tile + 8.0)).floor() as usize).max(1)
        } else {
            1
        };
        let visible = self.current().visible.len();
        let row_height = if grid {
            98.0
        } else {
            self.theme.metrics.row_height
        };
        let rows = visible.div_ceil(columns);
        if !grid {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Name").strong());
                ui.add_space((width * 0.55 - 54.0).max(8.0));
                ui.label(
                    egui::RichText::new("Type   •   Size   •   Modified")
                        .strong()
                        .small(),
                );
            });
            ui.separator();
        }
        egui::ScrollArea::vertical()
            .id_salt("file_scroller")
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows, |ui, range| {
                for row in range {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for col in 0..columns {
                            let visible_index = row * columns + col;
                            let entry = self
                                .current()
                                .visible
                                .get(visible_index)
                                .and_then(|index| self.current().entries.get(*index))
                                .cloned();
                            if let Some(entry) = entry {
                                self.file_item(
                                    ui,
                                    &entry,
                                    visible_index,
                                    if grid { tile } else { width },
                                    row_height,
                                    grid,
                                );
                            }
                        }
                    });
                }
            });
    }

    fn file_item(
        &mut self,
        ui: &mut egui::Ui,
        entry: &Entry,
        index: usize,
        width: f32,
        height: f32,
        grid: bool,
    ) {
        let selected = self.current().selected.contains(&entry.path);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width.max(80.0), height), Sense::click());
        let theme = self.theme.clone();
        let painter = ui.painter().with_clip_rect(rect);
        if selected || response.hovered() {
            painter.rect_filled(
                rect.shrink(2.0),
                theme.metrics.rounding,
                if selected {
                    theme.palette.selection()
                } else {
                    theme.palette.background()
                },
            );
        }
        let icon_size = theme.metrics.icon_size;
        let icon_area = if grid {
            Rect::from_center_size(
                Pos2::new(rect.center().x, rect.top() + 34.0),
                Vec2::splat(icon_size * 1.6),
            )
        } else {
            Rect::from_center_size(
                Pos2::new(rect.left() + 22.0, rect.center().y),
                Vec2::splat(icon_size),
            )
        };
        paint_file_icon(&painter, icon_area, entry, &theme);
        let name = if grid {
            shorten(&entry.name, 22)
        } else {
            shorten(&entry.name, ((width * 0.53) / 7.3) as usize)
        };
        let font = FontId::proportional(if grid { 12.5 } else { 13.0 });
        let foreground = theme.palette.text();
        if grid {
            painter.text(
                Pos2::new(rect.center().x, rect.top() + 67.0),
                Align2::CENTER_CENTER,
                name,
                font,
                foreground,
            );
        } else {
            painter.text(
                Pos2::new(rect.left() + 43.0, rect.center().y),
                Align2::LEFT_CENTER,
                name,
                font,
                foreground,
            );
            let label = format!(
                "{}    {}    {}",
                entry.kind(),
                if entry.is_dir {
                    "—".into()
                } else {
                    filesystem::human_size(entry.size)
                },
                filesystem::age(entry.modified)
            );
            painter.text(
                Pos2::new(rect.left() + width * 0.61, rect.center().y),
                Align2::LEFT_CENTER,
                shorten(&label, ((width * 0.38) / 7.0) as usize),
                FontId::proportional(11.5),
                theme.palette.muted(),
            );
        }
        let response = response.on_hover_text(entry.path.display().to_string());
        let double = response.double_clicked();
        let clicked = response.clicked();
        if clicked || double {
            let modifiers = ui.input(|input| input.modifiers);
            self.select_entry(entry, index, modifiers);
            if double {
                self.activate(entry);
            }
        }
        response.context_menu(|ui| {
            if ui.button("Open").clicked() {
                self.activate(entry);
                ui.close();
            }
            if ui.button("Preview").clicked() {
                self.open_preview(entry.path.clone());
                ui.close();
            }
            if ui.button("Open in new tab").clicked() {
                if entry.path.is_dir() {
                    self.new_tab(entry.path.clone());
                }
                ui.close();
            }
            ui.separator();
            if ui.button("Copy").clicked() {
                if !self.current().selected.contains(&entry.path) {
                    self.select_entry(entry, index, Modifiers::default());
                }
                self.copy_selection(false);
                ui.close();
            }
            if ui.button("Copy path").clicked() {
                if !self.current().selected.contains(&entry.path) {
                    self.select_entry(entry, index, Modifiers::default());
                }
                self.copy_selected_paths(ui.ctx());
                ui.close();
            }
            if ui.button("Cut").clicked() {
                if !self.current().selected.contains(&entry.path) {
                    self.select_entry(entry, index, Modifiers::default());
                }
                self.copy_selection(true);
                ui.close();
            }
            if ui.button("Rename").clicked() {
                self.dialog = Some(Dialog::Name {
                    mode: NameMode::Rename(entry.path.clone()),
                    value: entry.name.clone(),
                });
                ui.close();
            }
            if ui.button("Duplicate").clicked() {
                if let Some(parent) = entry.path.parent() {
                    self.run_action(Action::Copy {
                        sources: vec![entry.path.clone()],
                        destination: parent.to_path_buf(),
                    });
                }
                ui.close();
            }
            if ui.button("Create symbolic link").clicked() {
                self.run_action(Action::Link {
                    sources: vec![entry.path.clone()],
                    destination: self.current().path.clone(),
                });
                ui.close();
            }
            if ui.button("Move to Trash").clicked() {
                if !self.current().selected.contains(&entry.path) {
                    self.select_entry(entry, index, Modifiers::default());
                }
                self.ask_trash();
                ui.close();
            }
            if ui.button("Delete permanently…").clicked() {
                if !self.current().selected.contains(&entry.path) {
                    self.select_entry(entry, index, Modifiers::default());
                }
                self.ask_delete();
                ui.close();
            }
            if ui.button("Properties").clicked() {
                self.open_properties(entry.path.clone());
                ui.close();
            }
            ui.separator();
            ui.label(format!(
                "{} • {}",
                entry.kind(),
                if entry.is_dir {
                    "Folder".into()
                } else {
                    filesystem::human_size(entry.size)
                }
            ));
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        egui::Panel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.background())
                    .inner_margin(egui::Margin::symmetric(10, 5)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let tab = self.current();
                    ui.label(format!(
                        "{} items  •  {} selected",
                        tab.visible.len(),
                        tab.selected.len()
                    ));
                    if tab.loading || self.busy {
                        ui.add(egui::Spinner::new());
                        if let Some(progress) = &self.progress {
                            if progress.total > 0 {
                                ui.add_sized(
                                    [155.0, 16.0],
                                    egui::ProgressBar::new(
                                        (progress.transferred as f32 / progress.total as f32)
                                            .clamp(0.0, 1.0),
                                    )
                                    .show_percentage(),
                                );
                                ui.label(filesystem::human_size(progress.transferred));
                            } else {
                                ui.label("Preparing…");
                            }
                            ui.label(progress.phase);
                            let filename = progress
                                .current
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy();
                            ui.label(shorten(&filename, 28));
                        } else {
                            ui.label("Working…");
                        }
                        if self.busy && ui.small_button("Cancel").clicked() {
                            self.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    if let Some((message, failed)) = &self.status {
                        ui.separator();
                        ui.colored_label(
                            if *failed {
                                Color32::from_rgb(176, 46, 43)
                            } else {
                                theme.palette.accent()
                            },
                            message,
                        );
                    }
                });
            });
    }

    fn dialogs(&mut self, context: &egui::Context) {
        if self.dialog.is_some() && context.input(|input| input.key_pressed(Key::Escape)) {
            self.dialog = None;
            return;
        }
        let mut action = None;
        let mut dismiss = false;
        if let Some(dialog) = self.dialog.as_mut() {
            match dialog {
                Dialog::Name { mode, value } => {
                    let title = match mode {
                        NameMode::Folder(_) => "New folder",
                        NameMode::File(_) => "New empty file",
                        NameMode::Rename(_) => "Rename item",
                    };
                    egui::Window::new(title)
                        .collapsible(false)
                        .resizable(false)
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(context, |ui| {
                            ui.label("Name");
                            let input =
                                ui.add_sized([340.0, 28.0], egui::TextEdit::singleline(value));
                            let valid = filesystem::valid_name(value);
                            if !valid {
                                ui.colored_label(
                                    Color32::from_rgb(170, 48, 43),
                                    "Enter a name without slashes, or dots alone.",
                                );
                            }
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    dismiss = true;
                                }
                                if ui.add_enabled(valid, egui::Button::new("Save")).clicked()
                                    || (valid
                                        && input.lost_focus()
                                        && ui.input(|i| i.key_pressed(Key::Enter)))
                                {
                                    let name = value.clone();
                                    action = Some(mode.action(name));
                                    dismiss = true;
                                }
                            });
                        });
                }
                Dialog::Trash(paths) => {
                    egui::Window::new("Move to Trash?")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(context, |ui| {
                            ui.label(format!("Move {} selected item(s) to Trash?", paths.len()));
                            ui.label("The items can usually be restored from your desktop Trash.");
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    dismiss = true;
                                }
                                if ui.button("Move to Trash").clicked() {
                                    action = Some(Action::Trash {
                                        paths: paths.clone(),
                                    });
                                    dismiss = true;
                                }
                            });
                        });
                }
                Dialog::Delete(paths) => {
                    egui::Window::new("Permanently delete items?")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(context, |ui| {
                            ui.colored_label(
                                Color32::RED,
                                format!("Delete {} item(s) permanently?", paths.len()),
                            );
                            ui.label("This bypasses Trash. There is no undo.");
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    dismiss = true;
                                }
                                if ui.button("Delete permanently").clicked() {
                                    action = Some(Action::Delete {
                                        paths: paths.clone(),
                                    });
                                    dismiss = true;
                                }
                            });
                        });
                }
                Dialog::Properties { path, mode } => {
                    egui::Window::new("Properties")
                        .collapsible(false)
                        .resizable(true)
                        .default_width(460.0)
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(context, |ui| {
                            match filesystem::property_lines(path) {
                                Ok(lines) => {
                                    for (name, value) in lines {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.strong(format!("{name}:"));
                                            ui.label(value);
                                        });
                                    }
                                }
                                Err(error) => {
                                    ui.colored_label(
                                        Color32::RED,
                                        format!("Cannot read properties: {error}"),
                                    );
                                }
                            }
                            #[cfg(unix)]
                            {
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label("Mode (octal):");
                                    ui.add_sized([100.0, 25.0], egui::TextEdit::singleline(mode));
                                    let parsed = u32::from_str_radix(mode.trim(), 8)
                                        .ok()
                                        .filter(|number| *number <= 0o777);
                                    let symlink = std::fs::symlink_metadata(&*path)
                                        .is_ok_and(|metadata| metadata.file_type().is_symlink());
                                    if ui
                                        .add_enabled(
                                            parsed.is_some() && !symlink,
                                            egui::Button::new("Apply"),
                                        )
                                        .clicked()
                                    {
                                        action = Some(Action::SetMode {
                                            path: path.clone(),
                                            mode: parsed
                                                .expect("button only enabled for valid mode"),
                                        });
                                        dismiss = true;
                                    }
                                });
                            }
                            if ui.button("Close").clicked() {
                                dismiss = true;
                            }
                        });
                }
            }
        }
        if dismiss {
            self.dialog = None;
        }
        if let Some(action) = action {
            self.run_action(action);
        }
    }
}

impl eframe::App for Explorer {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        self.poll_background(&context);
        for index in 0..self.tabs.len() {
            if self.tabs[index]
                .search_pending
                .is_some_and(|when| when.elapsed() >= SEARCH_DEBOUNCE)
            {
                self.tabs[index].search_pending = None;
                self.scan(index);
            }
        }
        if !self.refresh_in_flight && self.last_refresh_check.elapsed() >= Duration::from_secs(3) {
            let folders: Vec<_> = self
                .tabs
                .iter()
                .map(|tab| (tab.id, tab.path.clone()))
                .collect();
            let sender = self.refresh_sender.clone();
            self.refresh_in_flight = true;
            thread::spawn(move || {
                let folders = folders
                    .into_iter()
                    .map(|(id, path)| {
                        let modified = std::fs::metadata(&path)
                            .and_then(|meta| meta.modified())
                            .ok();
                        (id, path, modified)
                    })
                    .collect();
                let _ = sender.send(RefreshResult {
                    folders,
                    volumes: filesystem::mounted_volumes(),
                });
            });
            self.last_refresh_check = Instant::now();
        }
        let dropped: Vec<PathBuf> = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.run_action(Action::Copy {
                sources: dropped,
                destination: self.current().path.clone(),
            });
        }
        if self.dialog.is_none() {
            self.keyboard(&context);
        }
        self.top_bar(ui);
        self.recovery_banner(ui);
        self.transfer_strip(ui);
        if self.theme.shell.status_bar.unwrap_or(true) {
            self.status_bar(ui);
        }
        self.sidebar(ui);
        self.preview_panel(ui);
        self.main_panel(ui);
        self.dialogs(&context);
        self.recovery_dialog(&context);
        self.transfer_dialog(&context);
        if self.dirty_config {
            if let Err(error) = self.config.save() {
                self.status = Some((format!("Cannot save preferences: {error}"), true));
            }
            self.dirty_config = false;
        }
        let idle_delay = if self.busy || self.tabs.iter().any(|tab| tab.loading) {
            Duration::from_millis(90)
        } else {
            Duration::from_secs(3)
        };
        let delay = next_search_repaint_delay(&self.tabs, Instant::now())
            .map_or(idle_delay, |search_delay| search_delay.min(idle_delay));
        context.request_repaint_after(delay);
    }
}

fn shorten(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max {
        return name.into();
    }
    let keep = max.saturating_sub(1);
    format!("{}…", name.chars().take(keep).collect::<String>())
}

fn paint_file_icon(painter: &egui::Painter, rect: Rect, file: &Entry, theme: &Theme) {
    let round: u8 = if theme.layout == Layout::Classic {
        0
    } else {
        3
    };
    if file.is_dir {
        let top = Rect::from_min_max(
            Pos2::new(rect.left() + 2.0, rect.top() + 6.0),
            Pos2::new(rect.left() + rect.width() * 0.55, rect.top() + 15.0),
        );
        painter.rect_filled(top, round, theme.palette.folder());
        let body = Rect::from_min_max(
            Pos2::new(rect.left() + 1.0, rect.top() + 12.0),
            Pos2::new(rect.right() - 1.0, rect.bottom() - 3.0),
        );
        painter.rect_filled(body, round, theme.palette.folder());
        painter.line_segment(
            [
                Pos2::new(body.left() + 2.0, body.top() + 3.0),
                Pos2::new(body.right() - 2.0, body.top() + 3.0),
            ],
            Stroke::new(1.0, Color32::WHITE.gamma_multiply(0.6)),
        );
    } else {
        let body = Rect::from_min_max(
            Pos2::new(rect.left() + rect.width() * 0.15, rect.top() + 1.0),
            Pos2::new(rect.right() - rect.width() * 0.15, rect.bottom() - 1.0),
        );
        painter.rect_filled(body, round, theme.palette.file());
        let short = match file.kind() {
            "Image" => "IMG",
            "Audio" => "AUD",
            "Video" => "VID",
            "Archive" => "ZIP",
            "PDF" => "PDF",
            "Code" => "</>",
            "Link" => "LNK",
            _ => "FILE",
        };
        painter.text(
            body.center(),
            Align2::CENTER_CENTER,
            short,
            FontId::proportional((rect.width() * 0.29).clamp(8.0, 13.0)),
            Color32::WHITE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, NameMode, Tab, next_search_repaint_delay};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn name_dialog_actions_keep_the_original_paths() {
        let parent = PathBuf::from("/initial/folder");
        let source = parent.join("old.txt");

        assert!(matches!(
            NameMode::Folder(parent.clone()).action("new".into()),
            Action::NewFolder { parent: target, name }
                if target == parent && name == "new"
        ));
        assert!(matches!(
            NameMode::File(parent.clone()).action("file.txt".into()),
            Action::NewFile { parent: target, name }
                if target == parent && name == "file.txt"
        ));
        assert!(matches!(
            NameMode::Rename(source.clone()).action("renamed.txt".into()),
            Action::Rename { source: target, name }
                if target == source && name == "renamed.txt"
        ));
    }

    #[test]
    fn pending_search_in_an_inactive_tab_wakes_the_ui_on_time() {
        let now = Instant::now();
        let active = Tab::new(1, PathBuf::from("/active"));
        let mut inactive = Tab::new(2, PathBuf::from("/inactive"));
        inactive.search_pending = Some(now - Duration::from_millis(200));
        assert_eq!(
            next_search_repaint_delay(&[active, inactive], now),
            Some(Duration::from_millis(150))
        );

        let mut overdue = Tab::new(3, PathBuf::from("/overdue"));
        overdue.search_pending = Some(now - Duration::from_millis(400));
        assert_eq!(
            next_search_repaint_delay(&[overdue], now),
            Some(Duration::ZERO)
        );
    }
}
