use crate::{
    clipboard,
    config::{self, Config},
    filesystem::{self, Action, ConflictPolicy, Entry, Progress},
    journal::{self, Record as JournalRecord, State as JournalState},
    operation_queue::OperationQueue,
    preview,
    theme::{
        BUILTIN_IDS, ChromeComponent, Layout, ShellKind, SidebarPosition, SidebarSection,
        SidebarSectionMode, Theme,
    },
};
use eframe::egui::{
    self, Align2, Color32, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2,
};
use std::{
    collections::{BTreeSet, VecDeque},
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
const MAX_RECENT_OPERATIONS: usize = 64;

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
    scan_cancel: Arc<AtomicBool>,
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
            scan_cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn cancel_scan(&self) {
        self.scan_cancel.store(true, Ordering::Relaxed);
    }

    fn next_scan_token(&mut self) -> Arc<AtomicBool> {
        self.cancel_scan();
        let token = Arc::new(AtomicBool::new(false));
        self.scan_cancel = Arc::clone(&token);
        token
    }

    fn accepts_scan_result(&self, tab_id: u64, generation: u64) -> bool {
        self.id == tab_id
            && self.generation == generation
            && !self.scan_cancel.load(Ordering::Relaxed)
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
    report: filesystem::OperationReport,
    moved_paths: Option<Vec<PathBuf>>,
}

struct CompletedOperation {
    id: u64,
    label: String,
    summary: String,
    outcome: OperationOutcome,
    finished_at: SystemTime,
    elapsed: Duration,
    items: Vec<filesystem::ItemResult>,
    counts: ItemCounts,
}

#[derive(Clone, Copy, Default)]
struct ItemCounts {
    completed: usize,
    skipped: usize,
    failed: usize,
    not_started: usize,
}

/// Average-rate estimate based on transferred byte progress, not wall-clock
/// samples. It is deliberately hidden until enough time and data exist.
fn transfer_progress_details(progress: &Progress, elapsed: Duration) -> String {
    let mut detail = if progress.total > 0 {
        format!(
            "{} / {}",
            filesystem::human_size(progress.transferred),
            filesystem::human_size(progress.total)
        )
    } else {
        format!("{} transferred", filesystem::human_size(progress.transferred))
    };
    detail.push_str(&format!(" · {} elapsed", elapsed_label(elapsed)));
    if elapsed >= Duration::from_secs(1) && progress.transferred > 0 {
        let rate = progress.transferred as f64 / elapsed.as_secs_f64();
        if rate.is_finite() && rate > 0.0 {
            detail.push_str(&format!(" · {}/s avg", filesystem::human_size(rate as u64)));
            if progress.total > progress.transferred {
                let remaining = (progress.total - progress.transferred) as f64 / rate;
                if remaining.is_finite() && remaining <= 24.0 * 3600.0 * 365.0 {
                    detail.push_str(&format!(
                        " · ~{} remaining",
                        elapsed_label(Duration::from_secs_f64(remaining.ceil()))
                    ));
                }
            }
        }
    }
    detail
}

fn elapsed_label(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

impl ItemCounts {
    fn from_items(items: &[filesystem::ItemResult]) -> Self {
        let mut counts = Self::default();
        for item in items {
            match item.state {
                filesystem::ItemState::Completed => counts.completed += 1,
                filesystem::ItemState::Skipped => counts.skipped += 1,
                filesystem::ItemState::Failed | filesystem::ItemState::Cancelled => {
                    counts.failed += 1;
                }
                filesystem::ItemState::NotStarted => counts.not_started += 1,
            }
        }
        counts
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OperationOutcome {
    Succeeded,
    Cancelled,
    Failed,
}

enum OperationEvent {
    Progress(Progress),
    Finished(OperationResult),
}

struct PastePreviewResult {
    generation: u64,
    result: Result<filesystem::TransferPlan, String>,
}

#[derive(Clone)]
struct Clipboard {
    paths: Vec<PathBuf>,
    cut: bool,
}

/// Reconcile cut paths after any result, including a partially failed batch.
/// `None` means nothing moved and the original clipboard stays untouched.
fn remaining_cut_paths(items: &[filesystem::ItemResult]) -> Option<Vec<PathBuf>> {
    if !items
        .iter()
        .any(|item| item.state == filesystem::ItemState::Completed)
    {
        return None;
    }
    Some(
        items
            .iter()
            .filter(|item| item.state != filesystem::ItemState::Completed)
            .map(|item| item.source.clone())
            .collect(),
    )
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
    Name {
        mode: NameMode,
        value: String,
    },
    BatchRename {
        paths: Vec<PathBuf>,
        prefix: String,
        suffix: String,
        find: String,
        replace: String,
        numbering: bool,
        number_start: u32,
        number_width: usize,
    },
    PasteOptions {
        clipboard: Clipboard,
        destination: PathBuf,
        conflict: ConflictPolicy,
        preview_generation: u64,
        preview: Option<Result<filesystem::TransferPlan, String>>,
    },
    Trash(Vec<PathBuf>),
    Delete(Vec<PathBuf>),
    Properties {
        path: PathBuf,
        mode: String,
    },
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
    paste_preview_sender: Sender<PastePreviewResult>,
    paste_preview_receiver: Receiver<PastePreviewResult>,
    paste_preview_generation: u64,
    operation_queue: OperationQueue,
    active_operation: Option<(u64, String)>,
    active_started: Option<Instant>,
    queue_paused: bool,
    show_operations: bool,
    recent_operations: VecDeque<CompletedOperation>,
    recovery_sender: Sender<(u64, std::io::Result<Vec<JournalRecord>>)>,
    recovery_receiver: Receiver<(u64, std::io::Result<Vec<JournalRecord>>)>,
    recovery_generation: u64,
    recovery_records: Vec<JournalRecord>,
    recovery_error: Option<String>,
    recovery_loading: bool,
    recovery_open: bool,
    clipboard: Option<Clipboard>,
    consumed_cut_paths: Option<Vec<PathBuf>>,
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
        let (paste_preview_sender, paste_preview_receiver) = mpsc::channel();
        let (recovery_sender, recovery_receiver) = mpsc::channel();
        // Explicit CLI paths take precedence over the saved session. Restored
        // paths are inspected by asynchronous scans, never by synchronous stat.
        let from_session = locations.is_empty();
        let (startup_paths, startup_active) = if from_session {
            config.restored_session()
        } else {
            (locations, 0)
        };
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
            paste_preview_sender,
            paste_preview_receiver,
            paste_preview_generation: 0,
            operation_queue: OperationQueue::default(),
            active_operation: None,
            active_started: None,
            queue_paused: false,
            show_operations: false,
            recent_operations: VecDeque::new(),
            recovery_sender,
            recovery_receiver,
            recovery_generation: 0,
            recovery_records: Vec::new(),
            recovery_error: None,
            recovery_loading: false,
            recovery_open: false,
            clipboard: None,
            consumed_cut_paths: None,
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
        if startup_paths.is_empty() {
            explorer.new_tab(config::home());
        } else {
            for location in startup_paths {
                let path = if !from_session && location.is_file() {
                    location.parent().unwrap_or(Path::new("/")).to_path_buf()
                } else {
                    location
                };
                explorer.new_tab(path);
            }
            explorer.active = startup_active.min(explorer.tabs.len() - 1);
            explorer.remember_session();
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

    fn remember_session(&mut self) {
        let paths: Vec<_> = self.tabs.iter().map(|tab| tab.path.clone()).collect();
        if self.config.remember_session(&paths, self.active) {
            self.dirty_config = true;
        }
    }

    fn new_tab(&mut self, path: PathBuf) {
        let id = self.next_tab;
        self.next_tab += 1;
        self.tabs.push(Tab::new(id, path));
        self.active = self.tabs.len() - 1;
        self.scan(self.active);
        self.remember_session();
    }

    fn close_tab(&mut self, index: usize) {
        if self.tabs.len() == 1 {
            self.navigate(config::home());
            return;
        }
        self.tabs[index].cancel_scan();
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
        self.remember_session();
    }

    fn scan(&mut self, index: usize) {
        let tab = &mut self.tabs[index];
        let cancel = tab.next_scan_token();
        tab.generation += 1;
        tab.loading = true;
        tab.error = None;
        let task = (tab.id, tab.generation, tab.path.clone());
        let recursive = tab.recursive_search && !tab.query.is_empty();
        let query = tab.query.clone();
        let show_hidden = self.config.show_hidden;
        let sender = self.scan_sender.clone();
        thread::spawn(move || {
            let result = if recursive {
                filesystem::search_cancellable(&task.2, &query, show_hidden, &cancel)
            } else {
                filesystem::scan_cancellable(&task.2, &cancel)
            };
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let dir_mtime = std::fs::metadata(&task.2)
                .and_then(|meta| meta.modified())
                .ok();
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = sender.send(ScanResult {
                tab: task.0,
                generation: task.1,
                result,
                dir_mtime,
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
        self.remember_session();
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
            self.remember_session();
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
                .position(|tab| tab.accepts_scan_result(message.tab, message.generation))
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
                    let OperationResult {
                        id,
                        report,
                        moved_paths,
                    } = message;
                    self.busy = false;
                    let label = self
                        .active_operation
                        .take()
                        .map(|(_, label)| label)
                        .unwrap_or_else(|| "File operation".into());
                    self.progress = None;
                    let elapsed = self
                        .active_started
                        .take()
                        .map(|start| start.elapsed())
                        .unwrap_or_default();
                    if let Some(moved) = moved_paths
                        && self
                            .clipboard
                            .as_ref()
                            .is_some_and(|clipboard| clipboard.cut && clipboard.paths == moved)
                        && let Some(remaining) = remaining_cut_paths(&report.items)
                    {
                        // A cut batch may have partially succeeded even when its overall
                        // result is an error. Never offer completed paths for a retry.
                        self.clipboard = if remaining.is_empty() {
                            None
                        } else {
                            Some(Clipboard {
                                paths: remaining,
                                cut: true,
                            })
                        };
                        // The external desktop clipboard may still contain the original
                        // full cut list. Ignore it while honoring the internal remainder.
                        if !self.consumed_cut_paths.as_ref().is_some_and(|consumed| {
                            moved.iter().all(|path| consumed.contains(path))
                        }) {
                            self.consumed_cut_paths = Some(moved);
                        }
                    }
                    let (summary, outcome) = match report.result {
                        Ok(success) => (success, OperationOutcome::Succeeded),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                            self.queue_paused = !self.operation_queue.is_empty();
                            let detail = if self.queue_paused {
                                format!(
                                    "Cancelled: {error}. Completed items remain; waiting copies are paused."
                                )
                            } else {
                                format!(
                                    "Cancelled: {error}. Previously completed items remain in place."
                                )
                            };
                            (detail, OperationOutcome::Cancelled)
                        }
                        Err(error) => {
                            self.queue_paused = !self.operation_queue.is_empty();
                            (
                                format!("Operation failed: {error}"),
                                OperationOutcome::Failed,
                            )
                        }
                    };
                    self.status = Some((
                        format!("#{id}: {summary}"),
                        outcome == OperationOutcome::Failed,
                    ));
                    self.recent_operations.push_front(CompletedOperation {
                        id,
                        label,
                        summary,
                        outcome,
                        finished_at: SystemTime::now(),
                        elapsed,
                        counts: ItemCounts::from_items(&report.items),
                        items: report.items,
                    });
                    if self.recent_operations.len() > MAX_RECENT_OPERATIONS {
                        self.recent_operations.pop_back();
                    }
                    self.refresh_all();
                    self.check_unfinished_moves();
                    self.start_next_operation();
                }
            }
        }
        while let Ok(message) = self.paste_preview_receiver.try_recv() {
            if let Some(Dialog::PasteOptions {
                preview_generation,
                preview,
                ..
            }) = self.dialog.as_mut()
                && *preview_generation == message.generation
            {
                *preview = Some(message.result);
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
                            if let Some(started) = self.active_started {
                                ui.small(transfer_progress_details(progress, started.elapsed()));
                            }
                        }
                    }
                    if !self.operation_queue.is_empty() {
                        ui.label(format!("{} waiting", self.operation_queue.len()));
                        if self.queue_paused {
                            ui.colored_label(Color32::from_rgb(165, 83, 30), "Paused after error");
                        }
                    }
                    // Several rice shells intentionally omit the bottom status bar.
                    // Their file-operation errors must still be visible without
                    // opening the Transfers dialog or switching themes.
                    if !theme.shell.status_bar.unwrap_or(true)
                        && let Some((message, failed)) = &self.status
                    {
                        ui.separator();
                        ui.colored_label(
                            if *failed {
                                Color32::LIGHT_RED
                            } else {
                                theme.palette.accent()
                            },
                            shorten(message, 72),
                        )
                        .on_hover_text(message);
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
                        if let Some(started) = self.active_started {
                            ui.small(transfer_progress_details(progress, started.elapsed()));
                        }
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
                ui.separator();
                ui.strong(format!("Recent results ({})", self.recent_operations.len()));
                ui.small("This session only. A failed batch may have completed earlier items.");
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for entry in &self.recent_operations {
                        let outcome = match entry.outcome {
                            OperationOutcome::Succeeded => "Completed",
                            OperationOutcome::Cancelled => "Cancelled",
                            OperationOutcome::Failed => "Failed",
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(format!("#{} · {outcome}", entry.id));
                            ui.label(&entry.label);
                            ui.small(filesystem::age(Some(entry.finished_at)));
                            ui.small(format!("Took {}", elapsed_label(entry.elapsed)));
                        });
                        if entry.outcome == OperationOutcome::Failed {
                            ui.colored_label(Color32::LIGHT_RED, &entry.summary);
                        } else {
                            ui.small(&entry.summary);
                        }
                        if !entry.items.is_empty() {
                            let ItemCounts {
                                completed,
                                skipped,
                                failed,
                                not_started,
                            } = entry.counts;
                            ui.collapsing(
                                format!(
                                    "Per-file results: {completed} done · {skipped} skipped · {failed} errors · {not_started} not started"
                                ),
                                |ui| {
                                    egui::ScrollArea::vertical()
                                        .max_height(190.0)
                                        .show_rows(ui, 25.0, entry.items.len(), |ui, range| {
                                            for item in &entry.items[range] {
                                                ui.horizontal(|ui| {
                                                    let state = match item.state {
                                                        filesystem::ItemState::NotStarted => {
                                                            "Not started"
                                                        }
                                                        filesystem::ItemState::Completed => "Done",
                                                        filesystem::ItemState::Skipped => "Skipped",
                                                        filesystem::ItemState::Failed => "Failed",
                                                        filesystem::ItemState::Cancelled => {
                                                            "Cancelled"
                                                        }
                                                    };
                                                    ui.small(state);
                                                    let name = item.source.display().to_string();
                                                    let mut details = name.clone();
                                                    if let Some(target) = &item.target {
                                                        details.push_str(&format!(
                                                            "\nDestination: {}",
                                                            target.display()
                                                        ));
                                                    }
                                                    if let Some(error) = &item.detail {
                                                        details.push_str(&format!("\n{error}"));
                                                    }
                                                    ui.monospace(shorten(&name, 54))
                                                        .on_hover_text(details);
                                                });
                                            }
                                        });
                                },
                            );
                        }
                        ui.separator();
                    }
                });
            });
        self.show_operations = open;
        if let Some(id) = remove
            && self.operation_queue.remove(id)
        {
            self.status = Some((format!("Removed waiting copy #{id}"), false));
            if self.operation_queue.is_empty() {
                self.queue_paused = false;
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
        self.active_started = Some(Instant::now());
        self.progress = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&self.cancel);
        let moved_paths = match &job.action {
            Action::Move { sources, .. } => Some(sources.clone()),
            _ => None,
        };
        let sender = self.op_sender.clone();
        thread::spawn(move || {
            let report = filesystem::perform_with_report(&job.action, &cancel, &mut |progress| {
                let _ = sender.send(OperationEvent::Progress(progress));
            });
            let _ = sender.send(OperationEvent::Finished(OperationResult {
                id: job.id,
                report,
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
        self.consumed_cut_paths = None;
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

    fn clipboard_for_paste(&self) -> Option<Clipboard> {
        let desktop = clipboard::paste_files().map(|files| Clipboard {
            paths: files.paths,
            cut: files.cut,
        });
        if desktop.as_ref().is_some_and(|files| {
            files.cut && self.consumed_cut_paths.as_ref() == Some(&files.paths)
        }) {
            return self.clipboard.clone();
        }
        desktop.or_else(|| self.clipboard.clone())
    }

    fn paste_action(
        clipboard: Clipboard,
        destination: PathBuf,
        conflict: ConflictPolicy,
    ) -> Action {
        if clipboard.cut {
            Action::Move {
                sources: clipboard.paths,
                destination,
                conflict,
            }
        } else {
            Action::Copy {
                sources: clipboard.paths,
                destination,
                conflict,
            }
        }
    }

    fn paste(&mut self) {
        if let Some(clipboard) = self.clipboard_for_paste() {
            self.clipboard = Some(clipboard.clone());
            self.run_action(Self::paste_action(
                clipboard,
                self.current().path.clone(),
                ConflictPolicy::KeepBoth,
            ));
        }
    }

    fn paste_with_options(&mut self) {
        if let Some(clipboard) = self.clipboard_for_paste() {
            let destination = self.current().path.clone();
            let conflict = ConflictPolicy::KeepBoth;
            let preview_generation = self.request_paste_preview(&clipboard, &destination, conflict);
            self.dialog = Some(Dialog::PasteOptions {
                clipboard,
                destination,
                conflict,
                preview_generation,
                preview: None,
            });
        } else {
            self.status = Some(("No files on the clipboard".into(), true));
        }
    }

    fn request_paste_preview(
        &mut self,
        clipboard: &Clipboard,
        destination: &Path,
        conflict: ConflictPolicy,
    ) -> u64 {
        self.paste_preview_generation = self.paste_preview_generation.saturating_add(1);
        let generation = self.paste_preview_generation;
        let sources = clipboard.paths.clone();
        let destination = destination.to_path_buf();
        let moving = clipboard.cut;
        let sender = self.paste_preview_sender.clone();
        thread::spawn(move || {
            let result = filesystem::preview_transfer(&sources, &destination, conflict, moving)
                .map_err(|error| error.to_string());
            let _ = sender.send(PastePreviewResult { generation, result });
        });
        generation
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
                        conflict: ConflictPolicy::KeepBoth,
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
        let paths = self.selected_paths();
        if paths.len() > 1 {
            if paths.len() > filesystem::MAX_BATCH_RENAME {
                self.status = Some((
                    format!(
                        "Batch rename supports at most {} selected items",
                        filesystem::MAX_BATCH_RENAME
                    ),
                    true,
                ));
            } else {
                self.dialog = Some(Dialog::BatchRename {
                    paths,
                    prefix: String::new(),
                    suffix: String::new(),
                    find: String::new(),
                    replace: String::new(),
                    numbering: false,
                    number_start: 1,
                    number_width: 3,
                });
            }
            return;
        }
        if let Some(path) = paths.first() {
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

    fn chrome_region(&mut self, ui: &mut egui::Ui, top: bool) {
        let theme = self.theme.clone();
        let components = if top {
            theme.top_components()
        } else {
            theme.bottom_components()
        };
        if components.is_empty() {
            return;
        }
        let panel = if top {
            egui::Panel::top("chrome")
        } else {
            egui::Panel::bottom("secondary_chrome")
        };
        panel
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
                    self.remember_session();
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
                    self.current().cancel_scan();
                    self.current_mut().search_pending = Some(Instant::now());
                } else {
                    self.rebuild_visible(self.active);
                }
            }
            if !self.current().query.is_empty()
                && ui.small_button("×").on_hover_text("Clear search").clicked()
            {
                self.current_mut().query.clear();
                self.current_mut().search_pending = None;
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
        let sidebar_id = egui::IdSalt::new(("places", theme.id.as_str()));
        let panel = match theme.sidebar_position() {
            SidebarPosition::Left => egui::Panel::left(sidebar_id),
            SidebarPosition::Right => egui::Panel::right(sidebar_id),
        };
        panel
            .resizable(true)
            .default_size(theme.sidebar_width())
            .min_size(120.0)
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
                            SidebarSection::Tasks => "FILE TASKS",
                            SidebarSection::Places => "PLACES",
                            SidebarSection::Favorites => "FAVORITES",
                            SidebarSection::Volumes => "VOLUMES",
                        };
                        let mut render_section = |ui: &mut egui::Ui| match section {
                            SidebarSection::Tasks => self.sidebar_tasks(ui),
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
                        };
                        if theme.sidebar_section_mode() == SidebarSectionMode::Collapsible {
                            egui::CollapsingHeader::new(title)
                                .id_salt((theme.id.as_str(), index))
                                .default_open(true)
                                .show(ui, |ui| render_section(ui));
                        } else {
                            ui.label(
                                egui::RichText::new(title)
                                    .small()
                                    .strong()
                                    .color(theme.palette.muted()),
                            );
                            render_section(ui);
                        }
                    }
                });
            });
    }

    fn sidebar_tasks(&mut self, ui: &mut egui::Ui) {
        let selection = self.current().selected.len();
        ui.add_space(8.0);
        if ui.button("New folder").clicked() {
            self.dialog = Some(Dialog::Name {
                mode: NameMode::Folder(self.current().path.clone()),
                value: String::new(),
            });
        }
        if ui
            .add_enabled(selection > 0, egui::Button::new("Copy selected"))
            .clicked()
        {
            self.copy_selection(false);
        }
        if ui
            .add_enabled(selection > 0, egui::Button::new("Cut selected"))
            .clicked()
        {
            self.copy_selection(true);
        }
        if ui.button("Paste here").clicked() {
            self.paste();
        }
        if ui
            .add_enabled(selection > 0, egui::Button::new("Rename"))
            .clicked()
        {
            self.rename_selected();
        }
        if ui
            .add_enabled(selection > 0, egui::Button::new("Move to Trash"))
            .clicked()
        {
            self.ask_trash();
        }
        if ui
            .add_enabled(selection == 1, egui::Button::new("Properties"))
            .clicked()
            && let Some(path) = self.selected_paths().first().cloned()
        {
            self.open_properties(path);
        }
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.preview_path.clone() else {
            return;
        };
        let theme = self.theme.clone();
        let preview_id = egui::IdSalt::new(("preview", theme.id.as_str()));
        let panel = match theme.preview_position() {
            SidebarPosition::Left => egui::Panel::left(preview_id),
            SidebarPosition::Right => egui::Panel::right(preview_id),
        };
        panel
            .resizable(true)
            .default_size(theme.preview_width())
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
                        if ui.button("Paste with options…").clicked() {
                            self.paste_with_options();
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
                                conflict: ConflictPolicy::KeepBoth,
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
                    ui.menu_button("Session", |ui| {
                        if ui
                            .checkbox(&mut self.config.restore_session, "Restore tabs on startup")
                            .changed()
                        {
                            self.remember_session();
                            self.dirty_config = true;
                        }
                        ui.small("Remembers up to 32 local paths, not file operations.");
                        if ui
                            .add_enabled(
                                self.config.restore_session,
                                egui::Button::new("Save current tabs now"),
                            )
                            .clicked()
                        {
                            self.remember_session();
                            self.dirty_config = true;
                            self.status = Some(("Tab session saved".into(), false));
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
                        conflict: ConflictPolicy::KeepBoth,
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
        let mut refresh_paste_preview = None;
        if let Some(dialog) = self.dialog.as_mut() {
            match dialog {
                Dialog::PasteOptions {
                    clipboard,
                    destination,
                    conflict,
                    preview,
                    ..
                } => {
                    egui::Window::new("Paste with options")
                        .collapsible(false)
                        .resizable(true)
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(context, |ui| {
                            ui.label(format!(
                                "{} {} item(s) into {}",
                                if clipboard.cut { "Move" } else { "Copy" },
                                clipboard.paths.len(),
                                destination.display()
                            ));
                            ui.separator();
                            let mut policy_changed = ui.radio_value(
                                conflict,
                                ConflictPolicy::KeepBoth,
                                "Keep both — rename incoming items when names collide",
                            ).changed();
                            policy_changed |= ui.radio_value(
                                conflict,
                                ConflictPolicy::SkipExisting,
                                "Skip existing — leave matching destination items untouched",
                            ).changed();
                            policy_changed |= ui.radio_value(
                                conflict,
                                ConflictPolicy::AbortOnConflict,
                                "Require no conflicts — cancel the entire batch if a name collides",
                            ).changed();
                            if policy_changed {
                                refresh_paste_preview =
                                    Some((clipboard.clone(), destination.clone(), *conflict));
                            }
                            ui.separator();
                            let can_start = if policy_changed {
                                false
                            } else {
                                match preview {
                                    None => {
                                        ui.horizontal(|ui| {
                                            ui.spinner();
                                            ui.small("Checking all selected paths in the background…");
                                        });
                                        false
                                    }
                                    Some(Err(error)) => {
                                        ui.colored_label(Color32::LIGHT_RED, error);
                                        false
                                    }
                                    Some(Ok(plan)) => {
                                        ui.strong(format!(
                                            "Preflight: {} selected · {} name collision(s) · {} renamed · {} skipped",
                                            plan.items.len(), plan.collisions,
                                            plan.renamed, plan.skipped
                                        ));
                                        egui::ScrollArea::vertical()
                                            .max_height(190.0)
                                            .show_rows(ui, 20.0, plan.items.len(), |ui, rows| {
                                                for planned in &plan.items[rows] {
                                                    let decision = if planned.disposition == filesystem::PlannedDisposition::Skip {
                                                        "Skip"
                                                    } else if planned.renamed {
                                                        "Rename incoming"
                                                    } else {
                                                        "Transfer"
                                                    };
                                                    let source = planned.source.display().to_string();
                                                    ui.horizontal(|ui| {
                                                        ui.small(if planned.collision && !planned.renamed {
                                                            format!("{decision} · collision")
                                                        } else {
                                                            decision.to_owned()
                                                        });
                                                        ui.monospace(shorten(&source, 45))
                                                            .on_hover_text(format!(
                                                                "Source: {}\nDestination: {}",
                                                                planned.source.display(),
                                                                planned.target.display()
                                                            ));
                                                    });
                                                }
                                            });
                                        true
                                    }
                                }
                            };
                            ui.small("Preview only: the worker rechecks sources and destination before execution. No existing file is overwritten. Concurrent changes may still stop the transfer safely.");
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    dismiss = true;
                                }
                                if ui.add_enabled(can_start, egui::Button::new("Start transfer")).clicked() {
                                    action = Some(Self::paste_action(
                                        clipboard.clone(),
                                        destination.clone(),
                                        *conflict,
                                    ));
                                    dismiss = true;
                                }
                            });
                        });
                }
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
                Dialog::BatchRename {
                    paths,
                    prefix,
                    suffix,
                    find,
                    replace,
                    numbering,
                    number_start,
                    number_width,
                } => {
                    egui::Window::new(format!("Batch rename ({} items)", paths.len()))
                        .collapsible(false)
                        .resizable(true)
                        .default_width(580.0)
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(context, |ui| {
                            ui.label("Preview every new name before applying the changes.");
                            ui.horizontal(|ui| {
                                ui.label("Prefix:");
                                ui.add_sized(
                                    [220.0, 26.0],
                                    egui::TextEdit::singleline(prefix),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Suffix:");
                                ui.add_sized(
                                    [220.0, 26.0],
                                    egui::TextEdit::singleline(suffix),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Find:");
                                ui.add_sized([170.0, 26.0], egui::TextEdit::singleline(find));
                                ui.label("Replace:");
                                ui.add_sized([170.0, 26.0], egui::TextEdit::singleline(replace));
                            });
                            ui.horizontal(|ui| {
                                ui.checkbox(numbering, "Add sequence numbers");
                                if *numbering {
                                    ui.label("Start:");
                                    ui.add(egui::DragValue::new(number_start).range(1..=99_999_999));
                                    ui.label("Digits:");
                                    ui.add(egui::DragValue::new(number_width).range(1..=8));
                                }
                            });
                            let options = filesystem::BatchRenameOptions {
                                prefix,
                                suffix,
                                find,
                                replace,
                                numbering: *numbering,
                                number_start: *number_start,
                                number_width: *number_width,
                            };
                            match filesystem::plan_batch_rename_with_options(paths, &options) {
                                Ok(changes) => {
                                    ui.separator();
                                    ui.strong("Preview (original → new name)");
                                    egui::ScrollArea::vertical()
                                        .max_height(245.0)
                                        .show(ui, |ui| {
                                            for (source, name) in &changes {
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.monospace(
                                                        source
                                                            .file_name()
                                                            .unwrap_or_default()
                                                            .to_string_lossy(),
                                                    );
                                                    ui.label("→");
                                                    ui.monospace(name.to_string_lossy());
                                                });
                                            }
                                        });
                                    ui.label(
                                        "Targets are checked before renaming. Existing names are never overwritten; if a later rename fails, earlier changes remain.",
                                    );
                                    ui.horizontal(|ui| {
                                        if ui.button("Cancel").clicked() {
                                            dismiss = true;
                                        }
                                        if ui.button("Rename selected").clicked() {
                                            action = Some(Action::BatchRename { changes });
                                            dismiss = true;
                                        }
                                    });
                                }
                                Err(error) => {
                                    ui.colored_label(
                                        Color32::from_rgb(170, 48, 43),
                                        error.to_string(),
                                    );
                                    if ui.button("Cancel").clicked() {
                                        dismiss = true;
                                    }
                                }
                            }
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
        if let Some((clipboard, destination, conflict)) = refresh_paste_preview {
            let generation = self.request_paste_preview(&clipboard, &destination, conflict);
            if let Some(Dialog::PasteOptions {
                preview_generation,
                preview,
                ..
            }) = self.dialog.as_mut()
            {
                *preview_generation = generation;
                *preview = None;
            }
        }
        if let Some(action) = action {
            if let Action::Copy { sources, .. } | Action::Move { sources, .. } = &action {
                self.clipboard = Some(Clipboard {
                    paths: sources.clone(),
                    cut: matches!(&action, Action::Move { .. }),
                });
            }
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
                conflict: ConflictPolicy::KeepBoth,
            });
        }
        if self.dialog.is_none() {
            self.keyboard(&context);
        }
        self.chrome_region(ui, true);
        self.recovery_banner(ui);
        self.transfer_strip(ui);
        if self.theme.shell.status_bar.unwrap_or(true) {
            self.status_bar(ui);
        }
        self.chrome_region(ui, false);
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
        let waiting_for_preview = matches!(
            &self.dialog,
            Some(Dialog::PasteOptions { preview: None, .. })
        );
        let idle_delay =
            if self.busy || waiting_for_preview || self.tabs.iter().any(|tab| tab.loading) {
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
    use super::{
        Action, NameMode, Tab, elapsed_label, next_search_repaint_delay, remaining_cut_paths,
        transfer_progress_details,
    };
    use crate::filesystem::{ItemResult, ItemState, Progress};
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
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

    #[test]
    fn replacing_tab_scan_token_cancels_old_worker_only() {
        let mut tab = Tab::new(9, PathBuf::from("/somewhere"));
        let old = tab.scan_cancel.clone();
        let current = tab.next_scan_token();
        assert!(old.load(Ordering::Relaxed));
        assert!(!current.load(Ordering::Relaxed));
        assert!(tab.accepts_scan_result(9, 0));
        let subsequent = tab.next_scan_token();
        assert!(current.load(Ordering::Relaxed));
        assert!(!subsequent.load(Ordering::Relaxed));
        assert!(!tab.accepts_scan_result(1, 0));
        assert!(!tab.accepts_scan_result(9, 1));
        tab.cancel_scan();
        assert!(!tab.accepts_scan_result(9, 0));
    }

    #[test]
    fn partially_failed_cut_keeps_only_unmoved_files_on_clipboard() {
        let items = [
            ItemResult {
                source: PathBuf::from("/original/moved.txt"),
                target: Some(PathBuf::from("/target/moved.txt")),
                state: ItemState::Completed,
                detail: None,
            },
            ItemResult {
                source: PathBuf::from("/original/failed.txt"),
                target: None,
                state: ItemState::Failed,
                detail: Some("device disconnected".into()),
            },
            ItemResult {
                source: PathBuf::from("/original/unstarted.txt"),
                target: None,
                state: ItemState::NotStarted,
                detail: None,
            },
        ];
        assert_eq!(
            remaining_cut_paths(&items),
            Some(vec![
                PathBuf::from("/original/failed.txt"),
                PathBuf::from("/original/unstarted.txt")
            ])
        );
    }

    #[test]
    fn failed_cut_with_no_completed_items_keeps_original_clipboard() {
        let items = [ItemResult {
            source: PathBuf::from("/original/failed.txt"),
            target: None,
            state: ItemState::Failed,
            detail: None,
        }];
        assert_eq!(remaining_cut_paths(&items), None);
    }

    #[test]
    fn transfer_metrics_show_average_rate_and_reasonable_eta() {
        let progress = Progress {
            transferred: 4 * 1024 * 1024,
            total: 16 * 1024 * 1024,
            current: PathBuf::from("/file"),
            phase: "Copying",
        };
        let details = transfer_progress_details(&progress, Duration::from_secs(2));
        assert!(details.contains("4.0 MiB / 16.0 MiB"));
        assert!(details.contains("2.0 MiB/s avg"));
        assert!(details.contains("~6s remaining"));
    }

    #[test]
    fn transfer_metrics_hide_unreliable_rate_before_first_second() {
        let progress = Progress {
            transferred: 0,
            total: 0,
            current: PathBuf::from("/empty"),
            phase: "Copying",
        };
        let details = transfer_progress_details(&progress, Duration::from_millis(200));
        assert!(details.contains("0 B transferred"));
        assert!(!details.contains("/s avg"));
        assert!(!details.contains("remaining"));
        assert_eq!(elapsed_label(Duration::from_secs(71)), "1m 11s");
        assert_eq!(elapsed_label(Duration::from_secs(3661)), "1h 01m");
    }
}
