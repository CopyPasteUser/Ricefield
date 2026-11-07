use crate::{
    aqua::{self, AquaAction, AquaContext},
    archive,
    chromeos::{self, FilesAction, FilesContext, FilesNavIcon},
    classic98::{self, ClassicAction, ClassicContext, TreeIcon},
    clipboard,
    config::{self, Config},
    desktop, discovery, duplicates,
    file_view::{self, natural_cmp},
    filesystem::{self, Action, ConflictPolicy, Entry, Progress},
    finder::{self, FinderAction, FinderContext, FinderNavIcon},
    folder_tree::FolderTree,
    journal::{self, Inspection as JournalInspection, State as JournalState},
    keyring,
    location::Location,
    metadata, open_with,
    operation_queue::OperationQueue,
    preview, remote_download, remote_upload, remote_upload_journal,
    shell_components::{self, ToolbarAction},
    shell_renderer::{self, FileCellLayout, shorten},
    theme::{BUILTIN_IDS, ChromeComponent, ShellKind, SidebarPosition, SidebarSection, Theme},
    theme_editor::{self, ThemeEditor},
    thumbnail::ThumbnailManager,
    transfer_history,
    volume::{self, MountCredentials, VolumeAction},
    win10::{self, RibbonAction, RibbonContext, RibbonPage},
};
use eframe::egui::{self, Align2, Color32, Key, Modifiers, Pos2, Rect, Vec2};
use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);
const MAX_RECENT_OPERATIONS: usize = transfer_history::MAX_RECORDS;
const CLIPBOARD_WAIT: Duration = Duration::from_secs(2);
const MAX_CLOSED_TABS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteSort {
    Name,
    Kind,
    Size,
}

impl RemoteSort {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Kind => "Kind",
            Self::Size => "Size",
        }
    }
}
fn sort_remote_entries(entries: &mut [volume::RemoteEntry], by: RemoteSort, descending: bool) {
    entries.sort_by(|left, right| {
        // Folder grouping is invariant even when reversing a selected column.
        right.is_directory.cmp(&left.is_directory).then_with(|| {
            let order = match by {
                RemoteSort::Name => file_view::natural_cmp(
                    &volume::display_remote_name(&left.name),
                    &volume::display_remote_name(&right.name),
                ),
                RemoteSort::Kind => left.is_regular_file.cmp(&right.is_regular_file).reverse(),
                RemoteSort::Size => left.reported_size.cmp(&right.reported_size),
            };
            let order = if descending { order.reverse() } else { order };
            order.then_with(|| file_view::natural_cmp(&left.name, &right.name))
        })
    });
}

/// Keep selection and displayed rows aligned with the current filename query,
/// even while a bounded GIO network search is still in flight. Use decoded names
/// for matching but preserve encoded URIs for navigation and transfers.
fn remote_matches_query(entry: &volume::RemoteEntry, query: &str) -> bool {
    let query = query.trim();
    query.is_empty()
        || volume::display_remote_name(&entry.name)
            .to_lowercase()
            .contains(&query.to_lowercase())
}

/// None means every entry is visible: avoid allocating an index vector on
/// every frame for large unfiltered network folders.
fn visible_remote_indices(entries: &[volume::RemoteEntry], query: &str) -> Option<Vec<usize>> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    Some(
        entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                volume::display_remote_name(&entry.name)
                    .to_lowercase()
                    .contains(&needle)
                    .then_some(index)
            })
            .collect(),
    )
}

fn next_remote_selection(
    entries: &[volume::RemoteEntry],
    query: &str,
    current_uri: Option<&str>,
    down: bool,
) -> Option<String> {
    let visible = visible_remote_indices(entries, query);
    let count = visible.as_ref().map_or(entries.len(), Vec::len);
    if count == 0 {
        return None;
    }
    let current = current_uri.and_then(|uri| match &visible {
        Some(indices) => indices.iter().position(|&index| entries[index].uri == uri),
        None => entries.iter().position(|entry| entry.uri == uri),
    });
    let next = match (current, down) {
        (None, true) => 0,
        (None, false) => count - 1,
        (Some(index), true) => (index + 1).min(count - 1),
        (Some(index), false) => index.saturating_sub(1),
    };
    Some(
        entries[visible.as_ref().map_or(next, |indices| indices[next])]
            .uri
            .clone(),
    )
}

fn remote_sort_header(
    ui: &mut egui::Ui,
    theme: &Theme,
    selected: RemoteSort,
    descending: bool,
) -> Option<RemoteSort> {
    let available = ui.available_width();
    let compact = available < 425.0;
    let mut change = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (field, proportion) in [
            (RemoteSort::Name, 0.65),
            (RemoteSort::Kind, 0.20),
            (RemoteSort::Size, 0.15),
        ] {
            if compact && field != RemoteSort::Name {
                continue;
            }
            let width = if compact {
                available
            } else {
                available * proportion
            };
            let arrow = if selected == field {
                if descending { " v" } else { " ^" }
            } else {
                ""
            };
            if ui
                .add_sized(
                    [width.max(1.0), 25.0],
                    egui::Button::new(format!("{}{arrow}", field.label()))
                        .fill(theme.palette.chrome())
                        .stroke(egui::Stroke::new(0.6, theme.palette.border()))
                        .corner_radius(if theme.shell_kind() == ShellKind::Chromeos {
                            egui::CornerRadius::same(6)
                        } else {
                            egui::CornerRadius::ZERO
                        }),
                )
                .clicked()
            {
                change = Some(field);
            }
        }
    });
    change
}

/// Keep navigation targets as validated, encoded URIs. Decoding is for labels
/// only: percent-encoded slash characters must never become path separators.
fn remote_breadcrumbs(uri: &str) -> Vec<(String, String)> {
    if !matches!(Location::parse(uri.into()), Ok(Location::Remote(_))) {
        return Vec::new();
    }
    let Some((scheme, remainder)) = uri.split_once("://") else {
        return Vec::new();
    };
    let mut pieces = remainder.split('/').filter(|piece| !piece.is_empty());
    let Some(host) = pieces.next() else {
        return Vec::new();
    };
    let mut current = format!("{scheme}://{host}");
    let mut crumbs = vec![(host.to_owned(), current.clone())];
    for piece in pieces.take(20) {
        current.push('/');
        current.push_str(piece);
        if matches!(
            Location::parse(current.clone().into()),
            Ok(Location::Remote(_))
        ) {
            crumbs.push((volume::display_remote_name(piece), current.clone()));
        }
    }
    crumbs
}

/// Key events retain the modifiers at press time even if the whole chord is
/// pressed and released between frames. The frame's final modifiers do not.
fn pressed_keys(context: &egui::Context) -> Vec<(Key, Modifiers)> {
    context.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
            .collect()
    })
}

struct Tab {
    id: u64,
    path: PathBuf,
    address: String,
    remote_uri: Option<String>,
    remote_entries: Vec<volume::RemoteEntry>,
    remote_selected_uri: Option<String>,
    remote_sort: RemoteSort,
    remote_sort_descending: bool,
    remote_preview_uri: Option<String>,
    remote_preview_result: Option<Result<String, String>>,
    remote_preview_generation: u64,
    remote_preview_cancel: Arc<AtomicBool>,
    query: String,
    back: Vec<Location>,
    forward: Vec<Location>,
    entries: Vec<Entry>,
    folded_names: Vec<String>,
    visible: Vec<usize>,
    selected: BTreeSet<PathBuf>,
    pending_reveal: BTreeSet<PathBuf>,
    last_selected: Option<usize>,
    generation: u64,
    loading: bool,
    error: Option<String>,
    scan_warning: Option<String>,
    dir_mtime: Option<SystemTime>,
    recursive_search: bool,
    content_search: bool,
    search_pending: Option<Instant>,
    scan_cancel: Arc<AtomicBool>,
}

impl Tab {
    fn new(id: u64, path: PathBuf) -> Self {
        Self {
            id,
            address: path.display().to_string(),
            path,
            remote_uri: None,
            remote_entries: Vec::new(),
            remote_selected_uri: None,
            remote_sort: RemoteSort::Name,
            remote_sort_descending: false,
            remote_preview_uri: None,
            remote_preview_result: None,
            remote_preview_generation: 0,
            remote_preview_cancel: Arc::new(AtomicBool::new(false)),
            query: String::new(),
            back: Vec::new(),
            forward: Vec::new(),
            entries: Vec::new(),
            folded_names: Vec::new(),
            visible: Vec::new(),
            selected: BTreeSet::new(),
            pending_reveal: BTreeSet::new(),
            last_selected: None,
            generation: 0,
            loading: false,
            error: None,
            scan_warning: None,
            dir_mtime: None,
            recursive_search: false,
            content_search: false,
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

    fn clear_remote_preview(&mut self) {
        self.remote_preview_cancel.store(true, Ordering::Relaxed);
        self.remote_preview_uri = None;
        self.remote_preview_result = None;
    }

    fn next_remote_preview_token(&mut self, uri: String) -> Arc<AtomicBool> {
        self.clear_remote_preview();
        self.remote_preview_generation = self.remote_preview_generation.wrapping_add(1);
        self.remote_preview_uri = Some(uri);
        let token = Arc::new(AtomicBool::new(false));
        self.remote_preview_cancel = Arc::clone(&token);
        token
    }

    fn accepts_remote_preview(&self, tab_id: u64, generation: u64, uri: &str) -> bool {
        self.id == tab_id
            && self.remote_preview_generation == generation
            && self.remote_preview_uri.as_deref() == Some(uri)
            && !self.remote_preview_cancel.load(Ordering::Relaxed)
    }

    fn location(&self) -> Location {
        self.remote_uri
            .as_ref()
            .and_then(|uri| Location::parse(uri.clone().into()).ok())
            .unwrap_or_else(|| Location::Local(self.path.clone()))
    }

    fn set_location(&mut self, location: Location) {
        self.clear_remote_preview();
        match location {
            Location::Local(path) => {
                self.path = path;
                self.remote_uri = None;
                self.address = self.path.display().to_string();
            }
            Location::Remote(remote) => {
                self.address = remote.uri.clone();
                self.remote_uri = Some(remote.uri);
            }
        }
        self.remote_entries.clear();
        self.remote_selected_uri = None;
        self.entries.clear();
        self.folded_names.clear();
        self.visible.clear();
        self.selected.clear();
        self.pending_reveal.clear();
        self.last_selected = None;
        self.query.clear();
        self.search_pending = None;
        self.dir_mtime = None;
        self.scan_warning = None;
    }

    fn visit(&mut self, location: Location) {
        self.back.push(self.location());
        if self.back.len() > 100 {
            self.back.remove(0);
        }
        self.forward.clear();
        self.set_location(location);
    }

    fn travel(&mut self, backwards: bool) -> bool {
        let target = if backwards {
            self.back.pop()
        } else {
            self.forward.pop()
        };
        let Some(target) = target else {
            return false;
        };
        let previous = self.location();
        let opposite = if backwards {
            &mut self.forward
        } else {
            &mut self.back
        };
        opposite.push(previous);
        if opposite.len() > 100 {
            opposite.remove(0);
        }
        self.set_location(target);
        true
    }
}

/// Session-local navigation state. Do not copy a tab's scan/preview tokens:
/// background workers must never be able to write into a duplicated tab.
#[derive(Clone)]
struct TabSnapshot {
    location: Location,
    fallback_path: PathBuf,
    query: String,
    back: Vec<Location>,
    forward: Vec<Location>,
    recursive_search: bool,
    content_search: bool,
}

impl TabSnapshot {
    fn capture(tab: &Tab) -> Self {
        Self {
            location: tab.location(),
            fallback_path: tab.path.clone(),
            query: tab.query.clone(),
            back: tab.back.clone(),
            forward: tab.forward.clone(),
            recursive_search: tab.recursive_search,
            content_search: tab.content_search,
        }
    }

    fn restore(self, id: u64) -> Tab {
        let mut tab = Tab::new(id, self.fallback_path);
        tab.set_location(self.location);
        tab.query = self.query;
        tab.back = self.back;
        tab.forward = self.forward;
        tab.recursive_search = self.recursive_search;
        tab.content_search = self.content_search;
        tab
    }
}

/// An invisible selection must never be acted on by Copy, Move or Delete.
fn retain_visible_selection(tab: &mut Tab) {
    if tab.selected.is_empty() {
        return;
    }
    let visible_paths: HashSet<&Path> = tab
        .visible
        .iter()
        .map(|&index| tab.entries[index].path.as_path())
        .collect();
    tab.selected
        .retain(|path| visible_paths.contains(path.as_path()));
}

#[derive(Clone, Copy)]
enum TabCommand {
    New,
    Activate(usize),
    Close(usize),
    Duplicate(usize),
    ReopenClosed,
    Move { index: usize, direction: i8 },
    Cycle(i8),
}

#[derive(Clone, Copy)]
enum NavigationCommand {
    Back,
    Forward,
    Parent,
    Refresh,
    Hidden,
    Split,
    NewWindow,
}

#[derive(Clone, Copy)]
enum FileCommand {
    NewFolder,
    Copy,
    Cut,
    Paste,
    Rename,
    Trash,
    Delete,
    Properties,
}

struct DragFiles {
    source_tab: u64,
    paths: Vec<PathBuf>,
}

struct DropArea {
    tab_id: u64,
    destination: PathBuf,
    rect: Rect,
    folder: bool,
}

fn active_after_swap(active: usize, first: usize, second: usize) -> usize {
    if active == first {
        second
    } else if active == second {
        first
    } else {
        active
    }
}

fn resolve_split_tabs(tabs: &[Tab], pair: Option<(u64, u64)>) -> Option<(usize, usize)> {
    let (left, right) = pair?;
    let a = tabs
        .iter()
        .position(|tab| tab.id == left && tab.remote_uri.is_none())?;
    let b = tabs
        .iter()
        .position(|tab| tab.id == right && tab.remote_uri.is_none())?;
    (a != b).then_some((a, b))
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
    path: PathBuf,
    reveal: Option<PathBuf>,
    result: std::io::Result<filesystem::FolderScan>,
    dir_mtime: Option<SystemTime>,
}

struct RemoteScanResult {
    tab: u64,
    generation: u64,
    uri: String,
    result: Result<volume::RemoteSearch, String>,
}

struct RemotePreviewResult {
    tab: u64,
    generation: u64,
    uri: String,
    result: Result<String, String>,
}

struct RefreshResult {
    folders: Vec<(u64, PathBuf, Option<SystemTime>)>,
    volumes: Vec<filesystem::MountedVolume>,
    device_volumes: Vec<crate::udisks2::DeviceVolume>,
    places: Vec<(&'static str, PathBuf)>,
}

enum KeyringEvent {
    Loaded {
        uri: String,
        result: Result<Option<MountCredentials>, String>,
    },
    Forgotten {
        uri: String,
        result: Result<(), String>,
    },
}

struct PreviewResult {
    generation: u64,
    path: PathBuf,
    result: std::io::Result<preview::Preview>,
}

struct PropertySnapshot {
    lines: Vec<(String, String)>,
    metadata: std::fs::Metadata,
    mode: String,
    owner: String,
    group: String,
    is_symlink: bool,
    is_dir: bool,
}

struct PropertyResult {
    generation: u64,
    path: PathBuf,
    result: Result<PropertySnapshot, String>,
}

struct PropertyAnalysis {
    generation: u64,
    path: PathBuf,
    result: Result<String, String>,
}

fn read_property_snapshot(path: &Path) -> std::io::Result<PropertySnapshot> {
    // Both stat and property enumeration may touch an unavailable mount; they
    // must be called from a worker, never from egui's render callback.
    let metadata = std::fs::symlink_metadata(path)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        format!("{:03o}", metadata.permissions().mode() & 0o777)
    };
    #[cfg(not(unix))]
    let mode = String::new();
    #[cfg(unix)]
    let (owner, group) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.uid().to_string(), metadata.gid().to_string())
    };
    #[cfg(not(unix))]
    let (owner, group) = (String::new(), String::new());
    Ok(PropertySnapshot {
        lines: {
            let mut rows = filesystem::property_lines(path)?;
            rows.extend(metadata::xattr_rows(path));
            rows
        },
        mode,
        owner,
        group,
        is_symlink: metadata.file_type().is_symlink(),
        is_dir: metadata.is_dir(),
        metadata,
    })
}

#[derive(Clone, Copy)]
enum PasteMode {
    Immediate,
    Options,
}

struct PendingPaste {
    id: u64,
    revision: u64,
    tab_id: u64,
    tab_generation: u64,
    destination: PathBuf,
    mode: PasteMode,
    started: Instant,
}

impl PendingPaste {
    fn is_current(&self, revision: u64, tab: &Tab, now: Instant) -> bool {
        self.revision == revision
            && self.tab_id == tab.id
            && self.tab_generation == tab.generation
            && self.destination == tab.path
            && now.saturating_duration_since(self.started) < CLIPBOARD_WAIT
    }
}

enum ClipboardEvent {
    Written {
        revision: u64,
        success: bool,
    },
    Read {
        id: u64,
        files: Option<clipboard::Files>,
    },
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
    rename_undo: Option<Box<filesystem::RenameUndo>>,
    batch_rename_undo: Option<Box<filesystem::BatchRenameUndo>>,
    from_history: bool,
}

impl CompletedOperation {
    fn history_record(&self) -> transfer_history::Record {
        transfer_history::Record {
            id: self.id,
            label: self.label.clone(),
            summary: self.summary.clone(),
            outcome: match self.outcome {
                OperationOutcome::Succeeded => transfer_history::Outcome::Succeeded,
                OperationOutcome::Cancelled => transfer_history::Outcome::Cancelled,
                OperationOutcome::Failed => transfer_history::Outcome::Failed,
            },
            finished_unix_seconds: self
                .finished_at
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            elapsed_millis: self.elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
            counts: transfer_history::Counts {
                completed: self.counts.completed,
                retained: self.counts.retained,
                skipped: self.counts.skipped,
                failed: self.counts.failed,
                not_started: self.counts.not_started,
            },
            items: history_items(&self.items),
        }
        .bounded()
    }

    fn from_history(record: transfer_history::Record) -> Self {
        Self {
            id: record.id,
            label: record.label,
            summary: record.summary,
            outcome: match record.outcome {
                transfer_history::Outcome::Succeeded => OperationOutcome::Succeeded,
                transfer_history::Outcome::Cancelled => OperationOutcome::Cancelled,
                transfer_history::Outcome::Failed => OperationOutcome::Failed,
            },
            finished_at: std::time::UNIX_EPOCH + Duration::from_secs(record.finished_unix_seconds),
            elapsed: Duration::from_millis(record.elapsed_millis),
            counts: ItemCounts {
                completed: record.counts.completed,
                retained: record.counts.retained,
                skipped: record.counts.skipped,
                failed: record.counts.failed,
                not_started: record.counts.not_started,
            },
            items: record
                .items
                .into_iter()
                .map(|item| filesystem::ItemResult {
                    source: PathBuf::from(item.source),
                    target: item.target.map(PathBuf::from),
                    state: match item.state {
                        transfer_history::ItemOutcome::NotStarted => {
                            filesystem::ItemState::NotStarted
                        }
                        transfer_history::ItemOutcome::Completed => {
                            filesystem::ItemState::Completed
                        }
                        transfer_history::ItemOutcome::CopiedSourceRetained => {
                            filesystem::ItemState::CopiedSourceRetained
                        }
                        transfer_history::ItemOutcome::Skipped => filesystem::ItemState::Skipped,
                        transfer_history::ItemOutcome::Failed => filesystem::ItemState::Failed,
                        transfer_history::ItemOutcome::Cancelled => {
                            filesystem::ItemState::Cancelled
                        }
                    },
                    detail: item.detail,
                })
                .collect(),
            rename_undo: None,
            batch_rename_undo: None,
            from_history: true,
        }
    }
}

/// Retain bounded diagnostics, never capabilities to replay historical actions.
/// Prioritize failed/incomplete files when a large batch exceeds the detail cap.
fn history_items(items: &[filesystem::ItemResult]) -> Vec<transfer_history::ItemSummary> {
    let mut saved = Vec::with_capacity(items.len().min(transfer_history::MAX_ITEMS_PER_RECORD));
    for priority in [
        filesystem::ItemState::Failed,
        filesystem::ItemState::Cancelled,
        filesystem::ItemState::NotStarted,
        filesystem::ItemState::Skipped,
        filesystem::ItemState::CopiedSourceRetained,
        filesystem::ItemState::Completed,
    ] {
        for item in items {
            if item.state != priority {
                continue;
            }
            let state = match item.state {
                filesystem::ItemState::NotStarted => transfer_history::ItemOutcome::NotStarted,
                filesystem::ItemState::Completed => transfer_history::ItemOutcome::Completed,
                filesystem::ItemState::CopiedSourceRetained => {
                    transfer_history::ItemOutcome::CopiedSourceRetained
                }
                filesystem::ItemState::Skipped => transfer_history::ItemOutcome::Skipped,
                filesystem::ItemState::Failed => transfer_history::ItemOutcome::Failed,
                filesystem::ItemState::Cancelled => transfer_history::ItemOutcome::Cancelled,
            };
            saved.push(transfer_history::ItemSummary {
                source: item.source.to_string_lossy().into_owned(),
                target: item
                    .target
                    .as_ref()
                    .map(|target| target.to_string_lossy().into_owned()),
                state,
                detail: item.detail.clone(),
            });
            if saved.len() == transfer_history::MAX_ITEMS_PER_RECORD {
                return saved;
            }
        }
    }
    saved
}

#[derive(Clone, Copy, Default)]
struct ItemCounts {
    completed: usize,
    retained: usize,
    skipped: usize,
    failed: usize,
    not_started: usize,
}

/// Average-rate estimate based on transferred byte progress, not wall-clock
/// samples. It is deliberately hidden until enough time and data exist.
fn transfer_progress_details(progress: &Progress, elapsed: Duration) -> String {
    let measures_items = matches!(
        progress.phase,
        "Trashing" | "Deleting" | "Renaming" | "Linking"
    );
    let mut detail = if measures_items && progress.total > 0 {
        format!("{} / {} items", progress.transferred, progress.total)
    } else if progress.total > 0 {
        format!(
            "{} / {}",
            filesystem::human_size(progress.transferred),
            filesystem::human_size(progress.total)
        )
    } else {
        format!(
            "{} transferred",
            filesystem::human_size(progress.transferred)
        )
    };
    detail.push_str(&format!(" · {} elapsed", elapsed_label(elapsed)));
    if !measures_items && elapsed >= Duration::from_secs(1) && progress.transferred > 0 {
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

/// Shared confirmation content for both recoverable Trash and irreversible
/// Delete, independent of the active rice. Virtualize large selections.
fn removal_confirmation_paths(ui: &mut egui::Ui, paths: &[PathBuf]) {
    ui.label("Selected paths:");
    egui::ScrollArea::vertical()
        .max_height(156.0)
        .show_rows(ui, 23.0, paths.len(), |ui, range| {
            for path in &paths[range] {
                let full = path.display().to_string();
                ui.monospace(shorten(&full, 76)).on_hover_text(full);
            }
        });
}

impl ItemCounts {
    fn from_items(items: &[filesystem::ItemResult]) -> Self {
        let mut counts = Self::default();
        for item in items {
            match item.state {
                filesystem::ItemState::Completed => counts.completed += 1,
                filesystem::ItemState::CopiedSourceRetained => counts.retained += 1,
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

fn choose_paste_clipboard(
    desktop: Option<clipboard::Files>,
    internal: Option<Clipboard>,
    consumed_cut_paths: Option<&[PathBuf]>,
) -> Option<Clipboard> {
    let desktop = desktop.map(|files| Clipboard {
        paths: files.paths,
        cut: files.cut,
    });
    if desktop
        .as_ref()
        .is_some_and(|files| files.cut && consumed_cut_paths == Some(files.paths.as_slice()))
    {
        // The external cut clipboard may still contain a batch that already
        // moved partially. Never replay completed sources from that batch.
        return internal;
    }
    desktop.or(internal)
}

/// Reconcile cut paths after any result, including a partially failed batch.
/// A verified cross-device copy also consumes the cut intent, avoiding
/// accidental duplicate pastes while leaving its original file untouched.
/// `None` means no cut item was completed or safely copied.
fn remaining_cut_paths(items: &[filesystem::ItemResult]) -> Option<Vec<PathBuf>> {
    if !items.iter().any(|item| {
        matches!(
            item.state,
            filesystem::ItemState::Completed | filesystem::ItemState::CopiedSourceRetained
        )
    }) {
        return None;
    }
    Some(
        items
            .iter()
            .filter(|item| {
                !matches!(
                    item.state,
                    filesystem::ItemState::Completed | filesystem::ItemState::CopiedSourceRetained
                )
            })
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
        regex: bool,
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
        owner: String,
        group: String,
        attribute_name: String,
        attribute_value: String,
        generation: u64,
        details: Option<Result<Box<PropertySnapshot>, String>>,
        analysis: Option<Result<String, String>>,
    },
    Duplicates {
        generation: u64,
        result: Option<Result<duplicates::Report, String>>,
    },
    OpenWith {
        path: PathBuf,
        generation: u64,
        apps: Option<Result<Vec<String>, String>>,
    },
    ThemeEditor(Box<ThemeEditor>),
}

enum ArchiveMode {
    Browse {
        path: PathBuf,
        destination: PathBuf,
        entries: Option<Vec<archive::ArchiveEntry>>,
        selected: BTreeSet<usize>,
        folder_name: String,
        password: String,
    },
    Create {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        stem: String,
        format: archive::Format,
        encrypt: bool,
        password: String,
    },
}

struct ArchiveDialog {
    mode: ArchiveMode,
    running: bool,
    message: Option<(String, bool)>,
    transferred: u64,
    current_file: String,
}

enum ArchiveOperation {
    List {
        path: PathBuf,
        password: Option<String>,
    },
    Verify {
        path: PathBuf,
        password: Option<String>,
    },
    Extract {
        path: PathBuf,
        destination: PathBuf,
        folder: String,
        selected: Vec<PathBuf>,
        password: Option<String>,
    },
    Create {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        stem: String,
        format: archive::Format,
        password: Option<String>,
    },
}

enum ArchiveEvent {
    Listed {
        generation: u64,
        result: Result<Vec<archive::ArchiveEntry>, String>,
    },
    Progress {
        generation: u64,
        bytes: u64,
        path: String,
    },
    Finished {
        generation: u64,
        changed: bool,
        result: Result<String, String>,
    },
}

fn archive_stem(path: &Path) -> String {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let lower = filename.to_ascii_lowercase();
    for suffix in [
        ".tar.gz", ".tar.xz", ".tar.zst", ".tgz", ".txz", ".tzst", ".zip", ".tar", ".7z", ".rar",
    ] {
        if lower.ends_with(suffix) {
            let stem = &filename[..filename.len() - suffix.len()];
            return if stem.is_empty() {
                "Archive".into()
            } else {
                stem.into()
            };
        }
    }
    "Archive".into()
}

fn accept_property_result(dialog: &mut Option<Dialog>, message: PropertyResult) {
    if let Some(Dialog::Properties {
        path,
        mode,
        owner,
        group,
        generation,
        details,
        ..
    }) = dialog
        && *generation == message.generation
        && *path == message.path
    {
        if let Ok(snapshot) = &message.result {
            *mode = snapshot.mode.clone();
            *owner = snapshot.owner.clone();
            *group = snapshot.group.clone();
        }
        *details = Some(message.result.map(Box::new));
    }
}

struct ThemeReloadResult {
    generation: u64,
    path: PathBuf,
    fingerprint: Option<(SystemTime, u64)>,
    theme: Result<Option<Theme>, String>,
}

fn current_theme_reload(
    active_path: Option<&Path>,
    pending_generation: Option<u64>,
    result: &ThemeReloadResult,
) -> bool {
    pending_generation == Some(result.generation) && active_path == Some(result.path.as_path())
}

/// First frame must depend only on bundled theme data, never an external path.
fn starting_theme(configured: &str, external: bool) -> (Theme, Option<(String, bool)>) {
    match Theme::builtin(configured) {
        Some(theme) => (
            theme,
            external.then(|| {
                (
                    "Loading external TOML theme in the background…".into(),
                    false,
                )
            }),
        ),
        None => (
            Theme::builtin("win7").expect("bundled theme"),
            Some(if external {
                (
                    "Loading external TOML theme in the background…".into(),
                    false,
                )
            } else {
                (format!("Unknown theme: {configured}"), true)
            }),
        ),
    }
}

pub struct Explorer {
    desktop_service: Option<desktop::Service>,
    desktop_properties: VecDeque<PathBuf>,
    config: Config,
    config_writer: config::SaveWorker,
    theme: Theme,
    theme_file: Option<PathBuf>,
    theme_file_state: Option<(SystemTime, u64)>,
    theme_last_check: Instant,
    theme_reload_sender: Sender<ThemeReloadResult>,
    theme_reload_receiver: Receiver<ThemeReloadResult>,
    theme_reload_generation: u64,
    theme_reload_pending: Option<u64>,
    session_owner: bool,
    tabs: Vec<Tab>,
    closed_tabs: VecDeque<TabSnapshot>,
    split_pair: Option<(u64, u64)>,
    active: usize,
    next_tab: u64,
    folder_tree: FolderTree,
    tree_expanded: BTreeSet<PathBuf>,
    drag_files: Option<DragFiles>,
    drop_areas: Vec<DropArea>,
    scan_sender: Sender<ScanResult>,
    scan_receiver: Receiver<ScanResult>,
    remote_scan_sender: Sender<RemoteScanResult>,
    remote_scan_receiver: Receiver<RemoteScanResult>,
    remote_preview_sender: Sender<RemotePreviewResult>,
    remote_preview_receiver: Receiver<RemotePreviewResult>,
    remote_download_sender: Sender<Result<PathBuf, String>>,
    remote_download_receiver: Receiver<Result<PathBuf, String>>,
    remote_download_busy: bool,
    remote_download_active: Option<(String, PathBuf)>,
    remote_download_queue: remote_download::DownloadQueue,
    remote_download_queue_paused: bool,
    remote_download_cancel: Arc<AtomicBool>,
    remote_download_bytes: Arc<AtomicU64>,
    remote_download_directory: String,
    remote_upload_sender: Sender<Result<remote_upload::UploadResult, String>>,
    remote_upload_receiver: Receiver<Result<remote_upload::UploadResult, String>>,
    remote_upload_busy: bool,
    remote_upload_cancel: Arc<AtomicBool>,
    remote_upload_open: bool,
    remote_upload_target: String,
    remote_upload_source: String,
    remote_upload_username: String,
    remote_upload_domain: String,
    remote_upload_password: String,
    remote_upload_allow_http: bool,
    remote_staging_sender: Sender<(PathBuf, Result<remote_download::StagingInspection, String>)>,
    remote_staging_receiver:
        Receiver<(PathBuf, Result<remote_download::StagingInspection, String>)>,
    remote_staging_result: Option<(PathBuf, Result<remote_download::StagingInspection, String>)>,
    remote_staging_busy: bool,
    refresh_sender: Sender<RefreshResult>,
    refresh_receiver: Receiver<RefreshResult>,
    volume_sender: Sender<Result<(String, Option<String>), String>>,
    volume_receiver: Receiver<Result<(String, Option<String>), String>>,
    volume_connect_open: bool,
    volume_connect_uri: String,
    volume_connect_user: String,
    volume_connect_domain: String,
    volume_connect_password: String,
    volume_connect_remember: bool,
    volume_unlock_target: Option<(String, String)>,
    volume_unlock_password: String,
    keyring_loaded_uri: Option<String>,
    keyring_sender: Sender<KeyringEvent>,
    keyring_receiver: Receiver<KeyringEvent>,
    keyring_busy: bool,
    discovery_sender: Sender<Result<Vec<discovery::NetworkHost>, String>>,
    discovery_receiver: Receiver<Result<Vec<discovery::NetworkHost>, String>>,
    discovery_open: bool,
    discovery_busy: bool,
    discovery_hosts: Vec<discovery::NetworkHost>,
    discovery_error: Option<String>,
    discovery_cancel: Arc<AtomicBool>,
    volume_confirm: Option<VolumeAction>,
    volume_busy: bool,
    preview_sender: Sender<PreviewResult>,
    preview_receiver: Receiver<PreviewResult>,
    preview_generation: u64,
    preview_worker_running: bool,
    preview_pending: Option<(u64, PathBuf)>,
    property_sender: Sender<PropertyResult>,
    property_receiver: Receiver<PropertyResult>,
    analysis_sender: Sender<PropertyAnalysis>,
    analysis_receiver: Receiver<PropertyAnalysis>,
    duplicate_sender: Sender<(u64, Result<duplicates::Report, String>)>,
    duplicate_receiver: Receiver<(u64, Result<duplicates::Report, String>)>,
    duplicate_generation: u64,
    duplicate_cancel: Arc<AtomicBool>,
    open_with_sender: Sender<(u64, Result<Vec<String>, String>)>,
    open_with_receiver: Receiver<(u64, Result<Vec<String>, String>)>,
    open_with_generation: u64,
    open_with_cancel: Arc<AtomicBool>,
    analysis_worker_running: bool,
    analysis_cancel: Arc<AtomicBool>,
    property_generation: u64,
    property_worker_running: bool,
    property_pending: Option<(u64, PathBuf)>,
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
    history_sender: Option<SyncSender<transfer_history::Record>>,
    history_receiver: Option<Receiver<transfer_history::Event>>,
    history_done_receiver: Option<Receiver<()>>,
    pending_undo_origin: Option<u64>,
    recovery_sender: Sender<(u64, std::io::Result<Vec<JournalInspection>>)>,
    recovery_receiver: Receiver<(u64, std::io::Result<Vec<JournalInspection>>)>,
    recovery_generation: u64,
    recovery_records: Vec<JournalInspection>,
    recovery_error: Option<String>,
    recovery_loading: bool,
    recovery_rescan_requested: bool,
    recovery_open: bool,
    recovery_comparison_sender: Sender<(u64, String, bool, journal::Comparison)>,
    recovery_comparison_receiver: Receiver<(u64, String, bool, journal::Comparison)>,
    recovery_comparison_active: Option<String>,
    recovery_comparison_cancel: Arc<AtomicBool>,
    recovery_comparisons: HashMap<String, (bool, journal::Comparison)>,
    recovery_resolution_confirm: Option<String>,
    recovery_resolution_active: bool,
    remote_journal_sender: Sender<(u64, Result<Vec<remote_upload_journal::Record>, String>)>,
    remote_journal_receiver: Receiver<(u64, Result<Vec<remote_upload_journal::Record>, String>)>,
    remote_journal_generation: u64,
    remote_journal_records: Vec<remote_upload_journal::Record>,
    remote_journal_error: Option<String>,
    remote_journal_loading: bool,
    remote_journal_rescan_requested: bool,
    remote_journal_confirm: Option<String>,
    clipboard: Option<Clipboard>,
    consumed_cut_paths: Option<Vec<PathBuf>>,
    clipboard_sender: Sender<ClipboardEvent>,
    clipboard_receiver: Receiver<ClipboardEvent>,
    clipboard_revision: u64,
    desktop_copy_running: bool,
    pending_desktop_copy: Option<(Vec<PathBuf>, bool, u64)>,
    desktop_read_running: bool,
    desktop_read_revision: u64,
    next_paste_id: u64,
    pending_paste: Option<PendingPaste>,
    dialog: Option<Dialog>,
    archive_dialog: Option<ArchiveDialog>,
    archive_sender: Sender<ArchiveEvent>,
    archive_receiver: Receiver<ArchiveEvent>,
    archive_generation: u64,
    archive_cancel: Arc<AtomicBool>,
    status: Option<(String, bool)>,
    busy: bool,
    progress: Option<Progress>,
    cancel: Arc<AtomicBool>,
    volumes: Vec<filesystem::MountedVolume>,
    device_volumes: Vec<crate::udisks2::DeviceVolume>,
    last_refresh_check: Instant,
    places: Vec<(&'static str, PathBuf)>,
    refresh_in_flight: bool,
    preview_path: Option<PathBuf>,
    preview_loading: bool,
    preview_content: Option<PreviewContent>,
    preview_quick_look: bool,
    thumbnails: ThumbnailManager,
    dirty_config: bool,
    focus_address: bool,
    focus_search: bool,
    breadcrumb_edit_address: bool,
    aero_advanced_open: bool,
    aqua_advanced_open: bool,
    finder_advanced_open: bool,
    chromeos_advanced_open: bool,
    classic98_advanced_open: bool,
    win10_ribbon_page: RibbonPage,
    win10_ribbon_expanded: bool,
    win10_advanced_open: bool,
}

impl Explorer {
    pub fn new(
        context: &egui::Context,
        selected_theme: Option<String>,
        theme_file: Option<PathBuf>,
        locations: Vec<Location>,
        separate_window: bool,
        file_manager_service: bool,
    ) -> Self {
        let mut config = Config::load();
        let explicit_builtin = selected_theme.is_some();
        if let Some(name) = selected_theme {
            config.theme = name;
        }
        // CLI overrides saved themes; only editor-owned files inside the user
        // config directory are restored automatically on next startup.
        let theme_file = theme_file.or_else(|| {
            if explicit_builtin {
                return None;
            }
            let saved = config.custom_theme_file.as_ref()?;
            let root = config::config_file().parent()?.join("themes");
            (saved.parent() == Some(root.as_path())
                && saved.extension().is_some_and(|ext| ext == "toml"))
            .then(|| saved.clone())
        });
        // Never open a user-selected TOML path during GUI construction: a
        // disconnected mount or hung FUSE file must not block the first frame.
        // Start with an available builtin and load custom content on a worker.
        let (theme, status) = starting_theme(&config.theme, theme_file.is_some());
        theme.apply(context);
        let theme_file_state = None;

        let (scan_sender, scan_receiver) = mpsc::channel();
        let (theme_reload_sender, theme_reload_receiver) = mpsc::channel();
        let (remote_scan_sender, remote_scan_receiver) = mpsc::channel();
        let (remote_preview_sender, remote_preview_receiver) = mpsc::channel();
        let (remote_download_sender, remote_download_receiver) = mpsc::channel();
        let (remote_upload_sender, remote_upload_receiver) = mpsc::channel();
        let (remote_staging_sender, remote_staging_receiver) = mpsc::channel();
        let remote_download_directory = config
            .remote_download_directory
            .as_ref()
            .and_then(|folder| folder.to_str())
            .and_then(|folder| remote_download::download_directory(folder).ok())
            .unwrap_or_else(|| config::home().join("Downloads"))
            .display()
            .to_string();
        let (refresh_sender, refresh_receiver) = mpsc::channel();
        let (volume_sender, volume_receiver) = mpsc::channel();
        let (keyring_sender, keyring_receiver) = mpsc::channel();
        let (discovery_sender, discovery_receiver) = mpsc::channel();
        let (preview_sender, preview_receiver) = mpsc::channel();
        let (property_sender, property_receiver) = mpsc::channel();
        let (analysis_sender, analysis_receiver) = mpsc::channel();
        let (duplicate_sender, duplicate_receiver) = mpsc::channel();
        let (open_with_sender, open_with_receiver) = mpsc::channel();
        let (op_sender, op_receiver) = mpsc::channel();
        let (paste_preview_sender, paste_preview_receiver) = mpsc::channel();
        let (recovery_sender, recovery_receiver) = mpsc::channel();
        let (recovery_comparison_sender, recovery_comparison_receiver) = mpsc::channel();
        let (remote_journal_sender, remote_journal_receiver) = mpsc::channel();
        let (clipboard_sender, clipboard_receiver) = mpsc::channel();
        let (archive_sender, archive_receiver) = mpsc::channel();
        let (history_sender, history_receiver, history_done_receiver) = if separate_window {
            (None, None, None)
        } else {
            let (sender, receiver, done) = transfer_history::spawn_worker();
            (Some(sender), Some(receiver), Some(done))
        };
        // Explicit CLI paths take precedence over the saved session. Restored
        // local and network locations are scanned asynchronously, never stat'ed
        // or contacted while reading preferences.
        let from_session = locations.is_empty() && !separate_window;
        let (saved_histories, saved_closed) = if from_session {
            config.restored_navigation()
        } else {
            (Vec::new(), Vec::new())
        };
        let (startup_locations, startup_active) = if from_session {
            config.restored_session()
        } else {
            (locations, 0)
        };
        let mut explorer = Self {
            desktop_service: file_manager_service.then(|| desktop::Service::start(context)),
            desktop_properties: VecDeque::new(),
            config,
            config_writer: config::SaveWorker::spawn(),
            theme,
            theme_file,
            theme_file_state,
            theme_last_check: Instant::now(),
            theme_reload_sender,
            theme_reload_receiver,
            theme_reload_generation: 0,
            theme_reload_pending: None,
            session_owner: !separate_window,
            tabs: Vec::new(),
            closed_tabs: VecDeque::new(),
            split_pair: None,
            active: 0,
            next_tab: 1,
            folder_tree: FolderTree::new(),
            tree_expanded: BTreeSet::new(),
            drag_files: None,
            drop_areas: Vec::new(),
            scan_sender,
            scan_receiver,
            remote_scan_sender,
            remote_scan_receiver,
            remote_preview_sender,
            remote_preview_receiver,
            remote_download_sender,
            remote_download_receiver,
            remote_download_busy: false,
            remote_download_active: None,
            remote_download_queue: remote_download::DownloadQueue::default(),
            remote_download_queue_paused: false,
            remote_download_cancel: Arc::new(AtomicBool::new(false)),
            remote_download_bytes: Arc::new(AtomicU64::new(0)),
            remote_download_directory,
            remote_upload_sender,
            remote_upload_receiver,
            remote_upload_busy: false,
            remote_upload_cancel: Arc::new(AtomicBool::new(false)),
            remote_upload_open: false,
            remote_upload_target: String::new(),
            remote_upload_source: String::new(),
            remote_upload_username: String::new(),
            remote_upload_domain: String::new(),
            remote_upload_password: String::new(),
            remote_upload_allow_http: false,
            remote_staging_sender,
            remote_staging_receiver,
            remote_staging_result: None,
            remote_staging_busy: false,
            refresh_sender,
            refresh_receiver,
            volume_sender,
            volume_receiver,
            volume_connect_open: false,
            volume_connect_uri: String::new(),
            volume_connect_user: String::new(),
            volume_connect_domain: String::new(),
            volume_connect_password: String::new(),
            volume_connect_remember: false,
            volume_unlock_target: None,
            volume_unlock_password: String::new(),
            keyring_loaded_uri: None,
            keyring_sender,
            keyring_receiver,
            keyring_busy: false,
            discovery_sender,
            discovery_receiver,
            discovery_open: false,
            discovery_busy: false,
            discovery_hosts: Vec::new(),
            discovery_error: None,
            discovery_cancel: Arc::new(AtomicBool::new(false)),
            volume_confirm: None,
            volume_busy: false,
            preview_sender,
            preview_receiver,
            preview_generation: 0,
            preview_worker_running: false,
            preview_pending: None,
            property_sender,
            property_receiver,
            analysis_sender,
            analysis_receiver,
            duplicate_sender,
            duplicate_receiver,
            duplicate_generation: 0,
            duplicate_cancel: Arc::new(AtomicBool::new(false)),
            open_with_sender,
            open_with_receiver,
            open_with_generation: 0,
            open_with_cancel: Arc::new(AtomicBool::new(false)),
            analysis_worker_running: false,
            analysis_cancel: Arc::new(AtomicBool::new(false)),
            property_generation: 0,
            property_worker_running: false,
            property_pending: None,
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
            history_sender,
            history_receiver,
            history_done_receiver,
            pending_undo_origin: None,
            recovery_sender,
            recovery_receiver,
            recovery_generation: 0,
            recovery_records: Vec::new(),
            recovery_error: None,
            recovery_loading: false,
            recovery_rescan_requested: false,
            recovery_open: false,
            recovery_comparison_sender,
            recovery_comparison_receiver,
            recovery_comparison_active: None,
            recovery_comparison_cancel: Arc::new(AtomicBool::new(false)),
            recovery_comparisons: HashMap::new(),
            recovery_resolution_confirm: None,
            recovery_resolution_active: false,
            remote_journal_sender,
            remote_journal_receiver,
            remote_journal_generation: 0,
            remote_journal_records: Vec::new(),
            remote_journal_error: None,
            remote_journal_loading: false,
            remote_journal_rescan_requested: false,
            remote_journal_confirm: None,
            clipboard: None,
            consumed_cut_paths: None,
            clipboard_sender,
            clipboard_receiver,
            clipboard_revision: 0,
            desktop_copy_running: false,
            pending_desktop_copy: None,
            desktop_read_running: false,
            desktop_read_revision: 0,
            next_paste_id: 0,
            pending_paste: None,
            dialog: None,
            archive_dialog: None,
            archive_sender,
            archive_receiver,
            archive_generation: 0,
            archive_cancel: Arc::new(AtomicBool::new(false)),
            status,
            busy: false,
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            volumes: vec![filesystem::MountedVolume {
                path: PathBuf::from("/"),
                network: false,
            }],
            device_volumes: Vec::new(),
            last_refresh_check: Instant::now() - Duration::from_secs(3),
            places: vec![("Home", config::home()), ("Filesystem", PathBuf::from("/"))],
            refresh_in_flight: false,
            preview_path: None,
            preview_loading: false,
            preview_content: None,
            preview_quick_look: false,
            thumbnails: ThumbnailManager::default(),
            dirty_config: false,
            focus_address: false,
            focus_search: false,
            breadcrumb_edit_address: false,
            aero_advanced_open: false,
            aqua_advanced_open: false,
            finder_advanced_open: false,
            chromeos_advanced_open: false,
            classic98_advanced_open: false,
            win10_ribbon_page: RibbonPage::Home,
            win10_ribbon_expanded: true,
            win10_advanced_open: false,
        };
        if startup_locations.is_empty() {
            explorer.new_tab(config::home());
        } else {
            for location in startup_locations {
                match location {
                    Location::Local(path) => explorer.new_tab(path),
                    Location::Remote(remote) => {
                        explorer.new_tab(config::home());
                        explorer.navigate_remote(remote.uri);
                        // The local Home placeholder used to initialize the tab
                        // is not a real step in the restored history.
                        explorer.current_mut().back.clear();
                    }
                }
            }
            explorer.active = startup_active.min(explorer.tabs.len() - 1);
            explorer.remember_session();
        }
        if from_session {
            for (tab, (back, forward)) in explorer.tabs.iter_mut().zip(saved_histories) {
                tab.back = back;
                tab.forward = forward;
            }
            for (location, back, forward) in saved_closed {
                explorer.closed_tabs.push_back(TabSnapshot {
                    fallback_path: config::home(),
                    location,
                    query: String::new(),
                    back,
                    forward,
                    recursive_search: false,
                    content_search: false,
                });
            }
            explorer.remember_session();
        }
        explorer.check_unfinished_moves();
        explorer.check_unfinished_webdav_uploads();
        if explorer.theme_file.is_some() {
            // The worker owns all initial filesystem reads and requests a
            // repaint when the theme has been validated and applied.
            explorer.reload_theme_file(context, true);
        }
        explorer
    }

    fn poll_desktop(&mut self, context: &egui::Context) {
        if let Some(service) = self.desktop_service.as_mut()
            && let Ok(result) = service.ready.try_recv()
        {
            match result {
                Ok(connection) => service.connection = Some(connection),
                Err(error) => {
                    self.status = Some((format!("FileManager1 unavailable: {error}"), true))
                }
            }
        }
        // Desktop requests must not change the target behind a confirmation.
        if self.dialog.is_some()
            || self.archive_dialog.is_some()
            || self.volume_connect_open
            || self.volume_unlock_target.is_some()
            || self.volume_confirm.is_some()
            || self.discovery_open
            || self.recovery_open
        {
            return;
        }
        if let Some(path) = self.desktop_properties.pop_front() {
            self.open_properties(path);
            return;
        }
        let request = self
            .desktop_service
            .as_ref()
            .and_then(|service| service.requests.try_recv().ok());
        let Some(request) = request else { return };
        if self.tabs.len() > 96 {
            self.status = Some((
                "Close some tabs before opening more desktop requests".into(),
                true,
            ));
            return;
        }
        match request {
            desktop::Request::Folders(locations) => {
                for location in locations {
                    match location {
                        Location::Local(path) => self.new_tab(path),
                        Location::Remote(remote) => {
                            self.new_tab(config::home());
                            self.navigate_remote(remote.uri);
                        }
                    }
                }
            }
            desktop::Request::Items(paths) => {
                let mut folders = std::collections::BTreeMap::<PathBuf, BTreeSet<PathBuf>>::new();
                for path in paths {
                    let parent = path.parent().unwrap_or(Path::new("/")).to_path_buf();
                    folders.entry(parent).or_default().insert(path);
                }
                for (parent, paths) in folders {
                    self.new_tab(parent);
                    self.current_mut().pending_reveal = paths;
                }
            }
            desktop::Request::Properties(paths) => {
                self.desktop_properties.extend(paths);
                if let Some(path) = self.desktop_properties.pop_front() {
                    self.open_properties(path);
                }
            }
        }
        context.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Journal inspection never blocks the UI thread.
    fn check_unfinished_moves(&mut self) {
        if self.recovery_loading {
            // A disconnected mount can stall metadata probes. Never spawn an
            // unbounded number of overlapping recovery scan threads.
            self.recovery_rescan_requested = true;
            return;
        }
        self.recovery_generation = self.recovery_generation.wrapping_add(1);
        let generation = self.recovery_generation;
        self.recovery_loading = true;
        self.recovery_resolution_active = false;
        self.recovery_comparisons.clear();
        self.recovery_resolution_confirm = None;
        self.recovery_comparison_cancel
            .store(true, Ordering::Relaxed);
        let sender = self.recovery_sender.clone();
        thread::spawn(move || {
            let result = journal::inspect_incomplete();
            let _ = sender.send((generation, result));
        });
    }

    /// WebDAV evidence lives in local private state. Never query, change,
    /// delete or replay a remote server during startup/recovery inspection.
    fn check_unfinished_webdav_uploads(&mut self) {
        if self.remote_journal_loading {
            self.remote_journal_rescan_requested = true;
            return;
        }
        self.remote_journal_generation = self.remote_journal_generation.wrapping_add(1);
        let generation = self.remote_journal_generation;
        self.remote_journal_loading = true;
        let sender = self.remote_journal_sender.clone();
        thread::spawn(move || {
            let result =
                remote_upload_journal::inspect_incomplete().map_err(|error| error.to_string());
            let _ = sender.send((generation, result));
        });
    }

    fn acknowledge_webdav_upload(&mut self, record: remote_upload_journal::Record) {
        if self.remote_journal_loading {
            return;
        }
        self.remote_journal_generation = self.remote_journal_generation.wrapping_add(1);
        let generation = self.remote_journal_generation;
        self.remote_journal_loading = true;
        let sender = self.remote_journal_sender.clone();
        thread::spawn(move || {
            let result = remote_upload_journal::acknowledge(&record)
                .and_then(|()| remote_upload_journal::inspect_incomplete())
                .map_err(|error| error.to_string());
            let _ = sender.send((generation, result));
        });
    }

    fn compare_recovery_record(&mut self, record: &journal::Record, staged: bool) {
        if self.recovery_comparison_active.is_some() || self.recovery_loading {
            return;
        }
        let id = record.id.clone();
        let source = record.source.clone();
        let destination = match (staged, record.staging.as_ref()) {
            (true, Some(path)) => path.join("payload"),
            (true, None) => return,
            _ => record.destination.clone(),
        };
        let generation = self.recovery_generation;
        let sender = self.recovery_comparison_sender.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.recovery_comparison_cancel = Arc::clone(&cancelled);
        self.recovery_comparisons.remove(&id);
        self.recovery_comparison_active = Some(id.clone());
        thread::spawn(move || {
            #[cfg(target_os = "linux")]
            let result = journal::compare_staged_copy(&source, &destination, &cancelled);
            #[cfg(not(target_os = "linux"))]
            let result = journal::compare_for_recovery(&source, &destination, &cancelled);
            let _ = sender.send((generation, id, staged, result));
        });
    }

    #[cfg(unix)]
    fn resolve_recovery_record(&mut self, id: String) {
        self.run_recovery_action(id, journal::resolve_verified_copy);
    }

    #[cfg(target_os = "linux")]
    fn publish_recovery_stage(&mut self, id: String) {
        self.run_recovery_action(id, journal::publish_verified_stage);
    }

    #[cfg(unix)]
    fn run_recovery_action(
        &mut self,
        id: String,
        action: fn(&str, &AtomicBool) -> std::io::Result<()>,
    ) {
        if self.recovery_loading || self.recovery_comparison_active.is_some() {
            return;
        }
        self.recovery_generation = self.recovery_generation.wrapping_add(1);
        let generation = self.recovery_generation;
        self.recovery_loading = true;
        self.recovery_resolution_active = true;
        self.recovery_resolution_confirm = None;
        self.recovery_comparisons.clear();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.recovery_comparison_cancel = Arc::clone(&cancelled);
        let sender = self.recovery_sender.clone();
        thread::spawn(move || {
            let result = match action(&id, &cancelled) {
                Ok(()) => journal::inspect_incomplete(),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                    journal::inspect_incomplete()
                }
                Err(error) => Err(error),
            };
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
        if !self.session_owner {
            return;
        }
        let locations: Vec<_> = self.tabs.iter().map(Tab::location).collect();
        if self.config.remember_session(&locations, self.active) {
            self.dirty_config = true;
        }
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| (tab.location(), tab.back.clone(), tab.forward.clone()))
            .collect();
        let closed: Vec<_> = self
            .closed_tabs
            .iter()
            .map(|tab| (tab.location.clone(), tab.back.clone(), tab.forward.clone()))
            .collect();
        if self.config.remember_navigation(&tabs, &closed) {
            self.dirty_config = true;
        }
    }

    fn new_tab_at(&mut self, location: Location) {
        let previous = self.tabs.get(self.active).map(|tab| tab.id);
        let id = self.next_tab;
        self.next_tab += 1;
        let fallback = match &location {
            Location::Local(path) => path.clone(),
            Location::Remote(_) => self.current().path.clone(),
        };
        let mut tab = Tab::new(id, fallback);
        tab.set_location(location);
        self.tabs.push(tab);
        if let Some(previous) = previous {
            self.assign_split_tab(previous, id);
        }
        self.active = self.tabs.len() - 1;
        self.scan(self.active);
        self.remember_session();
    }

    fn new_tab(&mut self, path: PathBuf) {
        self.new_tab_at(Location::Local(path));
    }

    fn new_remote_tab(&mut self, uri: String) {
        match Location::parse(uri.into()) {
            Ok(remote @ Location::Remote(_)) => self.new_tab_at(remote),
            _ => self.status = Some(("Invalid network folder address".into(), true)),
        }
    }

    fn new_tab_like_current(&mut self) {
        self.new_tab_at(self.current().location());
    }

    fn assign_split_tab(&mut self, previous: u64, replacement: u64) {
        if let Some((left, right)) = self.split_pair {
            if previous == left {
                self.split_pair = Some((replacement, right));
            } else if previous == right {
                self.split_pair = Some((left, replacement));
            }
        }
    }

    /// Common tab dispatcher for keyboard, context menus and every rice shell.
    fn tab_command(&mut self, command: TabCommand) {
        match command {
            TabCommand::New => self.new_tab_like_current(),
            TabCommand::Activate(index) if index < self.tabs.len() => {
                if let Some((left, right)) = self.split_pair {
                    let target = self.tabs[index].id;
                    if target != left && target != right && self.tabs[index].remote_uri.is_none() {
                        self.split_pair = Some(if self.current().id == right {
                            (left, target)
                        } else {
                            (target, right)
                        });
                    }
                }
                self.active = index;
                self.remember_session();
            }
            TabCommand::Close(index) if index < self.tabs.len() => self.close_tab(index),
            TabCommand::Duplicate(index) if index < self.tabs.len() => {
                let snapshot = TabSnapshot::capture(&self.tabs[index]);
                let previous = self.tabs[index].id;
                let id = self.next_tab;
                self.next_tab += 1;
                self.tabs.insert(index + 1, snapshot.restore(id));
                self.assign_split_tab(previous, id);
                self.active = index + 1;
                self.scan(self.active);
                self.remember_session();
            }
            TabCommand::ReopenClosed => {
                if let Some(snapshot) = self.closed_tabs.pop_back() {
                    let previous = self.current().id;
                    let id = self.next_tab;
                    self.next_tab += 1;
                    let index = self.active + 1;
                    self.tabs.insert(index, snapshot.restore(id));
                    self.assign_split_tab(previous, id);
                    self.active = index;
                    self.scan(index);
                    self.remember_session();
                }
            }
            TabCommand::Move { index, direction } if index < self.tabs.len() => {
                let target = index.checked_add_signed(isize::from(direction));
                if let Some(target) = target.filter(|&target| target < self.tabs.len()) {
                    self.tabs.swap(index, target);
                    self.active = active_after_swap(self.active, index, target);
                    self.remember_session();
                }
            }
            TabCommand::Cycle(direction) if self.tabs.len() > 1 => {
                let count = self.tabs.len();
                let index = if direction < 0 {
                    (self.active + count - 1) % count
                } else {
                    (self.active + 1) % count
                };
                self.tab_command(TabCommand::Activate(index));
            }
            _ => {}
        }
    }

    /// Unified commands keep file behavior independent of the active rice skin.
    fn navigation_command(&mut self, command: NavigationCommand) {
        match command {
            NavigationCommand::Back => self.history(true),
            NavigationCommand::Forward => self.history(false),
            NavigationCommand::Parent => self.parent(),
            NavigationCommand::Refresh => self.scan(self.active),
            NavigationCommand::Hidden => self.toggle_hidden(),
            NavigationCommand::Split => self.toggle_split_view(),
            NavigationCommand::NewWindow => self.spawn_window(),
        }
    }

    fn file_command(&mut self, command: FileCommand) {
        match command {
            FileCommand::NewFolder => {
                self.dialog = Some(Dialog::Name {
                    mode: NameMode::Folder(self.current().path.clone()),
                    value: String::new(),
                });
            }
            FileCommand::Copy => self.copy_selection(false),
            FileCommand::Cut => self.copy_selection(true),
            FileCommand::Paste => self.paste(),
            FileCommand::Rename => self.rename_selected(),
            FileCommand::Trash => self.ask_trash(),
            FileCommand::Delete => self.ask_delete(),
            FileCommand::Properties => {
                if let Some(path) = self.selected_paths().first().cloned() {
                    self.open_properties(path);
                }
            }
        }
    }

    fn toggle_split_view(&mut self) {
        if self.split_pair.is_some() {
            self.split_pair = None;
            return;
        }
        if self.current().remote_uri.is_some() {
            self.status = Some((
                "Split view currently supports local folders only".into(),
                true,
            ));
            return;
        }
        let left = self.current().id;
        let other = self
            .tabs
            .iter()
            .find(|tab| tab.id != left && tab.remote_uri.is_none())
            .map(|tab| tab.id);
        let right = match other {
            Some(id) => id,
            None => {
                self.new_tab_like_current();
                self.current().id
            }
        };
        self.split_pair = Some((left, right));
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == left) {
            self.active = index;
        }
        self.remember_session();
    }

    fn split_indices(&self) -> Option<(usize, usize)> {
        resolve_split_tabs(&self.tabs, self.split_pair)
    }

    fn finish_drag(&mut self, context: &egui::Context) {
        if !context.input(|input| input.pointer.any_released()) {
            return;
        }
        let Some(source) = self.drag_files.take() else {
            return;
        };
        let Some(position) = context.input(|input| input.pointer.latest_pos()) else {
            return;
        };
        let target = self.drop_destination(position);
        let Some((tab_id, destination, folder)) = target else {
            return;
        };
        if source.source_tab == tab_id && !folder {
            return;
        }
        if source
            .paths
            .iter()
            .any(|path| destination.starts_with(path))
        {
            self.status = Some((
                "Cannot copy a folder into itself or a descendant".into(),
                true,
            ));
            return;
        }
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) {
            self.tab_command(TabCommand::Activate(index));
        }
        self.run_action(Action::Copy {
            sources: source.paths,
            destination,
            conflict: ConflictPolicy::KeepBoth,
        });
    }

    fn drop_destination(&self, position: Pos2) -> Option<(u64, PathBuf, bool)> {
        self.drop_areas
            .iter()
            .rev()
            .filter(|area| area.rect.contains(position))
            .max_by_key(|area| area.folder)
            .map(|area| (area.tab_id, area.destination.clone(), area.folder))
    }

    fn accept_external_drop(&mut self, context: &egui::Context) {
        let files: Vec<_> = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if files.is_empty() {
            return;
        }
        if self.current().remote_uri.is_some() {
            self.status = Some((
                "Remote browsing is read-only; drag-in is disabled".into(),
                true,
            ));
            return;
        }
        let target = context
            .input(|input| input.pointer.latest_pos())
            .and_then(|position| self.drop_destination(position));
        let destination = target
            .as_ref()
            .map_or_else(|| self.current().path.clone(), |(_, path, _)| path.clone());
        if files.iter().any(|path| destination.starts_with(path)) {
            self.status = Some((
                "Cannot copy a folder into itself or a descendant".into(),
                true,
            ));
            return;
        }
        if let Some((id, _, _)) = target
            && let Some(index) = self.tabs.iter().position(|tab| tab.id == id)
        {
            self.tab_command(TabCommand::Activate(index));
        }
        self.run_action(Action::Copy {
            sources: files,
            destination,
            conflict: ConflictPolicy::KeepBoth,
        });
    }

    fn spawn_window(&mut self) {
        if self.current().remote_uri.is_some() {
            self.status = Some((
                "New windows currently support local folders only".into(),
                true,
            ));
            return;
        }
        let program = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                self.status = Some((format!("Cannot locate executable: {error}"), true));
                return;
            }
        };
        let mut command = Command::new(program);
        command.arg("--separate-window");
        if let Some(path) = self.theme_file.as_ref() {
            command.arg("--theme-file").arg(path);
        } else {
            command.arg("--theme").arg(&self.theme.id);
        }
        command.arg("--").arg(&self.current().path);
        match command.spawn() {
            Ok(_) => self.status = Some(("Opened a separate Explorer window".into(), false)),
            Err(error) => self.status = Some((format!("Cannot open window: {error}"), true)),
        }
    }

    fn close_tab(&mut self, index: usize) {
        if self.tabs.len() == 1 {
            self.navigate(config::home());
            return;
        }
        if self.closed_tabs.len() == MAX_CLOSED_TABS {
            self.closed_tabs.pop_front();
        }
        self.closed_tabs
            .push_back(TabSnapshot::capture(&self.tabs[index]));
        self.tabs[index].cancel_scan();
        self.tabs[index].clear_remote_preview();
        if self.split_pair.is_some_and(|(left, right)| {
            left == self.tabs[index].id || right == self.tabs[index].id
        }) {
            self.split_pair = None;
        }
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
        self.remember_session();
    }

    fn scan(&mut self, index: usize) {
        let show_hidden = self.config.show_hidden;
        let tab = &mut self.tabs[index];
        let cancel = tab.next_scan_token();
        tab.generation += 1;
        tab.loading = true;
        tab.error = None;
        tab.scan_warning = None;
        if let Some(uri) = tab.remote_uri.clone() {
            tab.clear_remote_preview();
            let sender = self.remote_scan_sender.clone();
            let id = tab.id;
            let generation = tab.generation;
            let query = tab.query.trim().to_owned();
            let recursive = tab.recursive_search;
            thread::spawn(move || {
                let result = if query.is_empty() {
                    volume::list_remote(&uri, show_hidden, &cancel).map(|entries| {
                        volume::RemoteSearch {
                            entries,
                            notice: None,
                        }
                    })
                } else {
                    volume::search_remote(&uri, &query, recursive, show_hidden, &cancel)
                };
                if !cancel.load(Ordering::Relaxed) {
                    let _ = sender.send(RemoteScanResult {
                        tab: id,
                        generation,
                        uri,
                        result,
                    });
                }
            });
            return;
        }
        let task = (tab.id, tab.generation, tab.path.clone());
        let searching = (tab.recursive_search || tab.content_search) && !tab.query.is_empty();
        let recursive = tab.recursive_search;
        let contents = tab.content_search;
        let query = tab.query.clone();
        let sender = self.scan_sender.clone();
        thread::spawn(move || {
            let (path, reveal) = match filesystem::navigation_target(&task.2) {
                Ok(target) => target,
                Err(error) => {
                    let _ = sender.send(ScanResult {
                        tab: task.0,
                        generation: task.1,
                        path: task.2,
                        reveal: None,
                        result: Err(error),
                        dir_mtime: None,
                    });
                    return;
                }
            };
            let result = if searching {
                filesystem::search_with_options(
                    &path,
                    &query,
                    show_hidden,
                    recursive,
                    contents,
                    &cancel,
                )
            } else {
                filesystem::scan_cancellable(&path, &cancel)
            };
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let dir_mtime = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok();
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = sender.send(ScanResult {
                tab: task.0,
                generation: task.1,
                path,
                reveal,
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

    fn start_remote_preview(&mut self, uri: String) {
        if self.current().remote_uri.is_none()
            || !self
                .current()
                .remote_entries
                .iter()
                .any(|entry| entry.is_regular_file && entry.uri == uri)
        {
            self.status = Some((
                "Select an entry from the current network folder".into(),
                true,
            ));
            return;
        }
        let tab = self.current_mut();
        let cancel = tab.next_remote_preview_token(uri.clone());
        let id = tab.id;
        let generation = tab.remote_preview_generation;
        let sender = self.remote_preview_sender.clone();
        thread::spawn(move || {
            let result = volume::preview_remote_text(&uri, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                let _ = sender.send(RemotePreviewResult {
                    tab: id,
                    generation,
                    uri,
                    result,
                });
            }
        });
    }

    fn start_remote_download(&mut self, uri: String, name: String, is_directory: bool) {
        if self.current().remote_uri.is_none()
            || !self.current().remote_entries.iter().any(|entry| {
                entry.is_directory == is_directory
                    && (is_directory || entry.is_regular_file)
                    && entry.uri == uri
                    && entry.name == name
            })
        {
            self.status = Some((
                "Select a regular file or folder from the network listing".into(),
                true,
            ));
            return;
        }
        // Only an explicitly selected absolute directory is accepted;
        // atomic no-overwrite publication protects existing names there.
        let destination = match remote_download::download_directory(&self.remote_download_directory)
        {
            Ok(directory) => directory,
            Err(error) => {
                self.status = Some((error.into(), true));
                return;
            }
        };
        if self.config.remote_download_directory.as_ref() != Some(&destination) {
            self.config.remote_download_directory = Some(destination.clone());
            self.dirty_config = true;
        }
        let job = remote_download::DownloadRequest {
            uri,
            encoded_name: name,
            directory: destination,
            is_directory,
        };
        if self.remote_download_active.as_ref() == Some(&(job.uri.clone(), job.directory.clone())) {
            self.status = Some(("This network entry is already downloading".into(), true));
            return;
        }
        if self.remote_download_busy || self.remote_download_queue_paused {
            self.status = Some(match self.remote_download_queue.enqueue(job) {
                Ok(waiting) => (
                    format!("Network download queued ({waiting} waiting)"),
                    false,
                ),
                Err(error) => (error.into(), true),
            });
        } else {
            self.launch_remote_download(job);
        }
    }

    fn launch_remote_download(&mut self, job: remote_download::DownloadRequest) {
        self.remote_download_active = Some((job.uri.clone(), job.directory.clone()));
        let cancelled = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(AtomicU64::new(0));
        self.remote_download_cancel = Arc::clone(&cancelled);
        self.remote_download_bytes = Arc::clone(&bytes);
        self.remote_download_busy = true;
        let sender = self.remote_download_sender.clone();
        thread::spawn(move || {
            let result = if job.is_directory {
                remote_download::download_folder(
                    &job.uri,
                    &job.encoded_name,
                    &job.directory,
                    &cancelled,
                    &bytes,
                )
            } else {
                remote_download::download(
                    &job.uri,
                    &job.encoded_name,
                    &job.directory,
                    &cancelled,
                    &bytes,
                )
            };
            let _ = sender.send(result);
        });
    }

    fn inspect_remote_staging(&mut self) {
        if self.remote_staging_busy {
            return;
        }
        let directory = match remote_download::download_directory(&self.remote_download_directory) {
            Ok(directory) => directory,
            Err(error) => {
                self.status = Some((error.into(), true));
                return;
            }
        };
        self.remote_staging_busy = true;
        self.remote_staging_result = None;
        let sender = self.remote_staging_sender.clone();
        thread::spawn(move || {
            let result =
                remote_download::inspect_staging(&directory).map_err(|error| error.to_string());
            let _ = sender.send((directory, result));
        });
    }

    fn start_next_remote_download(&mut self) {
        if !self.remote_download_busy
            && !self.remote_download_queue_paused
            && let Some(job) = self.remote_download_queue.dequeue()
        {
            self.launch_remote_download(job);
        }
    }

    fn open_remote_upload(&mut self) {
        let Some(target) = self.current().remote_uri.clone() else {
            return;
        };
        if !matches!(Location::parse(target.clone().into()),
            Ok(Location::Remote(location)) if matches!(location.scheme,
                crate::location::RemoteScheme::Dav | crate::location::RemoteScheme::Davs))
        {
            self.status = Some(("This backend cannot prove an atomic no-replace publish; network writes stay disabled".into(), true));
            return;
        }
        self.remote_upload_target = target;
        self.remote_upload_source = self
            .tabs
            .iter()
            .find(|tab| tab.remote_uri.is_none() && tab.selected.len() == 1)
            .and_then(|tab| tab.selected.iter().next())
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.remote_upload_username.clear();
        self.remote_upload_domain.clear();
        volume::clear_password(&mut self.remote_upload_password);
        self.remote_upload_allow_http = false;
        self.remote_upload_open = true;
    }

    fn remote_upload_dialog(&mut self, context: &egui::Context) {
        if !self.remote_upload_open {
            return;
        }
        let mut open = true;
        let mut upload = false;
        egui::Window::new("Upload to WebDAV · safe publication")
            .open(&mut open)
            .collapsible(false)
            .default_width(490.0)
            .show(context, |ui| {
                ui.label("Destination directory:");
                ui.monospace(&self.remote_upload_target);
                ui.label("Local regular file (absolute path):");
                ui.add(egui::TextEdit::singleline(&mut self.remote_upload_source)
                    .desired_width(f32::INFINITY).hint_text("/home/user/Documents/file.txt"));
                ui.label("Username and password (or leave blank to try a saved login):");
                ui.add(egui::TextEdit::singleline(&mut self.remote_upload_username)
                    .desired_width(f32::INFINITY).hint_text("WebDAV username"));
                ui.add(egui::TextEdit::singleline(&mut self.remote_upload_domain)
                    .desired_width(f32::INFINITY).hint_text("Domain (optional)"));
                ui.add(egui::TextEdit::singleline(&mut self.remote_upload_password)
                    .password(true).desired_width(f32::INFINITY).hint_text("Password"));
                if self.remote_upload_target.starts_with("dav://") {
                    ui.checkbox(&mut self.remote_upload_allow_http,
                        "Allow unencrypted HTTP for this upload")
                        .on_hover_text("HTTP may expose credentials and file contents on the network. Prefer davs:// with a trusted certificate.");
                }
                ui.separator();
                ui.small("Before any file is published, RiceField tests the server's overwrite protection using disposable internal files, uploads a private stage, and compares every byte. The server must accept Overwrite:F; existing destinations are never intentionally replaced. Unsuccessful remote stages are left for manual inspection.");
                upload = ui.add_enabled(!self.remote_upload_busy,
                    egui::Button::new("Verify server and upload")).clicked();
            });
        if upload {
            let source = PathBuf::from(self.remote_upload_source.trim());
            if !source.is_absolute() || self.remote_upload_source.trim().is_empty() {
                self.status = Some((
                    "Select a regular file using an absolute local path".into(),
                    true,
                ));
                return;
            }
            let request = remote_upload::UploadRequest {
                source,
                remote_directory: self.remote_upload_target.clone(),
                allow_insecure_http: self.remote_upload_allow_http,
                credentials: MountCredentials {
                    username: std::mem::take(&mut self.remote_upload_username),
                    domain: std::mem::take(&mut self.remote_upload_domain),
                    password: std::mem::take(&mut self.remote_upload_password),
                },
            };
            let cancelled = Arc::new(AtomicBool::new(false));
            self.remote_upload_cancel = Arc::clone(&cancelled);
            self.remote_upload_busy = true;
            let sender = self.remote_upload_sender.clone();
            thread::spawn(move || {
                let result = remote_upload::upload(request, &cancelled);
                let _ = sender.send(result);
            });
            self.remote_upload_open = false;
            self.remote_upload_source.clear();
        } else if !open {
            volume::clear_password(&mut self.remote_upload_password);
            self.remote_upload_open = false;
        }
    }

    fn toggle_hidden(&mut self) {
        self.config.show_hidden = !self.config.show_hidden;
        self.dirty_config = true;
        for index in 0..self.tabs.len() {
            if self.tabs[index].remote_uri.is_some()
                || self.tabs[index].recursive_search
                || self.tabs[index].content_search
            {
                self.scan(index);
            } else {
                self.rebuild_visible(index);
            }
        }
    }

    fn start_discovery(&mut self) {
        if self.discovery_busy {
            return;
        }
        self.discovery_open = true;
        self.discovery_busy = true;
        self.discovery_error = None;
        self.discovery_hosts.clear();
        self.discovery_cancel = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&self.discovery_cancel);
        let sender = self.discovery_sender.clone();
        thread::spawn(move || {
            let result = discovery::discover(&cancelled);
            let _ = sender.send(result);
        });
    }

    fn request_keyring(&mut self, forget: bool) {
        if self.keyring_busy || self.volume_connect_uri.is_empty() {
            return;
        }
        self.keyring_busy = true;
        let uri = self.volume_connect_uri.clone();
        let sender = self.keyring_sender.clone();
        thread::spawn(move || {
            let event = if forget {
                KeyringEvent::Forgotten {
                    result: keyring::forget(&uri),
                    uri,
                }
            } else {
                KeyringEvent::Loaded {
                    result: keyring::lookup(&uri),
                    uri,
                }
            };
            let _ = sender.send(event);
        });
    }

    fn open_network_login(&mut self, uri: String) {
        self.volume_connect_uri = uri;
        self.volume_connect_user.clear();
        self.volume_connect_domain.clear();
        volume::clear_password(&mut self.volume_connect_password);
        self.volume_connect_remember = false;
        self.keyring_loaded_uri = None;
        self.volume_connect_open = true;
    }

    fn start_volume_action(&mut self, action: VolumeAction, remember_login: bool) {
        if self.volume_busy {
            return;
        }
        self.volume_busy = true;
        let sender = self.volume_sender.clone();
        thread::spawn(move || {
            let label = action.label();
            let connected_uri = match &action {
                VolumeAction::Connect(uri) | VolumeAction::ConnectAuthenticated(uri, _) => {
                    Some(uri.clone())
                }
                _ => None,
            };
            let result = volume::execute(&action).map(|()| {
                let mut message = format!("{label} succeeded. Refreshing mounted volumes…");
                if remember_login
                    && let VolumeAction::ConnectAuthenticated(uri, credentials) = &action
                {
                    match keyring::save(uri, credentials) {
                        Ok(()) => message.push_str(" Login saved in the Linux keyring."),
                        Err(error) => message.push_str(&format!(" Login was not saved: {error}")),
                    }
                }
                (message, connected_uri)
            });
            let _ = sender.send(result);
        });
    }

    fn network_browser(&mut self, context: &egui::Context) {
        if !self.discovery_open {
            return;
        }
        let mut open = true;
        let mut retry = false;
        let mut connect = None;
        let mut browse = None;
        let mut browse_new_tab = None;
        egui::Window::new("Discover network computers")
            .open(&mut open)
            .resizable(true)
            .default_width(440.0)
            .show(context, |ui| {
                ui.label("Nearby advertised SMB/NAS servers (no automatic connections):");
                ui.horizontal(|ui| {
                    retry = ui
                        .add_enabled(!self.discovery_busy, egui::Button::new("Scan again"))
                        .clicked();
                    if self.discovery_busy {
                        ui.spinner();
                        ui.label("Looking for network shares…");
                    }
                });
                if let Some(error) = &self.discovery_error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                } else if !self.discovery_busy && self.discovery_hosts.is_empty() {
                    ui.label("No NAS was advertised. You can still connect using its address.");
                }
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .show(ui, |ui| {
                        for host in &self.discovery_hosts {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(&host.label);
                                    ui.small(format!("{} · {}", host.uri, host.source));
                                });
                                if ui
                                    .button("Browse shares")
                                    .on_hover_text(
                                        "List shares using GVfs; sign in when a share requires it",
                                    )
                                    .clicked()
                                {
                                    browse = Some(host.uri.clone());
                                }
                                if ui
                                    .button("New tab")
                                    .on_hover_text(
                                        "Browse this server without replacing the current folder",
                                    )
                                    .clicked()
                                {
                                    browse_new_tab = Some(host.uri.clone());
                                }
                                if ui.button("Sign in").clicked() {
                                    connect = Some(host.uri.clone());
                                }
                            });
                            ui.separator();
                        }
                    });
            });
        if retry {
            self.start_discovery();
        }
        if let Some(uri) = browse_new_tab {
            self.discovery_cancel.store(true, Ordering::Relaxed);
            self.discovery_open = false;
            self.new_remote_tab(uri);
        } else if let Some(uri) = browse {
            self.discovery_cancel.store(true, Ordering::Relaxed);
            self.discovery_open = false;
            self.navigate_remote(uri);
        } else if let Some(uri) = connect {
            self.discovery_cancel.store(true, Ordering::Relaxed);
            self.discovery_open = false;
            self.open_network_login(uri);
        } else if !open {
            self.discovery_cancel.store(true, Ordering::Relaxed);
            self.discovery_open = false;
        }
    }

    fn volume_dialog(&mut self, context: &egui::Context) {
        if self.volume_connect_open {
            let mut open = true;
            let mut connect = false;
            egui::Window::new("Connect to network location")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(420.0)
                .show(context, |ui| {
                    ui.label("SMB, SFTP, WebDAV or MTP address (no passwords in the URL):");
                    ui.add_sized(
                        [ui.available_width().min(420.0), 26.0],
                        egui::TextEdit::singleline(&mut self.volume_connect_uri)
                            .hint_text("smb://server/share"),
                    );
                    // A saved password belongs to one server; editing its
                    // address must not send the secret to a different host.
                    if self
                        .keyring_loaded_uri
                        .as_ref()
                        .is_some_and(|loaded| loaded != &self.volume_connect_uri)
                    {
                        volume::clear_password(&mut self.volume_connect_password);
                        self.volume_connect_user.clear();
                        self.volume_connect_domain.clear();
                        self.volume_connect_remember = false;
                        self.keyring_loaded_uri = None;
                    }
                    ui.separator();
                    ui.label("Username (leave empty for server default):");
                    ui.add_sized(
                        [ui.available_width().min(420.0), 26.0],
                        egui::TextEdit::singleline(&mut self.volume_connect_user)
                            .hint_text("NAS username"),
                    );
                    ui.label("Domain / workgroup (optional):");
                    ui.add_sized(
                        [ui.available_width().min(420.0), 26.0],
                        egui::TextEdit::singleline(&mut self.volume_connect_domain)
                            .hint_text("WORKGROUP"),
                    );
                    ui.label("Password (leave empty for guest / existing session):");
                    ui.add_sized(
                        [ui.available_width().min(420.0), 26.0],
                        egui::TextEdit::singleline(&mut self.volume_connect_password)
                            .password(true)
                            .hint_text("NAS password"),
                    );
                    ui.checkbox(&mut self.volume_connect_remember, "Remember login in Linux keyring")
                        .on_hover_text("Only after a successful connection. Requires libsecret-tools and an unlocked Secret Service; never stored in RiceField settings.");
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!self.keyring_busy, egui::Button::new("Use saved login")).clicked() {
                            self.request_keyring(false);
                        }
                        if ui.add_enabled(!self.keyring_busy, egui::Button::new("Forget saved login")).clicked() {
                            self.request_keyring(true);
                        }
                        if self.keyring_busy { ui.spinner(); }
                    });
                    ui.small("Passwords are never included in the URL. Saving is opt-in and uses your desktop's encrypted Secret Service.");
                    connect = ui.add_enabled(!self.volume_busy, egui::Button::new("Connect")).clicked();
                });
            self.volume_connect_open = open;
            if connect {
                match Location::parse(self.volume_connect_uri.clone().into()) {
                    Ok(Location::Remote(remote)) => {
                        let credentials = MountCredentials {
                            username: self.volume_connect_user.clone(),
                            domain: self.volume_connect_domain.clone(),
                            password: std::mem::take(&mut self.volume_connect_password),
                        };
                        let action = if credentials.username.is_empty()
                            && credentials.domain.is_empty()
                            && credentials.password.is_empty()
                        {
                            VolumeAction::Connect(remote.uri)
                        } else {
                            VolumeAction::ConnectAuthenticated(remote.uri, credentials)
                        };
                        if self.volume_connect_remember
                            && !matches!(&action, VolumeAction::ConnectAuthenticated(_, _))
                        {
                            self.status =
                                Some(("Enter a password before saving a login".into(), true));
                            return;
                        }
                        let remember_login = self.volume_connect_remember;
                        self.start_volume_action(action, remember_login);
                        self.volume_connect_open = false;
                        self.volume_connect_uri.clear();
                        self.volume_connect_remember = false;
                        self.keyring_loaded_uri = None;
                    }
                    Ok(Location::Local(_)) => {
                        self.status = Some(("Enter a supported network URI".into(), true))
                    }
                    Err(error) => self.status = Some((error.into(), true)),
                }
            } else if !open {
                volume::clear_password(&mut self.volume_connect_password);
                self.volume_connect_remember = false;
                self.keyring_loaded_uri = None;
                self.volume_connect_open = false;
            }
        }
        if let Some((object_path, label)) = self.volume_unlock_target.clone() {
            let mut open = true;
            let mut approve = false;
            egui::Window::new("Unlock encrypted removable volume")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(context, |ui| {
                    ui.strong(label);
                    ui.label("Enter the volume passphrase. UDisks2 and Polkit handle authorization; RiceField never saves this password.");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.volume_unlock_password)
                            .password(true)
                            .hint_text("Volume passphrase"),
                    );
                    approve = ui
                        .add_enabled(
                            !self.volume_busy && !self.volume_unlock_password.is_empty(),
                            egui::Button::new("Unlock volume"),
                        )
                        .clicked();
                });
            if approve {
                let credentials = MountCredentials {
                    username: String::new(),
                    domain: String::new(),
                    password: std::mem::take(&mut self.volume_unlock_password),
                };
                self.volume_unlock_target = None;
                self.start_volume_action(
                    VolumeAction::UnlockDevice(object_path, credentials),
                    false,
                );
            } else if !open {
                volume::clear_password(&mut self.volume_unlock_password);
                self.volume_unlock_target = None;
            }
        }
        if let Some(action) = self.volume_confirm.clone() {
            let mut approve = false;
            let mut dismiss = false;
            egui::Window::new(format!("Confirm {}", action.label()))
                .collapsible(false)
                .resizable(false)
                .show(context, |ui| {
                    if matches!(action, VolumeAction::LockEncrypted(_)) {
                        ui.label("Unmount all decrypted filesystems before locking this volume.");
                        ui.label("UDisks2 will refuse to lock a busy encrypted mapping.");
                    } else {
                        ui.label(
                            "Open files or transfers on this volume may fail after disconnecting it.",
                        );
                        ui.label("Make sure writes are complete before continuing.");
                    }
                    ui.horizontal(|ui| {
                        approve = ui
                            .add_enabled(!self.volume_busy, egui::Button::new(action.label()))
                            .clicked();
                        dismiss = ui.button("Cancel").clicked();
                    });
                });
            if approve {
                self.start_volume_action(action, false);
            }
            if approve || dismiss {
                self.volume_confirm = None;
            }
        }
    }

    fn navigate(&mut self, path: PathBuf) {
        self.thumbnails.clear_pending();
        if path == self.current().path && self.current().remote_uri.is_none() {
            self.scan(self.active);
            return;
        }
        self.preview_path = None;
        self.preview_content = None;
        self.preview_quick_look = false;
        self.current_mut().visit(Location::Local(path));
        self.scan(self.active);
        self.remember_session();
    }

    fn navigate_address(&mut self, address: String) {
        match Location::parse(address.into()) {
            Ok(Location::Local(path)) => self.navigate(path),
            Ok(Location::Remote(remote)) => {
                self.navigate_remote(remote.uri);
            }
            Err(error) => {
                // Preserve the actual local tab; never scan an unsupported URI
                // as a relative filename or leak URI credentials into status.
                let existing = self
                    .current()
                    .remote_uri
                    .clone()
                    .unwrap_or_else(|| self.current().path.display().to_string());
                self.current_mut().address = existing;
                self.status = Some((error.into(), true));
            }
        }
    }

    fn navigate_remote(&mut self, uri: String) {
        self.thumbnails.clear_pending();
        if self.current().remote_uri.as_deref() == Some(&uri) {
            self.scan(self.active);
            return;
        }
        let Ok(remote @ Location::Remote(_)) = Location::parse(uri.into()) else {
            self.status = Some(("Invalid network address".into(), true));
            return;
        };
        self.preview_path = None;
        self.preview_content = None;
        self.preview_quick_look = false;
        self.current_mut().visit(remote);
        self.scan(self.active);
        self.remember_session();
    }

    fn history(&mut self, backwards: bool) {
        if self.current_mut().travel(backwards) {
            self.preview_path = None;
            self.preview_content = None;
            self.preview_quick_look = false;
            self.scan(self.active);
            self.remember_session();
        }
    }

    fn parent(&mut self) {
        if let Some(uri) = self.current().remote_uri.as_deref() {
            if let Some((parent, _)) = uri.trim_end_matches('/').rsplit_once('/')
                && parent.contains("://")
            {
                self.navigate_remote(parent.to_owned());
            }
            return;
        }
        if let Some(parent) = self.current().path.parent().map(Path::to_path_buf) {
            self.navigate(parent);
        }
    }

    fn rebuild_visible(&mut self, index: usize) {
        let config = &self.config;
        let tab = &mut self.tabs[index];
        let search = tab.query.to_lowercase();
        let name_glob = file_view::CompiledGlob::new(&config.filter_name_glob);
        let name_regex = file_view::filename_regex(&config.filter_name_regex);
        let now = SystemTime::now();
        tab.visible = (0..tab.entries.len())
            .filter(|&index| {
                let entry = &tab.entries[index];
                (config.show_hidden || !entry.name.starts_with('.'))
                    && ((tab.content_search && !tab.query.is_empty())
                        || tab.folded_names[index].contains(&search))
                    && file_view::matches_filter(entry, &config.filter_kind)
                    && file_view::matches_size(entry, &config.filter_size)
                    && file_view::matches_modified(entry, &config.filter_modified, now)
                    && name_glob.matches(&entry.name)
                    && name_regex.as_ref().is_ok_and(|regex| {
                        regex
                            .as_ref()
                            .is_none_or(|regex| regex.is_match(&entry.name))
                    })
                    && file_view::matches_custom_ranges(
                        entry,
                        config.filter_min_size_mib,
                        config.filter_max_size_mib,
                        config.filter_newer_days,
                        config.filter_older_days,
                        now,
                    )
            })
            .collect();
        retain_visible_selection(tab);
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
                "lexical" => tab.folded_names[*left].cmp(&tab.folded_names[*right]),
                _ => natural_cmp(&tab.folded_names[*left], &tab.folded_names[*right]),
            };
            let ordered = if config.descending {
                comparison.reverse()
            } else {
                comparison
            };
            ordered.then_with(|| natural_cmp(&tab.folded_names[*left], &tab.folded_names[*right]))
        });
        tab.last_selected = None;
    }

    fn poll_background(&mut self, context: &egui::Context) {
        while let Some(error) = self.config_writer.next_error() {
            self.status = Some((format!("Cannot save preferences: {error}"), true));
        }
        while let Ok(message) = self.theme_reload_receiver.try_recv() {
            if !current_theme_reload(
                self.theme_file.as_deref(),
                self.theme_reload_pending,
                &message,
            ) {
                continue;
            }
            self.theme_reload_pending = None;
            self.theme_file_state = message.fingerprint;
            match message.theme {
                Ok(Some(theme)) => {
                    self.theme = theme;
                    self.theme.apply(context);
                    self.status = Some(("External TOML theme reloaded".into(), false));
                }
                Ok(None) => {}
                Err(error) => {
                    self.status = Some((
                        format!("Theme update rejected; previous theme preserved: {error}"),
                        true,
                    ));
                }
            }
        }
        if let Some(receiver) = &self.history_receiver {
            let mut events = Vec::new();
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
            for event in events {
                match event {
                    transfer_history::Event::Loaded(Ok(records)) => {
                        for record in records {
                            if self.recent_operations.len() >= MAX_RECENT_OPERATIONS {
                                break;
                            }
                            self.recent_operations
                                .push_back(CompletedOperation::from_history(record));
                        }
                    }
                    transfer_history::Event::Loaded(Err(error)) => {
                        self.status = Some((
                            format!(
                                "Previous transfer history unavailable (preserved unchanged): {error}"
                            ),
                            true,
                        ));
                    }
                    transfer_history::Event::SaveError(error) => {
                        self.status = Some((
                            format!("Transfer history could not be saved: {error}"),
                            true,
                        ));
                    }
                }
            }
        }
        while let Ok((generation, result)) = self.open_with_receiver.try_recv() {
            if let Some(Dialog::OpenWith {
                generation: active,
                apps,
                ..
            }) = self.dialog.as_mut()
                && *active == generation
            {
                *apps = Some(result);
            }
        }
        while let Ok((generation, result)) = self.duplicate_receiver.try_recv() {
            if let Some(Dialog::Duplicates {
                generation: active,
                result: slot,
            }) = self.dialog.as_mut()
                && *active == generation
            {
                *slot = Some(result);
            }
        }
        self.thumbnails.poll(context);
        if self.folder_tree.poll() {
            context.request_repaint();
        }
        while let Ok(event) = self.archive_receiver.try_recv() {
            match event {
                ArchiveEvent::Listed { generation, result }
                    if generation == self.archive_generation =>
                {
                    if let Some(dialog) = self.archive_dialog.as_mut() {
                        dialog.running = false;
                        match result {
                            Ok(entries) => {
                                if let ArchiveMode::Browse {
                                    selected,
                                    entries: existing,
                                    ..
                                } = &mut dialog.mode
                                {
                                    *selected = (0..entries.len()).collect();
                                    *existing = Some(entries);
                                }
                            }
                            Err(error) => {
                                dialog.message = Some((error, true));
                            }
                        }
                    }
                }
                ArchiveEvent::Progress {
                    generation,
                    bytes,
                    path,
                } if generation == self.archive_generation => {
                    if let Some(dialog) = self.archive_dialog.as_mut() {
                        dialog.transferred = bytes;
                        dialog.current_file = path;
                    }
                }
                ArchiveEvent::Finished {
                    generation,
                    changed,
                    result,
                } if generation == self.archive_generation => {
                    let success = result.is_ok();
                    if let Some(dialog) = self.archive_dialog.as_mut() {
                        dialog.running = false;
                        dialog.message = Some(match result {
                            Ok(message) => {
                                self.status = Some((message.clone(), false));
                                (message, false)
                            }
                            Err(error) => (error, true),
                        });
                    }
                    if changed && success {
                        self.scan(self.active);
                    }
                }
                _ => {} // Obsolete/closed archive dialog: never apply stale worker results.
            }
        }
        while let Ok(event) = self.clipboard_receiver.try_recv() {
            match event {
                ClipboardEvent::Written { revision, success } => {
                    self.desktop_copy_running = false;
                    if let Some((paths, cut, latest)) = self.pending_desktop_copy.take() {
                        self.start_desktop_copy(paths, cut, latest);
                    } else if revision == self.clipboard_revision
                        && self.status.as_ref().is_some_and(|(message, _)| {
                            message.contains("syncing desktop clipboard")
                        })
                        && let Some(files) = &self.clipboard
                    {
                        self.status = Some((
                            format!(
                                "{} {} item(s){}",
                                if files.cut { "Cut" } else { "Copied" },
                                files.paths.len(),
                                if success {
                                    ""
                                } else {
                                    " (internal clipboard only)"
                                }
                            ),
                            false,
                        ));
                    }
                }
                ClipboardEvent::Read { id, files } => {
                    self.desktop_read_running = false;
                    if let Some(pending) = self.pending_paste.take_if(|pending| pending.id == id) {
                        if pending.started.elapsed() >= CLIPBOARD_WAIT {
                            self.status = Some((
                                "Desktop clipboard is not responding; no files were pasted".into(),
                                true,
                            ));
                        } else if pending.is_current(
                            self.clipboard_revision,
                            self.current(),
                            Instant::now(),
                        ) {
                            let clipboard = self.clipboard_for_paste(files);
                            self.finish_paste(clipboard, pending.destination, pending.mode);
                        } else {
                            self.status = Some((
                                "Paste cancelled because the active folder or clipboard changed"
                                    .into(),
                                false,
                            ));
                        }
                    }
                }
            }
        }
        if self
            .pending_paste
            .as_ref()
            .is_some_and(|pending| pending.started.elapsed() >= CLIPBOARD_WAIT)
        {
            // Do not silently act on an old internal cut buffer if the
            // desktop clipboard is stuck. Ignore any late worker response.
            self.pending_paste = None;
            self.status = Some((
                "Desktop clipboard is not responding; no files were pasted".into(),
                true,
            ));
        }
        while let Ok((generation, id, staged, result)) =
            self.recovery_comparison_receiver.try_recv()
        {
            if self.recovery_comparison_active.as_deref() == Some(id.as_str()) {
                self.recovery_comparison_active = None;
            }
            if generation == self.recovery_generation
                && self
                    .recovery_records
                    .iter()
                    .any(|entry| entry.record.id == id)
            {
                self.recovery_comparisons.insert(id, (staged, result));
            }
        }
        while let Ok((generation, result)) = self.recovery_receiver.try_recv() {
            if generation == self.recovery_generation {
                self.recovery_loading = false;
                self.recovery_resolution_active = false;
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
                if std::mem::take(&mut self.recovery_rescan_requested) {
                    self.check_unfinished_moves();
                }
            }
        }
        while let Ok((generation, result)) = self.remote_journal_receiver.try_recv() {
            if generation != self.remote_journal_generation {
                continue;
            }
            self.remote_journal_loading = false;
            match result {
                Ok(records) => {
                    self.remote_journal_records = records;
                    self.remote_journal_error = None;
                    self.remote_journal_confirm = None;
                }
                Err(error) => {
                    // Retain previously inspected evidence on later failures.
                    self.remote_journal_error = Some(error);
                }
            }
            if std::mem::take(&mut self.remote_journal_rescan_requested) {
                self.check_unfinished_webdav_uploads();
            }
        }
        while let Ok(message) = self.preview_receiver.try_recv() {
            self.preview_worker_running = false;
            if message.generation == self.preview_generation
                && self.preview_path.as_ref() == Some(&message.path)
            {
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
            if let Some((generation, path)) = self.preview_pending.take()
                && self.preview_path.as_ref() == Some(&path)
                && generation == self.preview_generation
            {
                self.start_preview_worker(generation, path);
            }
        }
        while let Ok(message) = self.property_receiver.try_recv() {
            self.property_worker_running = false;
            accept_property_result(&mut self.dialog, message);
            // A blocked or slow mount cannot spawn an unbounded collection of
            // property readers. Only the latest requested path is kept.
            if let Some((generation, path)) = self.property_pending.take()
                && matches!(
                    &self.dialog,
                    Some(Dialog::Properties {
                        path: active_path,
                        generation: active_generation,
                        ..
                    }) if *active_generation == generation && *active_path == path
                )
            {
                self.start_property_worker(generation, path);
            }
        }
        while let Ok(message) = self.analysis_receiver.try_recv() {
            self.analysis_worker_running = false;
            if let Some(Dialog::Properties {
                path,
                generation,
                analysis,
                ..
            }) = self.dialog.as_mut()
                && *path == message.path
                && *generation == message.generation
            {
                *analysis = Some(message.result);
            }
        }
        while let Ok(result) = self.refresh_receiver.try_recv() {
            self.places = result.places;
            self.refresh_in_flight = false;
            self.volumes = result.volumes;
            self.device_volumes = result.device_volumes;
            for (tab_id, path, modified) in result.folders {
                if let Some(index) = self.tabs.iter().position(|tab| {
                    tab.id == tab_id && tab.path == path && tab.remote_uri.is_none()
                }) && !self.tabs[index].loading
                    && self.tabs[index].dir_mtime != modified
                {
                    self.scan(index);
                }
            }
        }
        while let Ok(result) = self.volume_receiver.try_recv() {
            self.volume_busy = false;
            self.status = Some(match result {
                Ok((message, connected_uri)) => {
                    if let Some(uri) = connected_uri {
                        if self.config.remote_favorites.len() < 32
                            && !self.config.remote_favorites.contains(&uri)
                        {
                            self.config.remote_favorites.push(uri.clone());
                            self.dirty_config = true;
                        }
                        self.navigate_remote(uri);
                    }
                    (message, false)
                }
                Err(error) => (error, true),
            });
            self.last_refresh_check = Instant::now() - Duration::from_secs(3);
        }
        while let Ok(event) = self.keyring_receiver.try_recv() {
            self.keyring_busy = false;
            match event {
                KeyringEvent::Loaded { uri, result }
                    if self.volume_connect_open && self.volume_connect_uri == uri =>
                {
                    match result {
                        Ok(Some(mut credentials)) => {
                            self.volume_connect_user = std::mem::take(&mut credentials.username);
                            self.volume_connect_domain = std::mem::take(&mut credentials.domain);
                            volume::clear_password(&mut self.volume_connect_password);
                            self.volume_connect_password =
                                std::mem::take(&mut credentials.password);
                            self.volume_connect_remember = true;
                            self.keyring_loaded_uri = Some(uri);
                            self.status =
                                Some(("Saved NAS login loaded from Linux keyring".into(), false));
                        }
                        Ok(None) => {
                            self.status =
                                Some(("No login saved for this network location".into(), false))
                        }
                        Err(error) => self.status = Some((error, true)),
                    }
                }
                KeyringEvent::Forgotten { uri, result }
                    if self.volume_connect_open && self.volume_connect_uri == uri =>
                {
                    self.status = Some(match result {
                        Ok(()) => {
                            self.volume_connect_remember = false;
                            self.keyring_loaded_uri = None;
                            volume::clear_password(&mut self.volume_connect_password);
                            (
                                "Saved network login removed from Linux keyring".into(),
                                false,
                            )
                        }
                        Err(error) => (error, true),
                    });
                }
                _ => {}
            }
        }
        while let Ok(result) = self.discovery_receiver.try_recv() {
            self.discovery_busy = false;
            match result {
                Ok(hosts) => {
                    self.discovery_hosts = hosts;
                    self.discovery_error = None;
                }
                Err(error) if error == "Discovery cancelled" => {}
                Err(error) => self.discovery_error = Some(error),
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
                    Ok(scan) => {
                        if tab.path != message.path {
                            tab.path = message.path;
                            tab.address = tab.path.display().to_string();
                        }
                        tab.scan_warning = (scan.unreadable > 0).then(|| {
                            format!(
                                "{} entry/entries could not be inspected; other items remain visible. First error: {}",
                                scan.unreadable,
                                scan.first_error.as_deref().unwrap_or("unknown")
                            )
                        });
                        if let Some(notice) = scan.notice {
                            tab.scan_warning = Some(match tab.scan_warning.take() {
                                Some(warning) => format!("{warning} · {notice}"),
                                None => notice,
                            });
                        }
                        tab.folded_names = scan
                            .entries
                            .iter()
                            .map(|entry| entry.name.to_lowercase())
                            .collect();
                        tab.entries = scan.entries;
                        tab.error = None;
                        tab.selected
                            .retain(|path| tab.entries.iter().any(|entry| &entry.path == path));
                        self.rebuild_visible(index);
                        let reveal = std::mem::take(&mut self.tabs[index].pending_reveal);
                        let requested = reveal.len() + usize::from(message.reveal.is_some());
                        self.tabs[index].selected.extend(reveal);
                        if let Some(path) = message.reveal {
                            self.tabs[index].selected.insert(path);
                        }
                        retain_visible_selection(&mut self.tabs[index]);
                        if requested > 0 && self.tabs[index].selected.len() < requested {
                            self.status = Some((
                                "Some requested items are missing or hidden by the current filters"
                                    .into(),
                                true,
                            ));
                        }
                        self.remember_session();
                    }
                    Err(error) => {
                        tab.entries.clear();
                        tab.folded_names.clear();
                        tab.visible.clear();
                        tab.error = Some(filesystem::folder_error_message(&tab.path, &error));
                        tab.scan_warning = None;
                    }
                }
            }
        }
        while let Ok(message) = self.remote_scan_receiver.try_recv() {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| {
                tab.accepts_scan_result(message.tab, message.generation)
                    && tab.remote_uri.as_deref() == Some(message.uri.as_str())
            }) {
                tab.loading = false;
                match message.result {
                    Ok(report) => {
                        tab.remote_entries = report.entries;
                        sort_remote_entries(
                            &mut tab.remote_entries,
                            tab.remote_sort,
                            tab.remote_sort_descending,
                        );
                        if tab.remote_selected_uri.as_ref().is_some_and(|uri| {
                            !tab.remote_entries.iter().any(|entry| &entry.uri == uri)
                        }) {
                            tab.remote_selected_uri = None;
                        }
                        tab.scan_warning = report.notice;
                        tab.error = None;
                    }
                    Err(error) => {
                        tab.remote_entries.clear();
                        tab.remote_selected_uri = None;
                        tab.scan_warning = None;
                        tab.error = Some(error);
                    }
                }
            }
        }
        while let Ok(message) = self.remote_preview_receiver.try_recv() {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| {
                tab.accepts_remote_preview(message.tab, message.generation, &message.uri)
            }) {
                tab.remote_preview_result = Some(message.result);
            }
        }
        while let Ok(result) = self.remote_download_receiver.try_recv() {
            self.remote_download_busy = false;
            self.remote_download_active = None;
            if result.is_err() && !self.remote_download_queue.is_empty() {
                self.remote_download_queue_paused = true;
            }
            if self.remote_download_queue.is_empty() {
                self.remote_download_queue_paused = false;
            }
            self.status = Some(match result {
                Ok(path) => (format!("Downloaded to {}", path.display()), false),
                Err(error) => (error, true),
            });
            self.start_next_remote_download();
        }
        while let Ok(result) = self.remote_upload_receiver.try_recv() {
            self.remote_upload_busy = false;
            self.check_unfinished_webdav_uploads();
            self.status = Some(match result {
                Ok(report) => {
                    if self.current().remote_uri.as_deref()
                        == report.uri.rsplit_once('/').map(|p| p.0)
                    {
                        self.scan(self.active);
                    }
                    (
                        format!(
                            "Published {} ({} bytes) without replacing an existing file",
                            report.uri, report.bytes
                        ),
                        false,
                    )
                }
                Err(error) => (
                    format!(
                        "WebDAV upload not published: {error}. Inspect private server staging if a transfer was interrupted."
                    ),
                    true,
                ),
            });
        }
        while let Ok(result) = self.remote_staging_receiver.try_recv() {
            self.remote_staging_busy = false;
            self.remote_staging_result = Some(result);
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
                    let undo_succeeded = report.result.is_ok();
                    let rename_undo = report.rename_undo;
                    let batch_rename_undo = report.batch_rename_undo;
                    if let Some(origin_id) = self.pending_undo_origin.take()
                        && undo_succeeded
                        && let Some(entry) = self
                            .recent_operations
                            .iter_mut()
                            .find(|entry| entry.id == origin_id)
                    {
                        entry.rename_undo = None;
                        entry.batch_rename_undo = None;
                    }
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
                    let completed = CompletedOperation {
                        id,
                        label,
                        summary,
                        outcome,
                        finished_at: SystemTime::now(),
                        elapsed,
                        counts: ItemCounts::from_items(&report.items),
                        items: report.items,
                        rename_undo,
                        batch_rename_undo,
                        from_history: false,
                    };
                    if let Some(writer) = &self.history_sender {
                        match writer.try_send(completed.history_record()) {
                            Ok(()) => {}
                            Err(mpsc::TrySendError::Full(_)) => {
                                self.status = Some((
                                    "Transfer completed, but the history writer is backlogged"
                                        .into(),
                                    true,
                                ));
                            }
                            Err(mpsc::TrySendError::Disconnected(_)) => {
                                self.status = Some((
                                    "Transfer completed, but history persistence stopped".into(),
                                    true,
                                ));
                            }
                        }
                    }
                    self.recent_operations.push_front(completed);
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
        if self.recovery_records.is_empty()
            && self.recovery_error.is_none()
            && self.remote_journal_records.is_empty()
            && self.remote_journal_error.is_none()
        {
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
                        if self.recovery_error.is_some() || self.remote_journal_error.is_some() {
                            "File transfer recovery records need attention".to_owned()
                        } else if !self.remote_journal_records.is_empty() {
                            format!(
                                "{} unfinished local move(s), {} uncertain WebDAV upload(s) — review before remote cleanup",
                                self.recovery_records.len(),
                                self.remote_journal_records.len(),
                            )
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
        let mut compare = None;
        let mut browse_parent = None;
        let mut prepare_copy = None;
        let mut acknowledge_webdav = None;
        let mut confirm_webdav = self.remote_journal_confirm.clone();
        #[cfg(unix)]
        let mut resolve = None;
        #[cfg(target_os = "linux")]
        let mut publish_staged = None;
        #[cfg(unix)]
        let mut confirm_resolution = self.recovery_resolution_confirm.clone();
        egui::Window::new("File transfer recovery")
            .open(&mut open)
            .default_width(610.0)
            .default_height(340.0)
            .resizable(true)
            .show(context, |ui| {
                ui.label("Recovery never deletes original files or overwrites destinations. An explicit confirmed action can publish a verified local staged copy to a free destination or retire a verified publication journal. No action runs automatically.");
                ui.label("Compare file contents on demand (maximum 512 MiB and 10,000 entries). A match is only a snapshot, never authorization to delete a source.");
                ui.small("If the original source identity still matches, you can separately prepare a new Copy with a full preview. This does not complete the interrupted Move or clear its journal.");
                if let Some(error) = &self.recovery_error {
                    ui.colored_label(Color32::LIGHT_RED, format!("Cannot read all journal records: {error}"));
                }
                if self.recovery_loading {
                    ui.spinner();
                    ui.label("Checking operation records…");
                    if self.recovery_resolution_active
                        && ui.button("Cancel verification").clicked()
                    {
                        self.recovery_comparison_cancel.store(true, Ordering::Relaxed);
                    }
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(330.0).show(ui, |ui| {
                    for inspection in &self.recovery_records {
                        let record = &inspection.record;
                        ui.group(|ui| {
                            ui.strong(format!("{} — {:?}", record.id, record.state));
                            ui.label(format!("Source: {}", record.source.display()));
                            ui.small(format!("Current source: {}", inspection.source.label()));
                            ui.small(format!(
                                "Source identity: {}",
                                inspection.source_identity.label()
                            ));
                            ui.label(format!("Destination: {}", record.destination.display()));
                            ui.small(format!(
                                "Current destination: {}",
                                inspection.destination.label()
                            ));
                            ui.small(format!(
                                "Destination identity: {}",
                                inspection.destination_identity.label()
                            ));
                            ui.small(format!(
                                "Staging → publication: {}",
                                inspection.publication_evidence.label()
                            ));
                            if let Some(staging) = &record.staging {
                                ui.label(format!("Staging: {}", staging.display()));
                            }
                            if let Some(staging) = &inspection.staging {
                                ui.small(format!("Current staging: {}", staging.label()));
                            }
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Browse source folder").clicked() {
                                    browse_parent = record.source.parent().map(Path::to_path_buf);
                                }
                                if ui.button("Browse destination folder").clicked() {
                                    browse_parent = record.destination.parent().map(Path::to_path_buf);
                                }
                                if let Some(staging) = &record.staging
                                    && matches!(
                                        inspection.staging.as_ref(),
                                        Some(journal::PathStatus::Directory)
                                    )
                                    && ui.button("Browse staging folder").clicked()
                                {
                                    browse_parent = Some(staging.clone());
                                }
                            });
                            if ui.add_enabled(
                                inspection.can_offer_fresh_copy()
                                    && record.destination.parent().is_some(),
                                egui::Button::new("Prepare separate Copy (keep both)…"),
                            ).on_hover_text("Start a new copy preview only; keep the original source and recovery record. Unavailable if the saved source identity changed or cannot be checked.").clicked()
                                && let Some(parent) = record.destination.parent()
                            {
                                prepare_copy = Some((record.source.clone(), parent.to_path_buf()));
                            }
                            let guidance = match record.state {
                                JournalState::Prepared => "Copy may be incomplete or may not have started.",
                                JournalState::StagedVerified => "Staged data was verified; final publication may have occurred.",
                                JournalState::Published => "Destination was published; the original is intentionally retained.",
                                JournalState::Completed => "Verified copy completed; original retained.",
                            };
                            ui.small(guidance);
                            ui.colored_label(Color32::LIGHT_YELLOW, inspection.guidance());
                            if self.recovery_comparison_active.as_deref() == Some(record.id.as_str()) {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Comparing bytes in background…");
                                    if ui.button("Cancel comparison").clicked() {
                                        self.recovery_comparison_cancel
                                            .store(true, Ordering::Relaxed);
                                    }
                                });
                            } else {
                                let enabled = !self.recovery_loading
                                    && self.recovery_comparison_active.is_none();
                                ui.horizontal_wrapped(|ui| {
                                    if ui
                                        .add_enabled(
                                            enabled,
                                            egui::Button::new("Compare source and destination"),
                                        )
                                        .clicked()
                                    {
                                        compare = Some((record.clone(), false));
                                    }
                                    if record.state == JournalState::StagedVerified
                                        && matches!(
                                            inspection.staging.as_ref(),
                                            Some(journal::PathStatus::Directory)
                                        )
                                        && ui
                                            .add_enabled(
                                                enabled,
                                                egui::Button::new("Compare source and staged copy"),
                                            )
                                            .clicked()
                                    {
                                        compare = Some((record.clone(), true));
                                    }
                                });
                            }
                            if let Some((staged, result)) = self.recovery_comparisons.get(&record.id) {
                                ui.small(if *staged {
                                    "Read-only comparison against staged payload:"
                                } else {
                                    "Read-only comparison against destination:"
                                });
                                let color = match result {
                                    journal::Comparison::Identical { .. } => Color32::LIGHT_GREEN,
                                    journal::Comparison::Different(_) => Color32::LIGHT_RED,
                                    journal::Comparison::Inconclusive(_) => Color32::LIGHT_YELLOW,
                                };
                                ui.colored_label(color, result.label());
                                #[cfg(target_os = "linux")]
                                if *staged
                                    && matches!(result, journal::Comparison::Identical { .. })
                                    && inspection.can_offer_stage_publication()
                                    && !self.recovery_loading
                                    && self.recovery_comparison_active.is_none()
                                {
                                    let confirmation = format!("publish:{}", record.id);
                                    if confirm_resolution.as_deref() == Some(confirmation.as_str()) {
                                        ui.colored_label(Color32::LIGHT_YELLOW, "Publish this verified staged copy to the currently vacant destination? The original remains untouched. The operation will recheck source, stage, nested metadata and target and refuse a collision; an interrupted publication may require another recovery review.");
                                        ui.horizontal(|ui| {
                                            if ui.button("Confirm safe publication").clicked() {
                                                publish_staged = Some(record.id.clone());
                                                confirm_resolution = None;
                                            }
                                            if ui.button("Cancel").clicked() {
                                                confirm_resolution = None;
                                            }
                                        });
                                    } else if ui.button("Publish verified staged copy…").clicked() {
                                        confirm_resolution = Some(confirmation);
                                    }
                                }
                                #[cfg(unix)]
                                if !*staged
                                    && matches!(result, journal::Comparison::Identical { .. })
                                    && matches!(record.state, JournalState::Published | JournalState::StagedVerified)
                                    && matches!(inspection.source, journal::PathStatus::File | journal::PathStatus::Symlink | journal::PathStatus::Directory)
                                    && !self.recovery_loading
                                    && self.recovery_comparison_active.is_none()
                                {
                                    if confirm_resolution.as_deref() == Some(record.id.as_str()) {
                                        ui.colored_label(Color32::LIGHT_YELLOW, "Confirm retiring this journal record? Both files are kept. A fresh identity and byte comparison will run again; failures leave the record intact.");
                                        ui.horizontal(|ui| {
                                            if ui.button("Confirm journal resolution").clicked() {
                                                resolve = Some(record.id.clone());
                                                confirm_resolution = None;
                                            }
                                            if ui.button("Cancel").clicked() {
                                                confirm_resolution = None;
                                            }
                                        });
                                    } else if ui.button("Resolve verified copy…").clicked() {
                                        confirm_resolution = Some(record.id.clone());
                                    }
                                }
                            }
                        });
                    }
                });
                if !self.remote_journal_records.is_empty() || self.remote_journal_error.is_some() {
                    ui.separator();
                    ui.heading("Uncertain WebDAV uploads");
                    ui.label("These records were saved before uploading. A timeout or crash may have left the stage, final file, both, or neither on the server. RiceField does not query, retry, move or delete these remote files during recovery.");
                    if let Some(error) = &self.remote_journal_error {
                        ui.colored_label(Color32::LIGHT_RED, format!("Cannot inspect WebDAV evidence: {error}"));
                    }
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for record in &self.remote_journal_records {
                        ui.group(|ui| {
                            ui.strong(format!("WebDAV upload {}", record.id));
                            ui.label("Intended destination:");
                            ui.monospace(&record.destination_uri);
                            ui.label("Private stage (may or may not exist):");
                            ui.monospace(&record.stage_uri);
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Copy stage URI").clicked() {
                                    context.copy_text(record.stage_uri.clone());
                                }
                                if ui.button("Copy destination URI").clicked() {
                                    context.copy_text(record.destination_uri.clone());
                                }
                            });
                            ui.small("Inspect both server-side paths independently and retain any ambiguous data. The record does not prove whether publication succeeded.");
                            if confirm_webdav.as_deref() == Some(record.id.as_str()) {
                                ui.colored_label(Color32::LIGHT_YELLOW, "Have you independently verified the remote stage and destination, and handled any retained data? Confirming removes ONLY this local journal record, not server files.");
                                ui.horizontal_wrapped(|ui| {
                                    if ui.add_enabled(!self.remote_journal_loading, egui::Button::new("Confirm discard local record")).clicked() {
                                        acknowledge_webdav = Some(record.clone());
                                        confirm_webdav = None;
                                    }
                                    if ui.button("Cancel").clicked() {
                                        confirm_webdav = None;
                                    }
                                });
                            } else if ui.add_enabled(!self.remote_journal_loading, egui::Button::new("I reviewed the server; discard record…")).clicked() {
                                confirm_webdav = Some(record.id.clone());
                            }
                        });
                    }
                    });
                }
                if self.remote_journal_loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking private WebDAV evidence…");
                    });
                }
                ui.separator();
                if ui.button("Rescan records").clicked() {
                    rescan = true;
                }
            });
        self.recovery_open = open;
        self.remote_journal_confirm = confirm_webdav;
        if let Some(record) = acknowledge_webdav {
            self.acknowledge_webdav_upload(record);
        }
        #[cfg(unix)]
        {
            self.recovery_resolution_confirm = confirm_resolution;
            if let Some(id) = resolve {
                self.resolve_recovery_record(id);
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(id) = publish_staged {
            self.publish_recovery_stage(id);
        }
        if !open {
            self.recovery_comparison_cancel
                .store(true, Ordering::Relaxed);
        }
        if let Some((record, staged)) = compare {
            self.compare_recovery_record(&record, staged);
        }
        if rescan {
            self.check_unfinished_moves();
            self.check_unfinished_webdav_uploads();
        }
        if let Some(folder) = browse_parent {
            self.recovery_open = false;
            self.recovery_comparison_cancel
                .store(true, Ordering::Relaxed);
            // Source/destination actions select their parent; staging opens
            // only if observed as a real directory. Scan on a worker.
            self.navigate(folder);
        }
        if let Some((source, destination)) = prepare_copy {
            self.recovery_open = false;
            self.recovery_comparison_cancel
                .store(true, Ordering::Relaxed);
            let clipboard = Clipboard {
                paths: vec![source],
                cut: false,
            };
            let conflict = ConflictPolicy::KeepBoth;
            let preview_generation = self.request_paste_preview(&clipboard, &destination, conflict);
            self.dialog = Some(Dialog::PasteOptions {
                clipboard,
                destination,
                conflict,
                preview_generation,
                preview: None,
            });
        }
    }

    /// One shared transfer surface for every rice. Historic and native-like
    /// shells keep it hidden when idle, while their menus/controls still
    /// expose transfer history whenever requested.
    fn transfer_strip(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        if matches!(
            theme.shell_kind(),
            ShellKind::Luna
                | ShellKind::Aero
                | ShellKind::Ribbon
                | ShellKind::Aqua
                | ShellKind::Finder
                | ShellKind::Chromeos
                | ShellKind::Classic
        ) && !self.busy
            && self.operation_queue.is_empty()
            && self.active_operation.is_none()
            && !self.show_operations
        {
            return;
        }
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
                            ui.colored_label(Color32::from_rgb(165, 83, 30), "Queue paused");
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
        let mut undo_request = None;
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
                if !self.operation_queue.is_empty() {
                    let label = if self.queue_paused {
                        "Resume waiting copies"
                    } else {
                        "Pause waiting copies"
                    };
                    if ui.button(label).clicked() {
                        self.queue_paused = !self.queue_paused;
                        self.status = Some((
                            if self.queue_paused {
                                "Waiting copies paused; any running transfer continues."
                            } else {
                                "Waiting copies resumed."
                            }
                            .into(),
                            false,
                        ));
                    }
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
                ui.small("Earlier sessions keep up to 24 read-only per-file results per operation (errors first). History never enables Undo or replay.");
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for entry in &self.recent_operations {
                        let outcome = match entry.outcome {
                            OperationOutcome::Succeeded => "Completed",
                            OperationOutcome::Cancelled => "Cancelled",
                            OperationOutcome::Failed => "Failed",
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(if entry.from_history {
                                format!("Earlier · {outcome}")
                            } else {
                                format!("#{} · {outcome}", entry.id)
                            });
                            ui.label(&entry.label);
                            ui.small(filesystem::age(Some(entry.finished_at)));
                            ui.small(format!("Took {}", elapsed_label(entry.elapsed)));
                            if let Some(candidate) = &entry.rename_undo
                                && ui
                                    .add_enabled(
                                        !self.busy
                                            && !self.queue_paused
                                            && self.operation_queue.is_empty(),
                                        egui::Button::new("Undo rename"),
                                    )
                                    .on_hover_text("Restore the previous name if this entry is unchanged and the original name is still free. Never overwrites another file; available only in this session.")
                                    .clicked()
                            {
                                undo_request = Some((entry.id, candidate.action()));
                            }
                            if let Some(candidate) = &entry.batch_rename_undo
                                && ui
                                    .add_enabled(
                                        !self.busy
                                            && !self.queue_paused
                                            && self.operation_queue.is_empty(),
                                        egui::Button::new("Undo batch rename"),
                                    )
                                    .on_hover_text("Restore every original filename in this completed batch only if all renamed entries still match their post-rename snapshots. Never overwrite unrelated entries; session-only.")
                                    .clicked()
                            {
                                undo_request = Some((entry.id, candidate.action()));
                            }
                        });
                        if entry.outcome == OperationOutcome::Failed {
                            ui.colored_label(Color32::LIGHT_RED, &entry.summary);
                        } else {
                            ui.small(&entry.summary);
                        }
                        if !entry.items.is_empty() {
                            let ItemCounts {
                                completed,
                                retained,
                                skipped,
                                failed,
                                not_started,
                            } = entry.counts;
                            ui.collapsing(
                                format!(
                                    "Per-file results: {completed} done · {retained} copied (source kept) · {skipped} skipped · {failed} errors · {not_started} not started"
                                ),
                                |ui| {
                                    if entry.from_history {
                                        let total = completed
                                            .saturating_add(retained)
                                            .saturating_add(skipped)
                                            .saturating_add(failed)
                                            .saturating_add(not_started);
                                        if total > entry.items.len() {
                                            ui.small(format!(
                                                "Showing {} of {total} items; failed or unfinished entries are retained first.",
                                                entry.items.len()
                                            ));
                                        }
                                        ui.small("Historical entries are display-only, not recovery or file-operation targets.");
                                    }
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
                                                        filesystem::ItemState::CopiedSourceRetained => "Copied — source kept",
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
        if let Some((origin_id, action)) = undo_request {
            self.run_action(action);
            if self.busy {
                self.pending_undo_origin = Some(origin_id);
            }
        }
        self.start_next_operation();
    }

    fn run_action(&mut self, action: Action) {
        if self
            .archive_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.running)
        {
            self.status = Some((
                "Wait for the archive operation or cancel it first".into(),
                true,
            ));
            return;
        }
        if self.current().remote_uri.is_some() {
            self.status = Some((
                "Remote writes are disabled: generic GIO rename cannot guarantee atomic no-overwrite publication".into(),
                true,
            ));
            return;
        }
        if self.queue_paused && !matches!(&action, Action::Copy { .. }) {
            self.status = Some((
                "Waiting copies are paused. Resume or remove them first.".into(),
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

    fn open_archive_browse(&mut self, path: PathBuf) {
        if self.busy
            || self
                .archive_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.running)
        {
            self.status = Some(("Finish the current operation first".into(), true));
            return;
        }
        if !archive::Format::is_archive(&path) || self.current().remote_uri.is_some() {
            self.status = Some(("Select a supported local archive".into(), true));
            return;
        }
        let destination = self.current().path.clone();
        let folder_name = format!("{}-extracted", archive_stem(&path));
        self.archive_dialog = Some(ArchiveDialog {
            mode: ArchiveMode::Browse {
                path: path.clone(),
                destination,
                entries: None,
                selected: BTreeSet::new(),
                folder_name,
                password: String::new(),
            },
            running: false,
            message: None,
            transferred: 0,
            current_file: String::new(),
        });
        self.start_archive_operation(ArchiveOperation::List {
            path,
            password: None,
        });
    }

    fn open_archive_create(&mut self) {
        if self.busy
            || self
                .archive_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.running)
        {
            self.status = Some(("Finish the current operation first".into(), true));
            return;
        }
        if self.current().remote_uri.is_some() {
            return;
        }
        let sources = self.selected_paths();
        if sources.is_empty() {
            return;
        }
        let stem = if sources.len() == 1 {
            sources[0]
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Archive".into())
        } else {
            "Archive".into()
        };
        self.archive_dialog = Some(ArchiveDialog {
            mode: ArchiveMode::Create {
                sources,
                destination: self.current().path.clone(),
                stem,
                format: archive::Format::Zip,
                encrypt: false,
                password: String::new(),
            },
            running: false,
            message: None,
            transferred: 0,
            current_file: String::new(),
        });
    }

    fn start_archive_operation(&mut self, operation: ArchiveOperation) {
        if self.busy {
            self.status = Some((
                "A file transfer is running; archive operation postponed".into(),
                true,
            ));
            return;
        }
        self.archive_cancel.store(true, Ordering::Relaxed);
        self.archive_cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&self.archive_cancel);
        self.archive_generation = self.archive_generation.wrapping_add(1);
        let generation = self.archive_generation;
        let sender = self.archive_sender.clone();
        if let Some(dialog) = self.archive_dialog.as_mut() {
            dialog.running = true;
            dialog.message = None;
            dialog.transferred = 0;
            dialog.current_file.clear();
        }
        thread::spawn(move || {
            let operation = match operation {
                ArchiveOperation::List { path, password } => {
                    let result =
                        archive::inspect_with_password(&path, password.as_deref(), &cancel)
                            .map_err(|error| error.to_string());
                    let _ = sender.send(ArchiveEvent::Listed { generation, result });
                    return;
                }
                other => other,
            };
            let changed = matches!(
                &operation,
                ArchiveOperation::Extract { .. } | ArchiveOperation::Create { .. }
            );
            let mut last_report = Instant::now() - Duration::from_secs(1);
            let mut total = 0u64;
            let mut report = |progress: archive::ArchiveProgress| {
                total = total.saturating_add(progress.bytes);
                if last_report.elapsed() >= Duration::from_millis(100) {
                    let _ = sender.send(ArchiveEvent::Progress {
                        generation,
                        bytes: total,
                        path: progress.path.display().to_string(),
                    });
                    last_report = Instant::now();
                }
            };
            let result = match operation {
                ArchiveOperation::Verify { path, password } => {
                    archive::verify_with_password(&path, password.as_deref(), &cancel, &mut report)
                        .map(|count| format!("Verified {count} archive entries"))
                }
                ArchiveOperation::Extract {
                    path,
                    destination,
                    folder,
                    selected,
                    password,
                } => archive::extract_with_password(
                    &path,
                    &destination,
                    &folder,
                    &selected,
                    password.as_deref(),
                    &cancel,
                    &mut report,
                )
                .map(|path| format!("Extracted to {}", path.display())),
                ArchiveOperation::Create {
                    sources,
                    destination,
                    stem,
                    format,
                    password,
                } => archive::create_with_password(
                    &sources,
                    &destination,
                    &stem,
                    format,
                    password.as_deref(),
                    &cancel,
                    &mut report,
                )
                .map(|path| format!("Created {}", path.display())),
                ArchiveOperation::List { .. } => unreachable!(),
            }
            .map_err(|error| error.to_string());
            let _ = sender.send(ArchiveEvent::Finished {
                generation,
                changed,
                result,
            });
        });
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
        self.clipboard_revision = self.clipboard_revision.wrapping_add(1);
        self.pending_paste = None;
        self.clipboard = Some(Clipboard {
            paths: paths.clone(),
            cut,
        });
        self.consumed_cut_paths = None;
        self.status = Some((
            format!(
                "{} {} item(s) (internal clipboard; syncing desktop clipboard)",
                if cut { "Cut" } else { "Copied" },
                paths.len(),
            ),
            false,
        ));
        if self.desktop_copy_running {
            // Keep only the latest copy request. A stuck desktop clipboard
            // provider must not spawn an unbounded set of write workers.
            self.pending_desktop_copy = Some((paths, cut, self.clipboard_revision));
        } else {
            self.start_desktop_copy(paths, cut, self.clipboard_revision);
        }
    }

    fn start_desktop_copy(&mut self, paths: Vec<PathBuf>, cut: bool, revision: u64) {
        self.desktop_copy_running = true;
        let sender = self.clipboard_sender.clone();
        thread::spawn(move || {
            let success = clipboard::copy_files(&paths, cut).is_ok();
            let _ = sender.send(ClipboardEvent::Written { revision, success });
        });
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

    fn clipboard_for_paste(&self, desktop: Option<clipboard::Files>) -> Option<Clipboard> {
        choose_paste_clipboard(
            desktop,
            self.clipboard.clone(),
            self.consumed_cut_paths.as_deref(),
        )
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
        self.request_desktop_paste(PasteMode::Immediate);
    }

    fn paste_with_options(&mut self) {
        self.request_desktop_paste(PasteMode::Options);
    }

    fn request_desktop_paste(&mut self, mode: PasteMode) {
        if self.pending_paste.is_some() {
            return;
        }
        let destination = self.current().path.clone();
        // A copy initiated here is authoritative until desktop sync returns.
        // If a desktop read has hung, do not start another unbounded worker.
        if self.desktop_copy_running {
            self.finish_paste(self.clipboard.clone(), destination, mode);
            return;
        }
        if self.desktop_read_running {
            if self.clipboard_revision != self.desktop_read_revision {
                // A newer in-app copy supersedes the hung external read.
                self.finish_paste(self.clipboard.clone(), destination, mode);
            } else {
                self.status = Some((
                    "Still waiting for the desktop clipboard; no files were pasted".into(),
                    true,
                ));
            }
            return;
        }
        self.next_paste_id = self.next_paste_id.wrapping_add(1);
        let id = self.next_paste_id;
        self.pending_paste = Some(PendingPaste {
            id,
            revision: self.clipboard_revision,
            tab_id: self.current().id,
            tab_generation: self.current().generation,
            destination,
            mode,
            started: Instant::now(),
        });
        self.desktop_read_running = true;
        self.desktop_read_revision = self.clipboard_revision;
        self.status = Some(("Reading desktop clipboard…".into(), false));
        let sender = self.clipboard_sender.clone();
        thread::spawn(move || {
            let files = clipboard::paste_files();
            let _ = sender.send(ClipboardEvent::Read { id, files });
        });
    }

    fn finish_paste(
        &mut self,
        clipboard: Option<Clipboard>,
        destination: PathBuf,
        mode: PasteMode,
    ) {
        let Some(clipboard) = clipboard else {
            self.status = Some((
                "No files on the clipboard (or desktop clipboard unavailable)".into(),
                true,
            ));
            return;
        };
        self.clipboard = Some(clipboard.clone());
        match mode {
            PasteMode::Immediate => self.run_action(Self::paste_action(
                clipboard,
                destination,
                ConflictPolicy::KeepBoth,
            )),
            PasteMode::Options => {
                let conflict = ConflictPolicy::KeepBoth;
                let preview_generation =
                    self.request_paste_preview(&clipboard, &destination, conflict);
                self.dialog = Some(Dialog::PasteOptions {
                    clipboard,
                    destination,
                    conflict,
                    preview_generation,
                    preview: None,
                });
            }
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
        if entry.is_dir {
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
            self.theme_file = None;
            self.theme_file_state = None;
            self.theme_reload_pending = None;
            self.config.theme = id.to_owned();
            self.config.custom_theme_file = None;
            self.dirty_config = true;
            self.theme.apply(context);
        }
    }

    fn theme_file_fingerprint(path: &Path) -> Option<(SystemTime, u64)> {
        let meta = fs::metadata(path).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    fn reload_theme_file(&mut self, context: &egui::Context, force: bool) {
        let Some(path) = self.theme_file.as_ref().cloned() else {
            return;
        };
        if self.theme_reload_pending.is_some() {
            return;
        }
        self.theme_reload_generation = self.theme_reload_generation.wrapping_add(1);
        let generation = self.theme_reload_generation;
        self.theme_reload_pending = Some(generation);
        let previous = self.theme_file_state;
        let sender = self.theme_reload_sender.clone();
        let context = context.clone();
        thread::spawn(move || {
            let fingerprint = Self::theme_file_fingerprint(&path);
            let theme = if !force && previous == fingerprint {
                Ok(None)
            } else {
                Theme::from_file(&path).and_then(|theme| {
                    (fingerprint == Self::theme_file_fingerprint(&path))
                        .then_some(Some(theme))
                        .ok_or_else(|| {
                            "Theme changed while reading; retry after next refresh".into()
                        })
                })
            };
            let _ = sender.send(ThemeReloadResult {
                generation,
                path,
                fingerprint,
                theme,
            });
            context.request_repaint();
        });
    }

    fn keyboard(&mut self, context: &egui::Context) {
        for (key, modifiers) in pressed_keys(context) {
            self.keyboard_event(context, key, modifiers);
            // A preceding event may open a confirmation in the same frame.
            if self.dialog.is_some()
                || self.volume_connect_open
                || self.volume_unlock_target.is_some()
                || self.volume_confirm.is_some()
            {
                break;
            }
        }
    }

    fn keyboard_event(&mut self, context: &egui::Context, key: Key, modifiers: Modifiers) {
        let (ctrl, alt, shift) = (modifiers.command, modifiers.alt, modifiers.shift);
        let l = key == Key::L;
        let f = key == Key::F;
        let t = key == Key::T;
        let w = key == Key::W;
        let c = key == Key::C;
        let x = key == Key::X;
        let v = key == Key::V;
        let h = key == Key::H;
        let r = key == Key::R;
        let n = key == Key::N;
        let f2 = key == Key::F2;
        let delete = key == Key::Delete;
        let left = key == Key::ArrowLeft;
        let right = key == Key::ArrowRight;
        let up = key == Key::ArrowUp;
        let down = key == Key::ArrowDown;
        let a = key == Key::A;
        let d = key == Key::D;
        let enter = key == Key::Enter;
        let space = key == Key::Space;
        let page_up = key == Key::PageUp;
        let page_down = key == Key::PageDown;
        let f3 = key == Key::F3;
        if ctrl && l {
            self.focus_address = true;
        }
        if ctrl && f {
            self.focus_search = true;
        }
        if ctrl && t {
            self.tab_command(if shift {
                TabCommand::ReopenClosed
            } else {
                TabCommand::New
            });
        }
        if ctrl && w {
            self.tab_command(TabCommand::Close(self.active));
        }
        if ctrl && !alt && page_up {
            self.tab_command(if shift {
                TabCommand::Move {
                    index: self.active,
                    direction: -1,
                }
            } else {
                TabCommand::Cycle(-1)
            });
        }
        if ctrl && !alt && page_down {
            self.tab_command(if shift {
                TabCommand::Move {
                    index: self.active,
                    direction: 1,
                }
            } else {
                TabCommand::Cycle(1)
            });
        }
        if f3 && !ctrl && !alt {
            self.navigation_command(NavigationCommand::Split);
        }
        // New-folder/new-window are application shortcuts, even after a tab
        // switch leaves an address/search widget holding keyboard focus.
        // Never dispatch them through an active modal dialog, however.
        if ctrl && n && !alt && self.dialog.is_none() {
            if shift {
                self.navigation_command(NavigationCommand::NewWindow);
            } else {
                self.file_command(FileCommand::NewFolder);
            }
        }
        // Text inputs must keep their normal copy/cut/paste shortcuts.
        let typing = context.egui_wants_keyboard_input() || self.focus_address || self.focus_search;
        if !typing {
            if self.current().remote_uri.is_some() && !ctrl && !alt {
                if up || down {
                    let tab = self.current_mut();
                    tab.remote_selected_uri = next_remote_selection(
                        &tab.remote_entries,
                        &tab.query,
                        tab.remote_selected_uri.as_deref(),
                        down,
                    );
                }
                if enter
                    && let Some(uri) = self.current().remote_selected_uri.clone()
                    && let Some(entry) = self
                        .current()
                        .remote_entries
                        .iter()
                        .find(|entry| {
                            entry.uri == uri && remote_matches_query(entry, &self.current().query)
                        })
                        .cloned()
                {
                    if entry.is_directory {
                        self.navigate_remote(uri);
                    } else if entry.is_regular_file {
                        self.start_remote_preview(uri);
                    }
                }
            }
            if key == Key::Escape {
                self.preview_quick_look = false;
                self.preview_path = None;
                self.preview_content = None;
                if self.current().remote_uri.is_some() {
                    self.current_mut().remote_selected_uri = None;
                    self.current_mut().clear_remote_preview();
                }
            }
            if ctrl && a && self.current().remote_uri.is_none() {
                let paths: Vec<_> = self
                    .current()
                    .visible
                    .iter()
                    .map(|&index| self.current().entries[index].path.clone())
                    .collect();
                self.current_mut().selected = paths.into_iter().collect();
            }
            if ctrl && c {
                if shift {
                    self.copy_selected_paths(context);
                } else {
                    self.file_command(FileCommand::Copy);
                }
            }
            if ctrl && x {
                self.file_command(FileCommand::Cut);
            }
            if ctrl && v {
                self.file_command(FileCommand::Paste);
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
            if space {
                if self.preview_quick_look {
                    self.preview_quick_look = false;
                    self.preview_path = None;
                    self.preview_content = None;
                } else if self.current().selected.len() == 1
                    && let Some(path) = self.selected_paths().first()
                {
                    self.preview_quick_look = true;
                    self.open_preview(path.clone());
                }
            }
            if alt
                && enter
                && self.current().selected.len() == 1
                && let Some(path) = self.selected_paths().first()
            {
                self.open_properties(path.clone());
            }
            if f2 {
                self.file_command(FileCommand::Rename);
            }
            if delete {
                if shift {
                    self.file_command(FileCommand::Delete);
                } else {
                    self.file_command(FileCommand::Trash);
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
            self.navigation_command(NavigationCommand::Hidden);
        }
        if ctrl && r {
            self.navigation_command(NavigationCommand::Refresh);
        }
        if alt && left {
            self.navigation_command(NavigationCommand::Back);
        }
        if alt && right {
            self.navigation_command(NavigationCommand::Forward);
        }
        if alt && up {
            self.navigation_command(NavigationCommand::Parent);
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
                    regex: false,
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

    fn find_duplicates_in_view(&mut self) {
        if self.current().remote_uri.is_some() {
            self.status = Some((
                "Duplicate scan is available for local folders only".into(),
                true,
            ));
            return;
        }
        let entries: Vec<_> = self
            .current()
            .visible
            .iter()
            .map(|&index| self.current().entries[index].clone())
            .collect();
        self.duplicate_cancel.store(true, Ordering::Relaxed);
        self.duplicate_cancel = Arc::new(AtomicBool::new(false));
        self.duplicate_generation = self.duplicate_generation.wrapping_add(1);
        let generation = self.duplicate_generation;
        self.dialog = Some(Dialog::Duplicates {
            generation,
            result: None,
        });
        let stop = Arc::clone(&self.duplicate_cancel);
        let sender = self.duplicate_sender.clone();
        thread::spawn(move || {
            let result = duplicates::find(&entries, &stop).map_err(|error| error.to_string());
            let _ = sender.send((generation, result));
        });
    }

    fn choose_open_with(&mut self) {
        if self.current().remote_uri.is_some() {
            return;
        }
        let selected = self.selected_paths();
        let [path] = selected.as_slice() else {
            return;
        };
        self.choose_open_with_path(path.clone());
    }

    fn choose_open_with_path(&mut self, path: PathBuf) {
        if self.current().remote_uri.is_some() || !path.is_absolute() {
            return;
        }
        self.open_with_cancel.store(true, Ordering::Relaxed);
        self.open_with_cancel = Arc::new(AtomicBool::new(false));
        self.open_with_generation = self.open_with_generation.wrapping_add(1);
        let generation = self.open_with_generation;
        self.dialog = Some(Dialog::OpenWith {
            path: path.clone(),
            generation,
            apps: None,
        });
        let cancelled = Arc::clone(&self.open_with_cancel);
        let sender = self.open_with_sender.clone();
        thread::spawn(move || {
            let result = open_with::applications(&path, &cancelled);
            let _ = sender.send((generation, result));
        });
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
        self.analysis_cancel.store(true, Ordering::Relaxed);
        self.property_generation = self.property_generation.wrapping_add(1);
        let generation = self.property_generation;
        self.dialog = Some(Dialog::Properties {
            path: path.clone(),
            mode: String::new(),
            owner: String::new(),
            group: String::new(),
            attribute_name: "user.".into(),
            attribute_value: String::new(),
            generation,
            details: None,
            analysis: None,
        });
        if self.property_worker_running {
            self.property_pending = Some((generation, path));
        } else {
            self.start_property_worker(generation, path);
        }
    }

    fn start_property_worker(&mut self, generation: u64, path: PathBuf) {
        self.property_worker_running = true;
        let sender = self.property_sender.clone();
        thread::spawn(move || {
            let result = read_property_snapshot(&path).map_err(|error| error.to_string());
            let _ = sender.send(PropertyResult {
                generation,
                path,
                result,
            });
        });
    }

    fn start_deep_analysis(&mut self, path: PathBuf, generation: u64) {
        if self.analysis_worker_running {
            return;
        }
        self.analysis_worker_running = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.analysis_cancel = Arc::clone(&cancel);
        let sender = self.analysis_sender.clone();
        thread::spawn(move || {
            let result = metadata::deep_analysis(&path, &cancel).map_err(|error| error.to_string());
            let _ = sender.send(PropertyAnalysis {
                generation,
                path,
                result,
            });
        });
    }

    fn open_preview(&mut self, path: PathBuf) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        self.preview_path = Some(path.clone());
        self.preview_content = None;
        self.preview_loading = true;
        if self.preview_worker_running {
            self.preview_pending = Some((generation, path));
        } else {
            self.start_preview_worker(generation, path);
        }
    }

    fn start_preview_worker(&mut self, generation: u64, path: PathBuf) {
        self.preview_worker_running = true;
        let sender = self.preview_sender.clone();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(|| preview::load(&path))
                .unwrap_or_else(|_| Err(std::io::Error::other("Preview decoder failed")));
            let _ = sender.send(PreviewResult {
                generation,
                path,
                result,
            });
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
                    .inner_margin(
                        if matches!(theme.shell_kind(), ShellKind::Luna | ShellKind::Classic) {
                            egui::Margin::ZERO
                        } else if theme.shell_kind() == ShellKind::Aero {
                            egui::Margin::symmetric(8, 5)
                        } else {
                            egui::Margin::same(7)
                        },
                    ),
            )
            .show(ui, |ui| {
                for (index, component) in components.iter().enumerate() {
                    if theme.shell_kind() == ShellKind::Classic {
                        if *component == ChromeComponent::Tabs && self.tabs.len() == 1 {
                            continue;
                        }
                        egui::Frame::new()
                            .fill(Color32::from_rgb(192, 192, 192))
                            .stroke(egui::Stroke::new(0.7, Color32::from_gray(102)))
                            .inner_margin(egui::Margin::symmetric(5, 3))
                            .show(ui, |ui| match component {
                                ChromeComponent::MenuBar => self.menu_bar(ui),
                                ChromeComponent::Tabs => self.tabs_bar(ui),
                                ChromeComponent::Navigation => self.navigation_bar(ui),
                                ChromeComponent::CommandBar => self.command_bar(ui, &theme),
                            });
                        continue;
                    }
                    if theme.shell_kind() == ShellKind::Aero
                        && *component == ChromeComponent::Tabs
                        && self.tabs.len() == 1
                    {
                        continue;
                    }
                    // Windows 10's classic Explorer has a ribbon but no
                    // single-folder tab row; keep RiceField's multi-tab UI
                    // when two or more tabs are actually open.
                    if matches!(theme.shell_kind(), ShellKind::Ribbon | ShellKind::Chromeos)
                        && *component == ChromeComponent::Tabs
                        && self.tabs.len() == 1
                    {
                        continue;
                    }
                    if theme.shell_kind() == ShellKind::Luna {
                        if *component == ChromeComponent::Tabs && self.tabs.len() == 1 {
                            continue;
                        }
                        // In Luna the menu, navigation toolbar and tab strip
                        // occupy their own bands; decorations remain the WM's
                        // responsibility, not RiceField's.
                        let fill = match component {
                            ChromeComponent::MenuBar => Color32::from_rgb(246, 245, 239),
                            ChromeComponent::Navigation => Color32::from_rgb(238, 236, 226),
                            ChromeComponent::Tabs => Color32::from_rgb(207, 222, 247),
                            ChromeComponent::CommandBar => Color32::from_rgb(250, 249, 244),
                        };
                        egui::Frame::new()
                            .fill(fill)
                            .inner_margin(egui::Margin::symmetric(7, 4))
                            .show(ui, |ui| match component {
                                ChromeComponent::MenuBar => self.menu_bar(ui),
                                ChromeComponent::Tabs => self.tabs_bar(ui),
                                ChromeComponent::Navigation => self.navigation_bar(ui),
                                ChromeComponent::CommandBar => self.command_bar(ui, &theme),
                            });
                        continue;
                    }
                    if index != 0 && theme.shell_kind() != ShellKind::Aero {
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
        let is_luna = self.theme.shell_kind() == ShellKind::Luna;
        let is_classic = self.theme.shell_kind() == ShellKind::Classic;
        let menu_text = if is_classic {
            Color32::BLACK
        } else if is_luna {
            Color32::from_rgb(33, 48, 77)
        } else {
            Color32::WHITE
        };
        ui.horizontal(|ui| {
            if !is_luna && !is_classic {
                ui.label(egui::RichText::new("RiceField").color(menu_text).strong());
                ui.separator();
            }
            ui.menu_button(egui::RichText::new("File").color(menu_text), |ui| {
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
            ui.menu_button(egui::RichText::new("Edit").color(menu_text), |ui| {
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
            ui.menu_button(egui::RichText::new("View").color(menu_text), |ui| {
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
                    self.toggle_hidden();
                    ui.close();
                }
                if is_luna {
                    ui.separator();
                    if ui.button("Show folder tree").clicked() {
                        shell_components::luna_toggle_panel(
                            ui.ctx(),
                            shell_components::LunaPanel::FolderTree,
                        );
                        ui.close();
                    }
                    if ui.button("Search in folder").clicked() {
                        shell_components::luna_toggle_panel(
                            ui.ctx(),
                            shell_components::LunaPanel::Search,
                        );
                        ui.close();
                    }
                }
            });
            if is_classic {
                ui.menu_button(egui::RichText::new("Go").color(menu_text), |ui| {
                    for (label, action) in [
                        ("Back  Alt+Left", ClassicAction::Back),
                        ("Forward  Alt+Right", ClassicAction::Forward),
                        ("Up One Level", ClassicAction::Up),
                        ("Refresh  Ctrl+R", ClassicAction::Refresh),
                        ("Network Neighborhood", ClassicAction::Network),
                    ] {
                        if ui
                            .add_enabled(
                                self.classic98_context().enabled(action),
                                egui::Button::new(label),
                            )
                            .clicked()
                        {
                            self.classic98_command(action);
                            ui.close();
                        }
                    }
                    if ui.button("Home").clicked() {
                        self.navigate(config::home());
                        ui.close();
                    }
                });
                ui.menu_button(egui::RichText::new("Favorites").color(menu_text), |ui| {
                    if ui
                        .add_enabled(
                            self.current().remote_uri.is_none(),
                            egui::Button::new("Add current folder"),
                        )
                        .clicked()
                    {
                        self.toggle_bookmark();
                        ui.close();
                    }
                    ui.separator();
                    for path in self.config.bookmarks.clone() {
                        if ui
                            .button(path.file_name().unwrap_or_default().to_string_lossy())
                            .clicked()
                        {
                            self.navigate(path);
                            ui.close();
                        }
                    }
                });
                ui.menu_button(egui::RichText::new("Tools").color(menu_text), |ui| {
                    if ui
                        .button(if self.classic98_advanced_open {
                            "Hide advanced tools"
                        } else {
                            "Show advanced tools"
                        })
                        .clicked()
                    {
                        self.classic98_advanced_open = !self.classic98_advanced_open;
                        ui.close();
                    }
                    if ui.button("Connect to server…").clicked() {
                        self.open_network_login(String::new());
                        ui.close();
                    }
                    if ui.button("Transfers & history…").clicked() {
                        self.show_operations = true;
                        ui.close();
                    }
                });
                ui.menu_button(egui::RichText::new("Help").color(menu_text), |ui| {
                    ui.strong("RiceField · Classic Explorer");
                    ui.label("A Windows 98-era inspired Linux file manager.");
                    ui.small("No Microsoft assets or bundled Windows applications.");
                    ui.label("Ctrl+N folder · F2 rename · F3 split · Ctrl+F search");
                });
            }
            if is_luna {
                ui.menu_button(egui::RichText::new("Favorites").color(menu_text), |ui| {
                    if ui.button("Add to Favorites").clicked() {
                        self.toggle_bookmark();
                        ui.close();
                    }
                    ui.separator();
                    for path in self.config.bookmarks.clone() {
                        let label = path.file_name().unwrap_or_default().to_string_lossy();
                        if ui.button(label).clicked() {
                            self.navigate(path);
                            ui.close();
                        }
                    }
                });
                ui.menu_button(egui::RichText::new("Tools").color(menu_text), |ui| {
                    let open = shell_components::luna_panel_open(
                        ui.ctx(),
                        shell_components::LunaPanel::Advanced,
                    );
                    if ui
                        .button(if open {
                            "Hide RiceField tools"
                        } else {
                            "Show RiceField tools"
                        })
                        .clicked()
                    {
                        shell_components::luna_toggle_panel(
                            ui.ctx(),
                            shell_components::LunaPanel::Advanced,
                        );
                        ui.close();
                    }
                    if ui.button("Discover network locations…").clicked() {
                        self.discovery_open = true;
                        if self.discovery_hosts.is_empty() {
                            self.start_discovery();
                        }
                        ui.close();
                    }
                    if ui.button("Transfers and history…").clicked() {
                        self.show_operations = true;
                        ui.close();
                    }
                });
                ui.menu_button(egui::RichText::new("Help").color(menu_text), |ui| {
                    ui.strong("RiceField · XP Luna");
                    ui.label("A Linux file manager with original XP-inspired visuals.");
                    ui.label("Keyboard shortcuts: Ctrl+N, Ctrl+T, F3, Ctrl+F.");
                });
            }
        });
        if self.theme.shell_kind() != ShellKind::Luna {
            ui.separator();
        }
    }

    fn command_bar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        if theme.shell_kind() == ShellKind::Chromeos {
            if let Some(action) = chromeos::secondary_commands(ui, self.chromeos_context()) {
                self.chromeos_command(action);
            }
            return;
        }
        if theme.shell_kind() == ShellKind::Classic {
            if let Some(action) = classic98::secondary_commands(ui, self.classic98_context()) {
                self.classic98_command(action);
            }
            return;
        }
        if theme.shell_kind() == ShellKind::Finder {
            // Custom TOML compositions may still explicitly request this
            // optional secondary strip. The default Finder theme uses a
            // single consolidated navigation toolbar instead.
            if let Some(action) = finder::secondary_commands(ui, self.finder_context()) {
                self.finder_command(action);
            }
            return;
        }
        if theme.shell_kind() == ShellKind::Aqua {
            self.aqua_command_bar(ui, theme);
            return;
        }
        if theme.shell_kind() == ShellKind::Aero {
            self.aero_command_bar(ui);
            return;
        }
        if theme.shell_kind() == ShellKind::Ribbon {
            self.win10_command_bar(ui, theme);
            return;
        }
        let has_selection = !self.current().selected.is_empty();
        match shell_components::command_bar(ui, theme, has_selection) {
            Some(ToolbarAction::NewFolder) => self.file_command(FileCommand::NewFolder),
            Some(ToolbarAction::Copy) => self.file_command(FileCommand::Copy),
            Some(ToolbarAction::Cut) => self.file_command(FileCommand::Cut),
            Some(ToolbarAction::Paste) => self.file_command(FileCommand::Paste),
            Some(ToolbarAction::Rename) => self.file_command(FileCommand::Rename),
            Some(ToolbarAction::Trash) => self.file_command(FileCommand::Trash),
            Some(ToolbarAction::Properties) => self.file_command(FileCommand::Properties),
            Some(ToolbarAction::Bookmark) => self.toggle_bookmark(),
            Some(ToolbarAction::Refresh) => self.navigation_command(NavigationCommand::Refresh),
            None => {}
        }
    }

    fn aqua_command_bar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let state = AquaContext {
            local: self.current().remote_uri.is_none(),
            selected: self.current().selected.len(),
            pinned: self.config.bookmarks.contains(&self.current().path),
            grid: self.config.view == "grid",
            split: self.split_pair.is_some(),
            advanced: self.aqua_advanced_open,
        };
        if let Some(action) = aqua::dock(ui, theme, state) {
            self.aqua_command(action);
        }
    }

    fn aqua_command(&mut self, action: AquaAction) {
        match action {
            AquaAction::NewFolder => self.file_command(FileCommand::NewFolder),
            AquaAction::Favorite => self.toggle_bookmark(),
            AquaAction::Copy => self.file_command(FileCommand::Copy),
            AquaAction::Cut => self.file_command(FileCommand::Cut),
            AquaAction::Paste => self.file_command(FileCommand::Paste),
            AquaAction::Rename => self.file_command(FileCommand::Rename),
            AquaAction::Trash => self.file_command(FileCommand::Trash),
            AquaAction::Preview => {
                if let Some(path) = self.selected_paths().first().cloned() {
                    if self.preview_path.as_ref() == Some(&path) {
                        self.preview_path = None;
                        self.preview_content = None;
                    } else {
                        self.open_preview(path);
                    }
                }
            }
            AquaAction::ToggleView => {
                self.config.view = if self.config.view == "grid" {
                    "list"
                } else {
                    "grid"
                }
                .into();
                self.dirty_config = true;
            }
            AquaAction::Split => self.navigation_command(NavigationCommand::Split),
            AquaAction::Tools => self.aqua_advanced_open = !self.aqua_advanced_open,
            AquaAction::Discover => {
                self.discovery_open = true;
                if self.discovery_hosts.is_empty() {
                    self.start_discovery();
                }
            }
            AquaAction::Refresh => self.navigation_command(NavigationCommand::Refresh),
            AquaAction::Transfers => self.show_operations = true,
        }
    }

    fn win10_command_bar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let state = RibbonContext {
            local: self.current().remote_uri.is_none(),
            selected: self.current().selected.len(),
            has_items: !self.current().visible.is_empty(),
            show_sidebar: self.config.show_sidebar,
            show_hidden: self.config.show_hidden,
            split: self.split_pair.is_some(),
            preview: self.preview_path.is_some(),
            grid: self.config.view == "grid",
            advanced: self.win10_advanced_open,
            volume_busy: self.volume_busy,
        };
        let command = win10::ribbon(
            ui,
            theme,
            &mut self.win10_ribbon_page,
            &mut self.win10_ribbon_expanded,
            state,
        );
        if let Some(command) = command {
            self.win10_command(command, ui.ctx());
        }
    }

    fn win10_command(&mut self, command: RibbonAction, context: &egui::Context) {
        match command {
            RibbonAction::NewWindow => self.navigation_command(NavigationCommand::NewWindow),
            RibbonAction::NewTab => self.tab_command(TabCommand::New),
            RibbonAction::NewFolder => self.file_command(FileCommand::NewFolder),
            RibbonAction::NewFile => {
                self.dialog = Some(Dialog::Name {
                    mode: NameMode::File(self.current().path.clone()),
                    value: String::new(),
                });
            }
            RibbonAction::Cut => self.file_command(FileCommand::Cut),
            RibbonAction::Copy => self.file_command(FileCommand::Copy),
            RibbonAction::Paste => self.file_command(FileCommand::Paste),
            RibbonAction::CopyPath => self.copy_selected_paths(context),
            RibbonAction::Rename => self.file_command(FileCommand::Rename),
            RibbonAction::Trash => self.file_command(FileCommand::Trash),
            RibbonAction::Properties => self.file_command(FileCommand::Properties),
            RibbonAction::Favorite => self.toggle_bookmark(),
            RibbonAction::SelectAll => {
                let paths = self
                    .current()
                    .visible
                    .iter()
                    .map(|&index| self.current().entries[index].path.clone())
                    .collect();
                self.current_mut().selected = paths;
            }
            RibbonAction::SelectNone => self.current_mut().selected.clear(),
            RibbonAction::InvertSelection => {
                let chosen = self.current().selected.clone();
                let inverse = self
                    .current()
                    .visible
                    .iter()
                    .filter_map(|&index| self.current().entries.get(index))
                    .map(|entry| entry.path.clone())
                    .filter(|path| !chosen.contains(path))
                    .collect();
                self.current_mut().selected = inverse;
            }
            RibbonAction::Archive => self.open_archive_create(),
            RibbonAction::FindNetwork => {
                self.discovery_open = true;
                if self.discovery_hosts.is_empty() {
                    self.start_discovery();
                }
            }
            RibbonAction::ConnectNetwork => self.open_network_login(String::new()),
            RibbonAction::Transfers => self.show_operations = true,
            RibbonAction::Details | RibbonAction::LargeIcons => {
                self.config.view = if command == RibbonAction::Details {
                    "list"
                } else {
                    "grid"
                }
                .into();
                self.dirty_config = true;
            }
            RibbonAction::NavigationPane => {
                self.config.show_sidebar = !self.config.show_sidebar;
                self.dirty_config = true;
            }
            RibbonAction::PreviewPane => {
                if let Some(path) = self.selected_paths().first().cloned() {
                    if self.preview_path.as_ref() == Some(&path) {
                        self.preview_path = None;
                        self.preview_content = None;
                    } else {
                        self.open_preview(path);
                    }
                }
            }
            RibbonAction::SplitView => self.navigation_command(NavigationCommand::Split),
            RibbonAction::HiddenItems => self.navigation_command(NavigationCommand::Hidden),
            RibbonAction::MoreTools => self.win10_advanced_open = !self.win10_advanced_open,
            RibbonAction::Refresh => self.navigation_command(NavigationCommand::Refresh),
        }
    }

    fn tabs_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let mut action: Option<TabCommand> = None;
            for index in 0..self.tabs.len() {
                let tab = &self.tabs[index];
                let label = tab.remote_uri.as_ref().map_or_else(
                    || {
                        tab.path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "/".into())
                    },
                    |uri| {
                        uri.trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .unwrap_or("Network")
                            .to_owned()
                    },
                );
                let selected = self.active == index;
                let path_hint = tab
                    .remote_uri
                    .clone()
                    .unwrap_or_else(|| tab.path.display().to_string());
                let response = ui
                    .add(egui::Button::new(label).selected(selected))
                    .on_hover_text(format!("{path_hint}\nRight-click for tab actions"));
                if response.clicked() {
                    action = Some(TabCommand::Activate(index));
                }
                response.context_menu(|ui| {
                    if ui.button("Duplicate tab").clicked() {
                        action = Some(TabCommand::Duplicate(index));
                        ui.close();
                    }
                    if ui
                        .add_enabled(index > 0, egui::Button::new("Move tab left"))
                        .clicked()
                    {
                        action = Some(TabCommand::Move {
                            index,
                            direction: -1,
                        });
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            index + 1 < self.tabs.len(),
                            egui::Button::new("Move tab right"),
                        )
                        .clicked()
                    {
                        action = Some(TabCommand::Move {
                            index,
                            direction: 1,
                        });
                        ui.close();
                    }
                    if ui.button("Close tab").clicked() {
                        action = Some(TabCommand::Close(index));
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            !self.closed_tabs.is_empty(),
                            egui::Button::new("Reopen closed tab (Ctrl+Shift+T)"),
                        )
                        .clicked()
                    {
                        action = Some(TabCommand::ReopenClosed);
                        ui.close();
                    }
                });
                if selected && ui.small_button("×").on_hover_text("Close tab").clicked() {
                    action = Some(TabCommand::Close(index));
                }
            }
            if ui
                .small_button("+")
                .on_hover_text("New tab (Ctrl+T)")
                .clicked()
            {
                action = Some(TabCommand::New);
            }
            if let Some(action) = action {
                self.tab_command(action);
            }
        });
    }

    fn navigation_bar(&mut self, ui: &mut egui::Ui) {
        if self.theme.shell_kind() == ShellKind::Chromeos {
            self.chromeos_navigation_bar(ui);
            return;
        }
        if self.theme.shell_kind() == ShellKind::Classic {
            self.classic98_navigation_bar(ui);
            return;
        }
        if self.theme.shell_kind() == ShellKind::Finder {
            self.finder_navigation_bar(ui);
            return;
        }
        if self.theme.shell_kind() == ShellKind::Aqua {
            self.aqua_navigation_bar(ui);
            return;
        }
        if self.theme.shell_kind() == ShellKind::Luna {
            self.luna_navigation_bar(ui);
            return;
        }
        if self.theme.shell_kind() == ShellKind::Aero {
            self.aero_navigation_bar(ui);
            return;
        }
        if self.theme.shell_kind() == ShellKind::Ribbon {
            self.win10_navigation_bar(ui);
            return;
        }
        if ui.available_width() < 800.0 {
            // Give address/search their own row on narrow windows. Neither
            // control may push the path field or Refresh off-screen.
            ui.vertical(|ui| {
                ui.horizontal(|ui| self.navigation_buttons(ui));
                ui.horizontal(|ui| self.navigation_address(ui, true));
                ui.horizontal(|ui| self.navigation_search(ui, true));
            });
        } else {
            ui.horizontal(|ui| {
                self.navigation_buttons(ui);
                self.navigation_address(ui, false);
                self.navigation_search(ui, false);
            });
        }
    }

    fn chromeos_navigation_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let title = self
            .current()
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "My files".into());
        if let Some(action) = chromeos::toolbar(ui, &theme, &title, self.chromeos_context()) {
            self.chromeos_command(action);
        }
        let narrow = ui.available_width() < 730.0;
        ui.horizontal(|ui| {
            let width = if narrow {
                ui.available_width().max(90.0)
            } else {
                (ui.available_width() - 265.0).max(130.0)
            };
            self.breadcrumb_bar(ui, width);
            if !narrow {
                self.navigation_search(ui, false);
            }
        });
        if narrow {
            ui.horizontal(|ui| self.navigation_search(ui, true));
        }
    }

    fn chromeos_context(&self) -> FilesContext {
        FilesContext {
            local: self.current().remote_uri.is_none(),
            selection: self.current().selected.len(),
            back: !self.current().back.is_empty(),
            forward: !self.current().forward.is_empty(),
            grid: self.config.view == "grid",
            split: self.split_pair.is_some(),
            sidebar: self.config.show_sidebar,
            hidden: self.config.show_hidden,
            pinned: self.config.bookmarks.contains(&self.current().path),
            tools: self.chromeos_advanced_open,
            volume_busy: self.volume_busy,
        }
    }

    fn chromeos_command(&mut self, action: FilesAction) {
        match action {
            FilesAction::Back => self.navigation_command(NavigationCommand::Back),
            FilesAction::Forward => self.navigation_command(NavigationCommand::Forward),
            FilesAction::Parent => self.navigation_command(NavigationCommand::Parent),
            FilesAction::Grid | FilesAction::List => {
                self.config.view = if action == FilesAction::Grid {
                    "grid"
                } else {
                    "list"
                }
                .into();
                self.dirty_config = true;
            }
            FilesAction::NewFolder => self.file_command(FileCommand::NewFolder),
            FilesAction::Refresh => self.navigation_command(NavigationCommand::Refresh),
            FilesAction::Copy => self.file_command(FileCommand::Copy),
            FilesAction::Cut => self.file_command(FileCommand::Cut),
            FilesAction::Paste => self.file_command(FileCommand::Paste),
            FilesAction::Rename => self.file_command(FileCommand::Rename),
            FilesAction::Trash => self.file_command(FileCommand::Trash),
            FilesAction::Info => self.file_command(FileCommand::Properties),
            FilesAction::Preview => {
                if self.preview_quick_look {
                    self.preview_quick_look = false;
                    self.preview_path = None;
                    self.preview_content = None;
                } else if let Some(path) = self.selected_paths().first().cloned() {
                    self.preview_quick_look = true;
                    self.open_preview(path);
                }
            }
            FilesAction::Pin => self.toggle_bookmark(),
            FilesAction::Split => self.navigation_command(NavigationCommand::Split),
            FilesAction::Sidebar => {
                self.config.show_sidebar = !self.config.show_sidebar;
                self.dirty_config = true;
            }
            FilesAction::Hidden => self.navigation_command(NavigationCommand::Hidden),
            FilesAction::Network => {
                self.discovery_open = true;
                if self.discovery_hosts.is_empty() {
                    self.start_discovery();
                }
            }
            FilesAction::Connect => self.open_network_login(String::new()),
            FilesAction::Transfers => self.show_operations = true,
            FilesAction::Tools => self.chromeos_advanced_open = !self.chromeos_advanced_open,
        }
    }

    fn classic98_navigation_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(action) = classic98::toolbar(ui, self.classic98_context()) {
            self.classic98_command(action);
        }
        let narrow = ui.available_width() < 760.0;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Address").color(Color32::BLACK));
            let width = if narrow {
                (ui.available_width() - 65.0).max(90.0)
            } else {
                (ui.available_width() - 260.0).max(155.0)
            };
            let input = ui.add_sized(
                [width, 25.0],
                egui::TextEdit::singleline(&mut self.current_mut().address)
                    .hint_text("Enter a local folder or network location"),
            );
            if self.focus_address {
                ui.memory_mut(|memory| memory.request_focus(input.id));
                self.focus_address = false;
            }
            if input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let address = self.current().address.clone();
                self.navigate_address(address);
            }
            if ui.button("Go").clicked() {
                let address = self.current().address.clone();
                self.navigate_address(address);
            }
            if !narrow {
                self.navigation_search(ui, false);
            }
        });
        if narrow {
            ui.horizontal(|ui| self.navigation_search(ui, true));
        }
    }

    fn classic98_context(&self) -> ClassicContext {
        ClassicContext {
            local: self.current().remote_uri.is_none(),
            selection: self.current().selected.len(),
            back: !self.current().back.is_empty(),
            forward: !self.current().forward.is_empty(),
            grid: self.config.view == "grid",
            split: self.split_pair.is_some(),
            hidden: self.config.show_hidden,
            sidebar: self.config.show_sidebar,
            pinned: self.config.bookmarks.contains(&self.current().path),
            tools: self.classic98_advanced_open,
            volume_busy: self.volume_busy,
        }
    }

    fn classic98_command(&mut self, action: ClassicAction) {
        match action {
            ClassicAction::Back => self.navigation_command(NavigationCommand::Back),
            ClassicAction::Forward => self.navigation_command(NavigationCommand::Forward),
            ClassicAction::Up => self.navigation_command(NavigationCommand::Parent),
            ClassicAction::NewFolder => self.file_command(FileCommand::NewFolder),
            ClassicAction::Copy => self.file_command(FileCommand::Copy),
            ClassicAction::Cut => self.file_command(FileCommand::Cut),
            ClassicAction::Paste => self.file_command(FileCommand::Paste),
            ClassicAction::Rename => self.file_command(FileCommand::Rename),
            ClassicAction::Delete => self.file_command(FileCommand::Trash),
            ClassicAction::Info => self.file_command(FileCommand::Properties),
            ClassicAction::Grid | ClassicAction::List => {
                self.config.view = if action == ClassicAction::Grid {
                    "grid"
                } else {
                    "list"
                }
                .into();
                self.dirty_config = true;
            }
            ClassicAction::Preview => {
                if self.preview_quick_look {
                    self.preview_quick_look = false;
                    self.preview_path = None;
                    self.preview_content = None;
                } else if let Some(path) = self.selected_paths().first().cloned() {
                    self.preview_quick_look = true;
                    self.open_preview(path);
                }
            }
            ClassicAction::Favorites => self.toggle_bookmark(),
            ClassicAction::Split => self.navigation_command(NavigationCommand::Split),
            ClassicAction::Hidden => self.navigation_command(NavigationCommand::Hidden),
            ClassicAction::Sidebar => {
                self.config.show_sidebar = !self.config.show_sidebar;
                self.dirty_config = true;
            }
            ClassicAction::Refresh => self.navigation_command(NavigationCommand::Refresh),
            ClassicAction::Network => {
                self.discovery_open = true;
                if self.discovery_hosts.is_empty() {
                    self.start_discovery();
                }
            }
            ClassicAction::Connect => self.open_network_login(String::new()),
            ClassicAction::Transfers => self.show_operations = true,
            ClassicAction::Tools => self.classic98_advanced_open = !self.classic98_advanced_open,
        }
    }
    fn finder_navigation_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let title = self
            .current()
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Computer".into());
        let state = self.finder_context();
        if let Some(action) = finder::toolbar(ui, &theme, &title, state) {
            self.finder_command(action);
        }
        let narrow = ui.available_width() < 730.0;
        ui.horizontal(|ui| {
            let width = if narrow {
                ui.available_width().max(90.0)
            } else {
                (ui.available_width() - 268.0).max(130.0)
            };
            self.breadcrumb_bar(ui, width);
            if !narrow {
                self.navigation_search(ui, false);
            }
        });
        if narrow {
            ui.horizontal(|ui| self.navigation_search(ui, true));
        }
    }

    fn finder_context(&self) -> FinderContext {
        FinderContext {
            local: self.current().remote_uri.is_none(),
            selection: self.current().selected.len(),
            back: !self.current().back.is_empty(),
            forward: !self.current().forward.is_empty(),
            grid: self.config.view == "grid",
            split: self.split_pair.is_some(),
            sidebar: self.config.show_sidebar,
            hidden: self.config.show_hidden,
            pinned: self.config.bookmarks.contains(&self.current().path),
            tools: self.finder_advanced_open,
            volume_busy: self.volume_busy,
        }
    }

    fn finder_command(&mut self, action: FinderAction) {
        match action {
            FinderAction::Back => self.navigation_command(NavigationCommand::Back),
            FinderAction::Forward => self.navigation_command(NavigationCommand::Forward),
            FinderAction::Up => self.navigation_command(NavigationCommand::Parent),
            FinderAction::Grid | FinderAction::List => {
                self.config.view = if action == FinderAction::Grid {
                    "grid"
                } else {
                    "list"
                }
                .into();
                self.dirty_config = true;
            }
            FinderAction::QuickLook => {
                if self.preview_quick_look {
                    self.preview_quick_look = false;
                    self.preview_path = None;
                    self.preview_content = None;
                } else if let Some(path) = self.selected_paths().first().cloned() {
                    self.preview_quick_look = true;
                    self.open_preview(path);
                }
            }
            FinderAction::NewFolder => self.file_command(FileCommand::NewFolder),
            FinderAction::Copy => self.file_command(FileCommand::Copy),
            FinderAction::Cut => self.file_command(FileCommand::Cut),
            FinderAction::Paste => self.file_command(FileCommand::Paste),
            FinderAction::Rename => self.file_command(FileCommand::Rename),
            FinderAction::Trash => self.file_command(FileCommand::Trash),
            FinderAction::Info => self.file_command(FileCommand::Properties),
            FinderAction::Pin => self.toggle_bookmark(),
            FinderAction::Split => self.navigation_command(NavigationCommand::Split),
            FinderAction::Sidebar => {
                self.config.show_sidebar = !self.config.show_sidebar;
                self.dirty_config = true;
            }
            FinderAction::Hidden => self.navigation_command(NavigationCommand::Hidden),
            FinderAction::Network => {
                self.discovery_open = true;
                if self.discovery_hosts.is_empty() {
                    self.start_discovery();
                }
            }
            FinderAction::Connect => self.open_network_login(String::new()),
            FinderAction::Refresh => self.navigation_command(NavigationCommand::Refresh),
            FinderAction::Tools => self.finder_advanced_open = !self.finder_advanced_open,
            FinderAction::Transfers => self.show_operations = true,
        }
    }

    fn aqua_navigation_bar(&mut self, ui: &mut egui::Ui) {
        let label = self
            .current()
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "My spaces".to_owned());
        aqua::masthead(ui, &self.theme, &label, self.current().remote_uri.is_some());
        let theme = self.theme.clone();
        let narrow = ui.available_width() < 810.0;
        ui.horizontal(|ui| {
            if aqua::nav_button(
                ui,
                &theme,
                "<",
                "Back (Alt+Left)",
                !self.current().back.is_empty(),
            ) {
                self.history(true);
            }
            if aqua::nav_button(
                ui,
                &theme,
                ">",
                "Forward (Alt+Right)",
                !self.current().forward.is_empty(),
            ) {
                self.history(false);
            }
            if aqua::nav_button(ui, &theme, "Up", "Parent folder (Alt+Up)", true) {
                self.parent();
            }
            let width = if narrow {
                ui.available_width().max(90.0)
            } else {
                (ui.available_width() - 265.0).max(120.0)
            };
            self.breadcrumb_bar(ui, width);
            if !narrow {
                self.navigation_search(ui, false);
            }
        });
        if narrow {
            ui.horizontal(|ui| self.navigation_search(ui, true));
        }
    }

    fn win10_navigation_bar(&mut self, ui: &mut egui::Ui) {
        let narrow = ui.available_width() < 760.0;
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
                .small_button("Up")
                .on_hover_text("Parent folder (Alt+Up)")
                .clicked()
            {
                self.parent();
            }
            let width = if narrow {
                ui.available_width().max(90.0)
            } else {
                (ui.available_width() - 270.0).max(120.0)
            };
            self.breadcrumb_bar(ui, width);
            if !narrow {
                self.navigation_search(ui, false);
            }
        });
        if narrow {
            ui.horizontal(|ui| self.navigation_search(ui, true));
        }
    }

    fn aero_navigation_bar(&mut self, ui: &mut egui::Ui) {
        let narrow = ui.available_width() < 760.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let back = egui::Button::new(egui::RichText::new("<").strong().size(16.0))
                .fill(Color32::from_rgb(223, 244, 252))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(110, 157, 183)))
                .corner_radius(egui::CornerRadius::same(14));
            if ui
                .add_enabled(!self.current().back.is_empty(), back)
                .on_hover_text("Back (Alt+Left)")
                .clicked()
            {
                self.history(true);
            }
            let forward = egui::Button::new(egui::RichText::new(">").strong().size(16.0))
                .fill(Color32::from_rgb(223, 244, 252))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(110, 157, 183)))
                .corner_radius(egui::CornerRadius::same(14));
            if ui
                .add_enabled(!self.current().forward.is_empty(), forward)
                .on_hover_text("Forward (Alt+Right)")
                .clicked()
            {
                self.history(false);
            }
            if ui
                .small_button("Up")
                .on_hover_text("Parent folder (Alt+Up)")
                .clicked()
            {
                self.parent();
            }
            let width = if narrow {
                ui.available_width().max(100.0)
            } else {
                (ui.available_width() - 260.0).max(120.0)
            };
            self.breadcrumb_bar(ui, width);
            if !narrow {
                self.navigation_search(ui, false);
            }
        });
        if narrow {
            ui.horizontal(|ui| self.navigation_search(ui, true));
        }
    }

    fn breadcrumb_bar(&mut self, ui: &mut egui::Ui, width: f32) {
        if self.focus_address {
            self.breadcrumb_edit_address = true;
        }
        let remote = self.current().remote_uri.is_some();
        let win10 = self.theme.shell_kind() == ShellKind::Ribbon;
        let aqua = self.theme.shell_kind() == ShellKind::Aqua;
        let finder_shell = matches!(
            self.theme.shell_kind(),
            ShellKind::Finder | ShellKind::Chromeos
        );
        egui::Frame::new()
            .fill(Color32::WHITE)
            .stroke(egui::Stroke::new(
                1.0,
                if win10 || aqua || finder_shell {
                    self.theme.palette.border()
                } else {
                    Color32::from_rgb(121, 161, 188)
                },
            ))
            .corner_radius(if win10 {
                0
            } else if aqua {
                11
            } else if finder_shell {
                8
            } else {
                2
            })
            .inner_margin(egui::Margin::symmetric(4, 2))
            .show(ui, |ui| {
                ui.set_width((width - 10.0).max(90.0));
                ui.set_min_height(24.0);
                if self.breadcrumb_edit_address || remote {
                    let edit = ui.add_sized(
                        [(width - 17.0).max(78.0), 23.0],
                        egui::TextEdit::singleline(&mut self.current_mut().address)
                            .frame(egui::Frame::NONE)
                            .hint_text("Address (Ctrl+L)"),
                    );
                    if self.focus_address {
                        ui.memory_mut(|memory| memory.request_focus(edit.id));
                        self.focus_address = false;
                    }
                    let enter = ui.input(|input| input.key_pressed(Key::Enter));
                    let escape = ui.input(|input| input.key_pressed(Key::Escape));
                    if edit.lost_focus() || escape {
                        if enter && !escape {
                            let address = self.current().address.clone();
                            self.navigate_address(address);
                        } else {
                            // Editing the address is not navigation until Enter.
                            let address = self
                                .current()
                                .remote_uri
                                .clone()
                                .unwrap_or_else(|| self.current().path.display().to_string());
                            self.current_mut().address = address;
                        }
                        self.breadcrumb_edit_address = false;
                    }
                } else {
                    let crumbs = crate::aero::breadcrumbs(&self.current().path, &config::home());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2((width - 52.0).max(48.0), 23.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                egui::ScrollArea::horizontal()
                                    .id_salt(("address-crumbs", self.theme.id.as_str()))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 1.0;
                                            for crumb in crumbs {
                                                if ui
                                                    .add(
                                                        egui::Button::new(
                                                            egui::RichText::new(format!(
                                                                "{}  >",
                                                                crumb.label
                                                            ))
                                                            .size(12.0)
                                                            .color(if win10 || finder_shell {
                                                                Color32::from_rgb(48, 48, 48)
                                                            } else {
                                                                Color32::from_rgb(39, 71, 98)
                                                            }),
                                                        )
                                                        .frame(false),
                                                    )
                                                    .on_hover_text(crumb.path.display().to_string())
                                                    .clicked()
                                                {
                                                    self.navigate(crumb.path);
                                                }
                                            }
                                        });
                                    });
                            },
                        );
                        if ui
                            .small_button("...")
                            .on_hover_text("Type a path (Ctrl+L)")
                            .clicked()
                        {
                            self.breadcrumb_edit_address = true;
                            self.focus_address = true;
                        }
                    });
                }
            });
    }

    fn aero_command_bar(&mut self, ui: &mut egui::Ui) {
        let local = self.current().remote_uri.is_none();
        let has_selection = local && !self.current().selected.is_empty();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            ui.menu_button("Organize  v", |ui| {
                if ui.add_enabled(local, egui::Button::new("New folder  Ctrl+N")).clicked() {
                    self.file_command(FileCommand::NewFolder);
                    ui.close();
                }
                ui.separator();
                for (label, enabled, command) in [
                    ("Cut  Ctrl+X", has_selection, FileCommand::Cut),
                    ("Copy  Ctrl+C", has_selection, FileCommand::Copy),
                    ("Paste  Ctrl+V", local, FileCommand::Paste),
                    ("Rename  F2", has_selection, FileCommand::Rename),
                    ("Move to Trash  Delete", has_selection, FileCommand::Trash),
                    ("Properties  Alt+Enter", has_selection, FileCommand::Properties),
                ] {
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        self.file_command(command);
                        ui.close();
                    }
                }
            });
            if ui.add_enabled(has_selection, egui::Button::new("Open")).clicked()
                && let Some(entry) = self.current().entries.iter()
                    .find(|entry| self.current().selected.contains(&entry.path)).cloned()
            {
                self.activate(&entry);
            }
            ui.menu_button("Share  v", |ui| {
                if ui.add_enabled(has_selection, egui::Button::new("Copy file path"))
                    .on_hover_text("Copy the selected Linux path for sharing")
                    .clicked()
                {
                    self.copy_selected_paths(ui.ctx());
                    ui.close();
                }
                if ui.add_enabled(has_selection, egui::Button::new("Compress selected…"))
                    .on_hover_text("Create a local archive without overwriting existing files")
                    .clicked()
                {
                    self.open_archive_create();
                    ui.close();
                }
            });
            if ui.add_enabled(local, egui::Button::new("New folder")).clicked() {
                self.file_command(FileCommand::NewFolder);
            }
            if ui.add_enabled(local, egui::Button::new("Add to favorites")).clicked() {
                self.toggle_bookmark();
            }
            ui.separator();
            ui.menu_button("Views  v", |ui| {
                for (title, view) in [("Details", "list"), ("Large icons", "grid")] {
                    if ui.selectable_label(self.config.view == view, title).clicked() {
                        self.config.view = view.into();
                        self.dirty_config = true;
                        ui.close();
                    }
                }
                ui.separator();
                if ui.checkbox(&mut self.config.show_sidebar, "Navigation pane").changed() {
                    self.dirty_config = true;
                }
                if ui.button("Toggle split view  F3").clicked() {
                    self.toggle_split_view();
                    ui.close();
                }
            });
            if ui.add_enabled(has_selection, egui::Button::new("Preview pane")).clicked()
                && let Some(path) = self.selected_paths().first().cloned()
            {
                if self.preview_path.as_ref() == Some(&path) {
                    self.preview_path = None;
                    self.preview_content = None;
                } else {
                    self.open_preview(path);
                }
            }
            if ui.button(if self.aero_advanced_open { "Hide tools" } else { "More tools" })
                .on_hover_text("Archives, filters, saved searches, session and theme settings")
                .clicked()
            {
                self.aero_advanced_open = !self.aero_advanced_open;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("?").on_hover_text("Windows 7-inspired shell · RiceField keyboard shortcuts in --help").clicked() {
                    self.status = Some(("RiceField: Ctrl+L address · Ctrl+F search · Ctrl+T new tab · F3 split · Alt+Enter properties".into(), false));
                }
            });
        });
    }

    fn luna_navigation_bar(&mut self, ui: &mut egui::Ui) {
        use shell_components::{LunaPanel as Panel, luna_panel_open};
        if ui.available_width() < 560.0 {
            // Two complete rows; never leave half a toolbar icon beyond the
            // narrow edge when egui's horizontal wrap meets a nested menu.
            ui.horizontal(|ui| self.luna_history_buttons(ui));
            ui.horizontal(|ui| self.luna_search_and_view_buttons(ui));
        } else {
            ui.horizontal(|ui| {
                self.luna_history_buttons(ui);
                ui.separator();
                self.luna_search_and_view_buttons(ui);
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Address").size(12.0));
            let width = (ui.available_width() - 51.0).max(90.0);
            let address = ui.add_sized(
                [width, 24.0],
                egui::TextEdit::singleline(&mut self.current_mut().address)
                    .hint_text("Location (Ctrl+L)"),
            );
            if self.focus_address {
                ui.memory_mut(|memory| memory.request_focus(address.id));
                self.focus_address = false;
            }
            if (address.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
                || ui
                    .button(egui::RichText::new("Go").color(Color32::from_rgb(29, 113, 36)))
                    .clicked()
            {
                let value = self.current().address.clone();
                self.navigate_address(value);
            }
        });
        if luna_panel_open(ui.ctx(), Panel::Search) {
            ui.horizontal_wrapped(|ui| {
                ui.label("Search");
                self.navigation_search(ui, ui.available_width() < 800.0);
            });
        }
    }

    fn luna_history_buttons(&mut self, ui: &mut egui::Ui) {
        use shell_components::{LunaToolbarIcon as Icon, luna_toolbar_button};
        ui.spacing_mut().item_spacing.x = 2.0;
        if luna_toolbar_button(ui, "Back", Icon::Back, !self.current().back.is_empty()) {
            self.history(true);
        }
        if luna_toolbar_button(
            ui,
            "Forward",
            Icon::Forward,
            !self.current().forward.is_empty(),
        ) {
            self.history(false);
        }
        if luna_toolbar_button(ui, "Up", Icon::Up, true) {
            self.parent();
        }
    }

    fn luna_search_and_view_buttons(&mut self, ui: &mut egui::Ui) {
        use shell_components::{
            LunaPanel as Panel, LunaToolbarIcon as Icon, luna_panel_open, luna_toggle_panel,
            luna_toolbar_button,
        };
        ui.spacing_mut().item_spacing.x = 2.0;
        if luna_toolbar_button(ui, "Search", Icon::Search, true) {
            luna_toggle_panel(ui.ctx(), Panel::Search);
            self.focus_search = luna_panel_open(ui.ctx(), Panel::Search);
        }
        if luna_toolbar_button(ui, "Folders", Icon::Folders, true) {
            luna_toggle_panel(ui.ctx(), Panel::FolderTree);
            self.config.show_sidebar = true;
            self.dirty_config = true;
        }
        ui.separator();
        ui.menu_button("Views", |ui| {
            for (label, view) in [("Icons", "grid"), ("Details", "list")] {
                if ui
                    .selectable_label(self.config.view == view, label)
                    .clicked()
                {
                    self.config.view = view.to_owned();
                    self.dirty_config = true;
                    ui.close();
                }
            }
        });
        if ui
            .small_button("Refresh")
            .on_hover_text("Refresh (Ctrl+R)")
            .clicked()
        {
            self.scan(self.active);
        }
    }

    fn navigation_buttons(&mut self, ui: &mut egui::Ui) {
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
    }

    fn navigation_address(&mut self, ui: &mut egui::Ui, compact: bool) {
        let address_width = if compact {
            (ui.available_width() - 92.0).max(90.0)
        } else {
            (ui.available_width() - 380.0).max(90.0)
        };
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
            self.navigate_address(value);
        }
        if ui
            .button("Reload")
            .on_hover_text("Refresh (Ctrl+R)")
            .clicked()
        {
            self.scan(self.active);
        }
    }

    fn navigation_search(&mut self, ui: &mut egui::Ui, compact: bool) {
        let aero = matches!(
            self.theme.shell_kind(),
            ShellKind::Aero
                | ShellKind::Ribbon
                | ShellKind::Aqua
                | ShellKind::Finder
                | ShellKind::Chromeos
                | ShellKind::Classic
        );
        let search_width = if aero {
            if compact {
                (ui.available_width() - 112.0).max(90.0)
            } else {
                158.0
            }
        } else if compact {
            (ui.available_width() - 220.0).max(80.0)
        } else if ui.available_width() < 300.0 {
            90.0
        } else {
            170.0
        };
        let remote = self.current().remote_uri.is_some();
        let search = ui.add_enabled(
            true,
            egui::TextEdit::singleline(&mut self.current_mut().query)
                .desired_width(search_width)
                .hint_text(if aero {
                    "Search this folder"
                } else {
                    "Search here… (Ctrl+F)"
                }),
        );
        if self.focus_search {
            ui.memory_mut(|memory| memory.request_focus(search.id));
            self.focus_search = false;
        }
        if search.changed() {
            self.current_mut().selected.clear();
            if remote || self.current().recursive_search || self.current().content_search {
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
            if remote || self.current().recursive_search || self.current().content_search {
                self.scan(self.active);
            } else {
                self.rebuild_visible(self.active);
            }
            ui.memory_mut(|memory| memory.request_focus(search.id));
        }
        if aero {
            ui.menu_button("Options", |ui| {
                self.search_scope_buttons(ui);
            });
        } else {
            self.search_scope_buttons(ui);
        }
    }

    fn search_scope_buttons(&mut self, ui: &mut egui::Ui) {
        let remote = self.current().remote_uri.is_some();
        if ui
            .add_enabled(
                true,
                egui::Button::new("Subfolders").selected(self.current().recursive_search),
            )
            .on_hover_text(
                "Search the current folder and its subfolders (same filesystem for content search)",
            )
            .clicked()
        {
            let next = !self.current().recursive_search;
            self.current_mut().recursive_search = next;
            self.current_mut().search_pending = None;
            self.scan(self.active);
        }
        if ui
            .add_enabled(
                !remote,
                egui::Button::new("Contents").selected(self.current().content_search),
            )
            .on_hover_text(if remote { "Network content search is disabled; filenames are read-only" } else { "Also search local UTF-8 file contents (up to 2 MiB/file, 4,096 files, 64 MiB/12 s total); no background index" })
            .clicked()
        {
            let next = !self.current().content_search;
            self.current_mut().content_search = next;
            self.current_mut().search_pending = None;
            self.scan(self.active);
        }
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
            .default_size(
                theme
                    .sidebar_width()
                    .min((ui.available_width() * 0.36).max(126.0)),
            )
            .min_size(120.0)
            // `default_size` only affects the initial frame. Cap an already
            // open/resized sidebar as the application window shrinks, too.
            .max_size((ui.available_width() * 0.42).max(120.0))
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.sidebar())
                    .inner_margin(
                        if matches!(
                            theme.shell_kind(),
                            ShellKind::Ribbon
                                | ShellKind::Aqua
                                | ShellKind::Finder
                                | ShellKind::Chromeos
                                | ShellKind::Classic
                        ) {
                            egui::Margin::symmetric(5, 8)
                        } else {
                            egui::Margin::same(10)
                        },
                    ),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if theme.shell_kind() == ShellKind::Luna
                        && shell_components::luna_panel_open(
                            ui.ctx(),
                            shell_components::LunaPanel::FolderTree,
                        )
                    {
                        ui.label(
                            egui::RichText::new("Folders")
                                .strong()
                                .color(theme.palette.accent()),
                        );
                        ui.separator();
                        self.folder_tree_root(ui);
                        return;
                    }
                    let sections = theme.sidebar_sections();
                    for (index, section) in sections.into_iter().enumerate() {
                        if index > 0 {
                            if matches!(
                                theme.shell_kind(),
                                ShellKind::Luna
                                    | ShellKind::Aero
                                    | ShellKind::Ribbon
                                    | ShellKind::Aqua
                                    | ShellKind::Finder
                                    | ShellKind::Chromeos
                            ) {
                                ui.add_space(9.0);
                            } else {
                                ui.add_space(14.0);
                                ui.separator();
                            }
                        }
                        let mut render_section = |ui: &mut egui::Ui| match section {
                            SidebarSection::Tasks => self.sidebar_tasks(ui),
                            SidebarSection::Places => {
                                if theme.shell_kind() == ShellKind::Chromeos {
                                    let home = config::home();
                                    if chromeos::sidebar_link(
                                        ui,
                                        &theme,
                                        "My files",
                                        self.current().path == home,
                                        FilesNavIcon::Home,
                                    ) {
                                        self.navigate(home.clone());
                                    }
                                    for (name, icon) in [
                                        ("Downloads", FilesNavIcon::Download),
                                        ("Documents", FilesNavIcon::Folder),
                                        ("Pictures", FilesNavIcon::Image),
                                        ("Music", FilesNavIcon::Music),
                                        ("Videos", FilesNavIcon::Video),
                                        ("Desktop", FilesNavIcon::Folder),
                                    ] {
                                        let path = home.join(name);
                                        if self
                                            .places
                                            .iter()
                                            .any(|(_, available)| available == &path)
                                            && chromeos::sidebar_link(
                                                ui,
                                                &theme,
                                                name,
                                                self.current().path == path,
                                                icon,
                                            )
                                        {
                                            self.navigate(path);
                                        }
                                    }
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Classic {
                                    for (label, path) in self.places.clone() {
                                        let icon = if path == Path::new("/") {
                                            TreeIcon::Disk
                                        } else if path == config::home() {
                                            TreeIcon::Home
                                        } else {
                                            TreeIcon::Folder
                                        };
                                        let response = classic98::tree_link(
                                            ui,
                                            label,
                                            self.current().path == path,
                                            icon,
                                        );
                                        if classic98::activated(ui, &response) {
                                            self.navigate(path);
                                        }
                                    }
                                    ui.collapsing("Browse folders", |ui| self.folder_tree_root(ui));
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Finder {
                                    for (name, path) in self.places.clone() {
                                        let icon = if path == Path::new("/") {
                                            FinderNavIcon::Computer
                                        } else {
                                            FinderNavIcon::Home
                                        };
                                        if finder::sidebar_link(
                                            ui,
                                            &theme,
                                            name,
                                            self.current().path == path,
                                            icon,
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    ui.collapsing("Folder tree", |ui| self.folder_tree_root(ui));
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Aqua {
                                    for (name, path) in self.places.clone() {
                                        let label = if path == Path::new("/") {
                                            "Filesystem"
                                        } else {
                                            name
                                        };
                                        let icon = if path == Path::new("/") {
                                            aqua::AquaNavIcon::Computer
                                        } else if path == config::home() {
                                            aqua::AquaNavIcon::Home
                                        } else {
                                            aqua::AquaNavIcon::Folder
                                        };
                                        if aqua::sidebar_link(
                                            ui,
                                            &theme,
                                            label,
                                            self.current().path == path,
                                            icon,
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    ui.add_space(5.0);
                                    ui.collapsing("Browse folders", |ui| self.folder_tree_root(ui));
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Ribbon {
                                    for (name, path) in self.places.clone() {
                                        let label = if path == Path::new("/") {
                                            "Linux filesystem"
                                        } else {
                                            name
                                        };
                                        if shell_components::win10_sidebar_link(
                                            ui,
                                            &theme,
                                            label,
                                            self.current().path == path,
                                            if path == Path::new("/") {
                                                shell_components::Win10NavIcon::Computer
                                            } else {
                                                shell_components::Win10NavIcon::Folder
                                            },
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    ui.collapsing("Folders", |ui| self.folder_tree_root(ui));
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Luna {
                                    use shell_components::{LunaLinkIcon as Icon, luna_link};
                                    for (label, path) in self.places.clone() {
                                        let title = if path == Path::new("/") {
                                            "My Computer"
                                        } else {
                                            label
                                        };
                                        if luna_link(
                                            ui,
                                            title,
                                            true,
                                            if path == Path::new("/") {
                                                Icon::Computer
                                            } else {
                                                Icon::Folder
                                            },
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    if luna_link(ui, "Network Places", true, Icon::Computer) {
                                        self.discovery_open = true;
                                        if self.discovery_hosts.is_empty() {
                                            self.start_discovery();
                                        }
                                    }
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Aero {
                                    for (name, path) in self.places.clone() {
                                        if !matches!(
                                            name,
                                            "Documents" | "Music" | "Pictures" | "Videos"
                                        ) {
                                            continue;
                                        }
                                        if shell_components::aero_sidebar_link(
                                            ui,
                                            name,
                                            self.current().path == path,
                                            shell_components::AeroNavIcon::Library,
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    // The folder tree stays available for directories that
                                    // are not one of the conventional user libraries.
                                    ui.add_space(4.0);
                                    ui.collapsing("Folders", |ui| self.folder_tree_root(ui));
                                    return;
                                }
                                ui.add_space(8.0);
                                for (label, path) in self.places.clone() {
                                    if ui
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
                                ui.separator();
                                ui.strong("FOLDER TREE");
                                self.folder_tree_root(ui);
                            }
                            SidebarSection::Favorites => {
                                if theme.shell_kind() == ShellKind::Chromeos {
                                    for path in self.config.bookmarks.clone() {
                                        let label =
                                            path.file_name().unwrap_or_default().to_string_lossy();
                                        if chromeos::sidebar_link(
                                            ui,
                                            &theme,
                                            &shorten(&label, 22),
                                            self.current().path == path,
                                            FilesNavIcon::Shortcut,
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    if self.current().remote_uri.is_none()
                                        && ui
                                            .small_button(
                                                if self
                                                    .config
                                                    .bookmarks
                                                    .contains(&self.current().path)
                                                {
                                                    "− Remove shortcut"
                                                } else {
                                                    "+ Add shortcut"
                                                },
                                            )
                                            .clicked()
                                    {
                                        self.toggle_bookmark();
                                    }
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Classic {
                                    for path in self.config.bookmarks.clone() {
                                        let label =
                                            path.file_name().unwrap_or_default().to_string_lossy();
                                        let response = classic98::tree_link(
                                            ui,
                                            &shorten(&label, 23),
                                            self.current().path == path,
                                            TreeIcon::Favorite,
                                        );
                                        if classic98::activated(ui, &response) {
                                            self.navigate(path);
                                        }
                                    }
                                    if self.current().remote_uri.is_none()
                                        && ui.small_button("Add to Favorites").clicked()
                                    {
                                        self.toggle_bookmark();
                                    }
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Finder {
                                    let home = config::home();
                                    if finder::sidebar_link(
                                        ui,
                                        &theme,
                                        "Home",
                                        self.current().path == home,
                                        FinderNavIcon::Home,
                                    ) {
                                        self.navigate(home.clone());
                                    }
                                    for (label, icon) in [
                                        ("Desktop", FinderNavIcon::Folder),
                                        ("Documents", FinderNavIcon::Folder),
                                        ("Downloads", FinderNavIcon::Downloads),
                                        ("Pictures", FinderNavIcon::Pictures),
                                        ("Music", FinderNavIcon::Music),
                                        ("Videos", FinderNavIcon::Folder),
                                    ] {
                                        let path = home.join(label);
                                        if self
                                            .places
                                            .iter()
                                            .any(|(_, available)| available == &path)
                                            && finder::sidebar_link(
                                                ui,
                                                &theme,
                                                label,
                                                self.current().path == path,
                                                icon,
                                            )
                                        {
                                            self.navigate(path);
                                        }
                                    }
                                    for path in self.config.bookmarks.clone() {
                                        if path == home {
                                            continue;
                                        }
                                        let name = path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned();
                                        if finder::sidebar_link(
                                            ui,
                                            &theme,
                                            &shorten(&name, 22),
                                            self.current().path == path,
                                            FinderNavIcon::Favorite,
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    if self.current().remote_uri.is_none()
                                        && ui
                                            .small_button(
                                                if self
                                                    .config
                                                    .bookmarks
                                                    .contains(&self.current().path)
                                                {
                                                    "Remove from Favorites"
                                                } else {
                                                    "+ Add to Favorites"
                                                },
                                            )
                                            .clicked()
                                    {
                                        self.toggle_bookmark();
                                    }
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Aqua {
                                    let home = config::home();
                                    if aqua::sidebar_link(
                                        ui,
                                        &theme,
                                        "Home",
                                        self.current().path == home,
                                        aqua::AquaNavIcon::Home,
                                    ) {
                                        self.navigate(home);
                                    }
                                    for path in self.config.bookmarks.clone() {
                                        let label = path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned();
                                        if aqua::sidebar_link(
                                            ui,
                                            &theme,
                                            &label,
                                            self.current().path == path,
                                            aqua::AquaNavIcon::Favorite,
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    let pinned =
                                        self.config.bookmarks.contains(&self.current().path);
                                    if ui
                                        .small_button(if pinned {
                                            "Unpin this place"
                                        } else {
                                            "+ Pin this place"
                                        })
                                        .clicked()
                                    {
                                        self.toggle_bookmark();
                                    }
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Ribbon {
                                    let home = config::home();
                                    if shell_components::win10_sidebar_link(
                                        ui,
                                        &theme,
                                        "Home",
                                        self.current().path == home,
                                        shell_components::Win10NavIcon::QuickAccess,
                                    ) {
                                        self.navigate(home.clone());
                                    }
                                    for name in ["Desktop", "Downloads", "Documents", "Pictures"] {
                                        let path = home.join(name);
                                        if self.places.iter().any(|(_, existing)| *existing == path)
                                            && shell_components::win10_sidebar_link(
                                                ui,
                                                &theme,
                                                name,
                                                self.current().path == path,
                                                shell_components::Win10NavIcon::Folder,
                                            )
                                        {
                                            self.navigate(path);
                                        }
                                    }
                                    for path in self.config.bookmarks.clone() {
                                        let label = path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned();
                                        if shell_components::win10_sidebar_link(
                                            ui,
                                            &theme,
                                            &label,
                                            self.current().path == path,
                                            shell_components::Win10NavIcon::QuickAccess,
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    if ui.small_button("Pin current folder").clicked() {
                                        self.toggle_bookmark();
                                    }
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Luna {
                                    use shell_components::{LunaLinkIcon as Icon, luna_link};
                                    let selected = self.selected_paths();
                                    if let Some(path) = selected.first() {
                                        ui.label(
                                            egui::RichText::new(
                                                path.file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy(),
                                            )
                                            .small()
                                            .strong(),
                                        );
                                        ui.small("Selected item");
                                    } else {
                                        let count = self.current().visible.len();
                                        ui.small(format!("{count} items in this folder"));
                                    }
                                    if luna_link(
                                        ui,
                                        "Add folder to favorites",
                                        true,
                                        Icon::Bookmark,
                                    ) {
                                        self.toggle_bookmark();
                                    }
                                    for path in self.config.bookmarks.clone() {
                                        let label =
                                            path.file_name().unwrap_or_default().to_string_lossy();
                                        if luna_link(ui, &label, true, Icon::Folder) {
                                            self.navigate(path);
                                        }
                                    }
                                    return;
                                }
                                if theme.shell_kind() == ShellKind::Aero {
                                    let home = config::home();
                                    if shell_components::aero_sidebar_link(
                                        ui,
                                        "Home",
                                        self.current().path == home,
                                        shell_components::AeroNavIcon::Favorite,
                                    ) {
                                        self.navigate(home.clone());
                                    }
                                    for name in ["Desktop", "Downloads"] {
                                        let path = home.join(name);
                                        if self.places.iter().any(|(_, existing)| *existing == path)
                                            && shell_components::aero_sidebar_link(
                                                ui,
                                                name,
                                                self.current().path == path,
                                                shell_components::AeroNavIcon::Favorite,
                                            )
                                        {
                                            self.navigate(path);
                                        }
                                    }
                                    for path in self.config.bookmarks.clone() {
                                        let label = path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned();
                                        if shell_components::aero_sidebar_link(
                                            ui,
                                            &label,
                                            self.current().path == path,
                                            shell_components::AeroNavIcon::Favorite,
                                        ) {
                                            self.navigate(path);
                                        }
                                    }
                                    if ui.small_button("Add current folder").clicked() {
                                        self.toggle_bookmark();
                                    }
                                    return;
                                }
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
                                let luna = theme.shell_kind() == ShellKind::Luna;
                                let aero = theme.shell_kind() == ShellKind::Aero;
                                let ribbon = theme.shell_kind() == ShellKind::Ribbon;
                                let aqua_shell = theme.shell_kind() == ShellKind::Aqua;
                                let finder_shell = theme.shell_kind() == ShellKind::Finder;
                                let chromeos_shell = theme.shell_kind() == ShellKind::Chromeos;
                                let classic_shell = theme.shell_kind() == ShellKind::Classic;
                                let discover = if chromeos_shell {
                                    chromeos::sidebar_link(
                                        ui,
                                        &theme,
                                        "Network",
                                        false,
                                        FilesNavIcon::Network,
                                    )
                                } else if classic_shell {
                                    let response = classic98::tree_link(
                                        ui,
                                        "Network Neighborhood",
                                        false,
                                        TreeIcon::Network,
                                    );
                                    classic98::activated(ui, &response)
                                } else if finder_shell {
                                    finder::sidebar_link(
                                        ui,
                                        &theme,
                                        "Network",
                                        false,
                                        FinderNavIcon::Network,
                                    )
                                } else if aqua_shell {
                                    aqua::sidebar_link(
                                        ui,
                                        &theme,
                                        "Discover network",
                                        false,
                                        aqua::AquaNavIcon::Network,
                                    )
                                } else if ribbon {
                                    shell_components::win10_sidebar_link(
                                        ui,
                                        &theme,
                                        "Discover network",
                                        false,
                                        shell_components::Win10NavIcon::Network,
                                    )
                                } else if aero {
                                    shell_components::aero_sidebar_link(
                                        ui,
                                        "Network · discover",
                                        false,
                                        shell_components::AeroNavIcon::Network,
                                    )
                                } else if luna {
                                    shell_components::luna_link(
                                        ui,
                                        "Discover network storage",
                                        true,
                                        shell_components::LunaLinkIcon::Drive,
                                    )
                                } else {
                                    ui.button("Discover NAS on network…").clicked()
                                };
                                if discover {
                                    self.discovery_open = true;
                                    if self.discovery_hosts.is_empty() {
                                        self.start_discovery();
                                    }
                                }
                                let connect = if chromeos_shell {
                                    !self.volume_busy
                                        && chromeos::sidebar_link(
                                            ui,
                                            &theme,
                                            "Connect to server",
                                            false,
                                            FilesNavIcon::Network,
                                        )
                                } else if classic_shell {
                                    let response = classic98::tree_link(
                                        ui,
                                        "Map network folder",
                                        false,
                                        TreeIcon::Network,
                                    );
                                    !self.volume_busy && classic98::activated(ui, &response)
                                } else if finder_shell {
                                    !self.volume_busy
                                        && finder::sidebar_link(
                                            ui,
                                            &theme,
                                            "Connect to server",
                                            false,
                                            FinderNavIcon::Network,
                                        )
                                } else if aqua_shell {
                                    !self.volume_busy
                                        && aqua::sidebar_link(
                                            ui,
                                            &theme,
                                            "Connect to NAS",
                                            false,
                                            aqua::AquaNavIcon::Network,
                                        )
                                } else if ribbon {
                                    !self.volume_busy
                                        && shell_components::win10_sidebar_link(
                                            ui,
                                            &theme,
                                            "Connect to network",
                                            false,
                                            shell_components::Win10NavIcon::Network,
                                        )
                                } else if aero {
                                    shell_components::aero_sidebar_link(
                                        ui,
                                        "Connect to network…",
                                        false,
                                        shell_components::AeroNavIcon::Network,
                                    )
                                } else if luna {
                                    shell_components::luna_link(
                                        ui,
                                        "Connect to network",
                                        !self.volume_busy,
                                        shell_components::LunaLinkIcon::Computer,
                                    )
                                } else {
                                    ui.add_enabled(
                                        !self.volume_busy,
                                        egui::Button::new("+ Connect to network…"),
                                    )
                                    .clicked()
                                };
                                if connect {
                                    self.open_network_login(String::new());
                                }
                                if self.volume_busy {
                                    ui.small("Device operation in progress…");
                                }
                                for volume in self.device_volumes.clone() {
                                    if !volume.mount_paths.is_empty() {
                                        continue;
                                    }
                                    let title = format!(
                                        "{}: {}",
                                        if volume.removable { "Removable" } else { "USB" },
                                        volume.label
                                    );
                                    let action_label = if volume.encrypted {
                                        if volume.unlocked {
                                            format!("Lock {title}…")
                                        } else {
                                            format!("Unlock {title}…")
                                        }
                                    } else {
                                        format!("Mount {title}")
                                    };
                                    let response = ui
                                        .add_enabled(
                                            !self.volume_busy,
                                            egui::Button::new(action_label),
                                        )
                                        .on_hover_text(format!(
                                            "{} · UDisks2/Polkit removable-device operation",
                                            volume.device
                                        ));
                                    if response.clicked() {
                                        if volume.encrypted {
                                            if volume.unlocked {
                                                self.volume_confirm =
                                                    Some(VolumeAction::LockEncrypted(
                                                        volume.object_path.clone(),
                                                    ));
                                            } else {
                                                volume::clear_password(
                                                    &mut self.volume_unlock_password,
                                                );
                                                self.volume_unlock_target = Some((
                                                    volume.object_path.clone(),
                                                    title.clone(),
                                                ));
                                            }
                                        } else {
                                            self.start_volume_action(
                                                VolumeAction::MountDevice(
                                                    volume.object_path.clone(),
                                                ),
                                                false,
                                            );
                                        }
                                    }
                                    if volume.ejectable || volume.can_power_off {
                                        response.context_menu(|ui| {
                                            if volume.ejectable
                                                && ui
                                                    .add_enabled(
                                                        !self.volume_busy,
                                                        egui::Button::new("Eject drive…"),
                                                    )
                                                    .clicked()
                                            {
                                                self.volume_confirm =
                                                    Some(VolumeAction::EjectDrive(
                                                        volume.drive_path.clone(),
                                                    ));
                                                ui.close();
                                            }
                                            if volume.can_power_off
                                                && ui
                                                    .add_enabled(
                                                        !self.volume_busy,
                                                        egui::Button::new(
                                                            "Safely power off USB drive…",
                                                        ),
                                                    )
                                                    .clicked()
                                            {
                                                self.volume_confirm =
                                                    Some(VolumeAction::PowerOffDrive(
                                                        volume.drive_path.clone(),
                                                    ));
                                                ui.close();
                                            }
                                        });
                                    }
                                }
                                for uri in self.config.remote_favorites.clone() {
                                    if !matches!(
                                        Location::parse(uri.clone().into()),
                                        Ok(Location::Remote(_))
                                    ) {
                                        continue;
                                    }
                                    let remote_clicked = if chromeos_shell {
                                        chromeos::sidebar_link(
                                            ui,
                                            &theme,
                                            &shorten(&uri, 24),
                                            false,
                                            FilesNavIcon::Network,
                                        )
                                    } else if classic_shell {
                                        let response = classic98::tree_link(
                                            ui,
                                            &shorten(&uri, 25),
                                            false,
                                            TreeIcon::Network,
                                        );
                                        classic98::activated(ui, &response)
                                    } else if finder_shell {
                                        finder::sidebar_link(
                                            ui,
                                            &theme,
                                            &shorten(&uri, 24),
                                            false,
                                            FinderNavIcon::Network,
                                        )
                                    } else if aqua_shell {
                                        aqua::sidebar_link(
                                            ui,
                                            &theme,
                                            &shorten(&uri, 24),
                                            false,
                                            aqua::AquaNavIcon::Network,
                                        )
                                    } else {
                                        ui.button(if aero {
                                            format!("    {}", shorten(&uri, 24))
                                        } else {
                                            format!("Network favorite: {}", shorten(&uri, 27))
                                        })
                                        .on_hover_text(&uri)
                                        .clicked()
                                    };
                                    if remote_clicked {
                                        self.navigate_remote(uri);
                                    }
                                }
                                for volume in self.volumes.clone() {
                                    let path = volume.path;
                                    let device = self
                                        .device_volumes
                                        .iter()
                                        .find(|device| {
                                            device
                                                .mount_paths
                                                .iter()
                                                .any(|mounted| mounted == &path)
                                        })
                                        .cloned();
                                    let label = if path == Path::new("/") {
                                        "System disk".to_owned()
                                    } else {
                                        format!(
                                            "{}: {}",
                                            if volume.network { "Network" } else { "Volume" },
                                            path.file_name().unwrap_or_default().to_string_lossy()
                                        )
                                    };
                                    let response = if chromeos_shell {
                                        chromeos::sidebar_response(
                                            ui,
                                            &theme,
                                            &label,
                                            self.current().path == path,
                                            if volume.network {
                                                FilesNavIcon::Network
                                            } else {
                                                FilesNavIcon::Drive
                                            },
                                        )
                                    } else if classic_shell {
                                        classic98::tree_link(
                                            ui,
                                            &label,
                                            self.current().path == path,
                                            if volume.network {
                                                TreeIcon::Network
                                            } else {
                                                TreeIcon::Disk
                                            },
                                        )
                                    } else if finder_shell {
                                        finder::sidebar_response(
                                            ui,
                                            &theme,
                                            &label,
                                            self.current().path == path,
                                            if path == Path::new("/") {
                                                FinderNavIcon::Computer
                                            } else if volume.network {
                                                FinderNavIcon::Network
                                            } else {
                                                FinderNavIcon::Folder
                                            },
                                        )
                                    } else if aqua_shell {
                                        aqua::sidebar_response(
                                            ui,
                                            &theme,
                                            &label,
                                            self.current().path == path,
                                            if path == Path::new("/") {
                                                aqua::AquaNavIcon::Computer
                                            } else if volume.network {
                                                aqua::AquaNavIcon::Network
                                            } else {
                                                aqua::AquaNavIcon::Folder
                                            },
                                        )
                                    } else if ribbon {
                                        shell_components::win10_sidebar_response(
                                            ui,
                                            &theme,
                                            &label,
                                            self.current().path == path,
                                            if path == Path::new("/") {
                                                shell_components::Win10NavIcon::Computer
                                            } else if volume.network {
                                                shell_components::Win10NavIcon::Network
                                            } else {
                                                shell_components::Win10NavIcon::Folder
                                            },
                                        )
                                    } else {
                                        ui.add_sized(
                                            [ui.available_width(), if aero { 26.0 } else { 29.0 }],
                                            egui::Button::new(if aero {
                                                format!("    {label}")
                                            } else {
                                                label
                                            })
                                            .selected(self.current().path == path)
                                            .frame(!aero || self.current().path == path),
                                        )
                                    }
                                    .on_hover_text(format!(
                                        "{}\nRight-click for mount actions",
                                        path.display()
                                    ));
                                    if path != Path::new("/") {
                                        response.context_menu(|ui| {
                                            if ui
                                                .add_enabled(
                                                    !self.volume_busy,
                                                    egui::Button::new("Unmount safely…"),
                                                )
                                                .clicked()
                                            {
                                                self.volume_confirm =
                                                    Some(device.as_ref().map_or_else(
                                                        || VolumeAction::Unmount(path.clone()),
                                                        |device| VolumeAction::UnmountDevice {
                                                            object_path: device.object_path.clone(),
                                                            mount_point: path.clone(),
                                                        },
                                                    ));
                                                ui.close();
                                            }
                                            if ui
                                                .add_enabled(
                                                    !self.volume_busy,
                                                    egui::Button::new("Eject…"),
                                                )
                                                .clicked()
                                            {
                                                self.volume_confirm = Some(
                                                    device
                                                        .as_ref()
                                                        .filter(|device| device.ejectable)
                                                        .map_or_else(
                                                            || VolumeAction::Eject(path.clone()),
                                                            |device| {
                                                                VolumeAction::EjectDrive(
                                                                    device.drive_path.clone(),
                                                                )
                                                            },
                                                        ),
                                                );
                                                ui.close();
                                            }
                                        });
                                    }
                                    let activated = if chromeos_shell {
                                        chromeos::activated(ui, &response)
                                    } else if classic_shell {
                                        classic98::activated(ui, &response)
                                    } else if finder_shell {
                                        finder::sidebar_activated(ui, &response)
                                    } else if aqua_shell {
                                        aqua::sidebar_activated(ui, &response)
                                    } else if ribbon {
                                        shell_components::win10_sidebar_activated(ui, &response)
                                    } else {
                                        response.clicked()
                                    };
                                    if activated {
                                        self.navigate(path);
                                    }
                                }
                            }
                        };
                        shell_components::sidebar_section(
                            ui,
                            &theme,
                            index,
                            section,
                            &mut render_section,
                        );
                    }
                });
            });
    }

    fn folder_tree_root(&mut self, ui: &mut egui::Ui) {
        let home = config::home();
        self.folder_tree_entry(ui, home, 0);
        self.folder_tree_entry(ui, PathBuf::from("/"), 0);
    }

    fn folder_tree_entry(&mut self, ui: &mut egui::Ui, path: PathBuf, depth: usize) {
        if depth > 8 {
            return;
        }
        let expanded = self.tree_expanded.contains(&path);
        let title = if path == Path::new("/") {
            "Filesystem".to_owned()
        } else if depth == 0 {
            "Home".to_owned()
        } else {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 9.0);
            if ui
                // Linux's bundled egui font cannot render the Unicode tree
                // triangles consistently on the older Mint/Xfce targets.
                // Classic +/- affordances are period-correct for Win98 and
                // remain legible and keyboard-operable in every rice.
                .add(
                    egui::Button::new(if expanded { "-" } else { "+" }).min_size(Vec2::splat(24.0)),
                )
                .on_hover_text(if expanded {
                    "Collapse folder"
                } else {
                    "Expand folder"
                })
                .clicked()
            {
                if expanded {
                    self.tree_expanded.remove(&path);
                } else {
                    self.tree_expanded.insert(path.clone());
                    self.folder_tree.request(path.clone(), false);
                }
            }
            let label = ui
                .add(egui::Button::new(shorten(&title, 22)).selected(self.current().path == path));
            if label.on_hover_text(path.display().to_string()).clicked() {
                self.navigate(path.clone());
            }
        });
        if !self.tree_expanded.contains(&path) {
            return;
        }
        match self.folder_tree.get(&path).cloned() {
            Some(Ok(children)) => {
                for child in children {
                    self.folder_tree_entry(ui, child, depth + 1);
                }
            }
            Some(Err(error)) => {
                ui.small(format!("Cannot list: {}", shorten(&error, 60)));
                if ui.small_button("Retry folder").clicked() {
                    self.folder_tree.request(path, true);
                }
            }
            None if self.folder_tree.is_pending(&path) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.small("Loading folders…");
                });
            }
            None => self.folder_tree.request(path, false),
        }
    }

    fn sidebar_tasks(&mut self, ui: &mut egui::Ui) {
        if self.current().remote_uri.is_some() {
            ui.label("Network folder (read-only)");
            return;
        }
        let selection = self.current().selected.len();
        if self.theme.shell_kind() == ShellKind::Luna {
            use shell_components::{LunaLinkIcon as Icon, luna_link};
            if luna_link(ui, "Make a new folder", true, Icon::Folder) {
                self.file_command(FileCommand::NewFolder);
            }
            if luna_link(ui, "Copy selected items", selection > 0, Icon::File) {
                self.file_command(FileCommand::Copy);
            }
            if luna_link(ui, "Move selected items", selection > 0, Icon::Folder) {
                self.file_command(FileCommand::Cut);
            }
            if luna_link(ui, "Paste items here", true, Icon::Folder) {
                self.file_command(FileCommand::Paste);
            }
            if luna_link(ui, "Rename this item", selection > 0, Icon::File) {
                self.file_command(FileCommand::Rename);
            }
            if luna_link(ui, "Move to Trash", selection > 0, Icon::File) {
                self.file_command(FileCommand::Trash);
            }
            if luna_link(ui, "View properties", selection == 1, Icon::File) {
                self.file_command(FileCommand::Properties);
            }
            return;
        }
        ui.add_space(8.0);
        if ui.button("New folder").clicked() {
            self.file_command(FileCommand::NewFolder);
        }
        if ui
            .add_enabled(selection > 0, egui::Button::new("Copy selected"))
            .clicked()
        {
            self.file_command(FileCommand::Copy);
        }
        if ui
            .add_enabled(selection > 0, egui::Button::new("Cut selected"))
            .clicked()
        {
            self.file_command(FileCommand::Cut);
        }
        if ui.button("Paste here").clicked() {
            self.file_command(FileCommand::Paste);
        }
        if ui
            .add_enabled(selection > 0, egui::Button::new("Rename"))
            .clicked()
        {
            self.file_command(FileCommand::Rename);
        }
        if ui
            .add_enabled(selection > 0, egui::Button::new("Move to Trash"))
            .clicked()
        {
            self.file_command(FileCommand::Trash);
        }
        if ui
            .add_enabled(selection == 1, egui::Button::new("Properties"))
            .clicked()
        {
            self.file_command(FileCommand::Properties);
        }
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        if self.preview_quick_look {
            return;
        }
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
                    Self::preview_body(ui, content);
                }
                ui.separator();
                if ui.button("Properties").clicked() {
                    self.open_properties(path.clone());
                }
                if ui.button("Open").clicked() {
                    if self
                        .current()
                        .entries
                        .iter()
                        .any(|entry| entry.path == path && entry.is_dir)
                    {
                        self.navigate(path.clone());
                        self.preview_path = None;
                    } else if let Err(error) = Command::new("xdg-open").arg(&path).spawn() {
                        self.status = Some((format!("Cannot open previewed file: {error}"), true));
                    }
                }
                if ui
                    .add_enabled(
                        self.current()
                            .entries
                            .iter()
                            .any(|entry| entry.path == path && !entry.is_dir),
                        egui::Button::new("Open With…"),
                    )
                    .clicked()
                {
                    self.choose_open_with_path(path.clone());
                }
            });
    }

    fn preview_body(ui: &mut egui::Ui, content: &PreviewContent) {
        match content {
            PreviewContent::Image(texture) => {
                let size = texture.size_vec2();
                let scale = ((ui.available_width() - 8.0) / size.x)
                    .min((ui.available_height() - 12.0) / size.y)
                    .clamp(0.1, 1.0);
                ui.add(egui::Image::new((texture.id(), size)).fit_to_exact_size(size * scale));
            }
            PreviewContent::Text(contents) => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(egui::RichText::new(contents).monospace());
                });
            }
            PreviewContent::Info(contents) => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(contents);
                });
            }
        }
    }

    fn quick_look_window(&mut self, context: &egui::Context) {
        if !self.preview_quick_look {
            return;
        }
        let Some(path) = self.preview_path.clone() else {
            self.preview_quick_look = false;
            return;
        };
        let mut open = true;
        egui::Window::new(format!(
            "Quick Look · {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
        .open(&mut open)
        .default_size([700.0, 500.0])
        .min_size([280.0, 170.0])
        .resizable(true)
        .show(context, |ui| {
            if self.preview_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading preview…");
                });
            } else if let Some(content) = &self.preview_content {
                Self::preview_body(ui, content);
            }
            ui.separator();
            ui.label(egui::RichText::new("Space or Esc to close · read-only preview").small());
        });
        if !open {
            self.preview_quick_look = false;
            self.preview_path = None;
            self.preview_content = None;
        }
    }

    fn main_panel(&mut self, ui: &mut egui::Ui) {
        if self.current().remote_uri.is_some() {
            self.remote_panel(ui);
            return;
        }
        let theme = self.theme.clone();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.panel())
                    .inner_margin(egui::Margin::same(if matches!(theme.shell_kind(), ShellKind::Luna | ShellKind::Aero | ShellKind::Ribbon | ShellKind::Aqua | ShellKind::Finder | ShellKind::Chromeos | ShellKind::Classic) { 5 } else { 14 })),
            )
            .show(ui, |ui| {
                // The XP shell starts with the folder view, not two rows of
                // modern commands. Advanced tools remain available via Tools
                // -> Show RiceField tools, without losing any functionality.
                let show_toolbox = if theme.shell_kind() == ShellKind::Chromeos {
                    self.chromeos_advanced_open
                } else if theme.shell_kind() == ShellKind::Classic {
                    self.classic98_advanced_open
                } else if theme.shell_kind() == ShellKind::Finder {
                    self.finder_advanced_open
                } else if theme.shell_kind() == ShellKind::Aqua {
                    self.aqua_advanced_open
                } else if theme.shell_kind() == ShellKind::Ribbon {
                    self.win10_advanced_open
                } else if theme.shell_kind() == ShellKind::Aero {
                    self.aero_advanced_open
                } else {
                    theme.shell_kind() != ShellKind::Luna
                    || shell_components::luna_panel_open(
                        ui.ctx(),
                        shell_components::LunaPanel::Advanced,
                    )
                };
                if show_toolbox {
                ui.horizontal_wrapped(|ui| {
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
                            .add(egui::Button::new("Split").selected(self.split_indices().is_some()))
                            .on_hover_text("Toggle two-folder split view (F3)")
                            .clicked()
                        {
                            self.toggle_split_view();
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
                ui.horizontal_wrapped(|ui| {
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
                        if ui
                            .add_enabled(
                                self.current().remote_uri.is_none()
                                    && self.current().selected.len() == 1,
                                egui::Button::new("Open With…"),
                            )
                            .clicked()
                        {
                            self.choose_open_with();
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                self.current().remote_uri.is_none()
                                    && self.current().visible.len() >= 2,
                                egui::Button::new("Find duplicate candidates in view…"),
                            )
                            .on_hover_text("Read-only SHA-256 comparison of visible local files; bounded and cancellable, never auto-deletes")
                            .clicked()
                        {
                            self.find_duplicates_in_view();
                            ui.close();
                        }
                    });
                    ui.menu_button("Archive", |ui| {
                        let local = self.current().remote_uri.is_none();
                        let paths = self.selected_paths();
                        let browsable =
                            local && paths.len() == 1 && archive::Format::is_archive(&paths[0]);
                        if ui
                            .add_enabled(browsable, egui::Button::new("Browse / extract archive…"))
                            .clicked()
                        {
                            self.open_archive_browse(paths[0].clone());
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                local && !paths.is_empty(),
                                egui::Button::new("Compress selected…"),
                            )
                            .clicked()
                        {
                            self.open_archive_create();
                            ui.close();
                        }
                        ui.small("Local archives only · bounded extraction · no overwrite");
                    });
                    ui.menu_button("Session", |ui| {
                        if ui.button("Open this folder in a new window  Ctrl+Shift+N").clicked() {
                            self.spawn_window();
                            ui.close();
                        }
                        if ui.button("Toggle split view  F3").clicked() {
                            self.toggle_split_view();
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .checkbox(&mut self.config.restore_session, "Restore tabs on startup")
                            .changed()
                        {
                            self.remember_session();
                            self.dirty_config = true;
                        }
                        ui.small("Restores up to 32 tabs, bounded navigation and 16 closed tabs; no file operations.");
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
                            for option in ["name", "lexical", "type", "size", "modified"] {
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
                    let active_filters = self.config.active_filter_count();
                    egui::containers::menu::MenuButton::new(
                        if active_filters == 0 {
                            "Filters".to_owned()
                        } else {
                            format!("Filters ({active_filters})")
                        },
                    )
                    .config(egui::containers::menu::MenuConfig::new().close_behavior(
                        egui::PopupCloseBehavior::CloseOnClickOutside,
                    ))
                    .ui(ui, |ui| {
                            ui.set_min_width(200.0);
                            let old_filters = (
                                self.config.filter_kind.clone(),
                                self.config.filter_size.clone(),
                                self.config.filter_modified.clone(),
                                self.config.filter_name_glob.clone(),
                                self.config.filter_name_regex.clone(),
                                self.config.filter_min_size_mib,
                                self.config.filter_max_size_mib,
                                self.config.filter_newer_days,
                                self.config.filter_older_days,
                            );
                            ui.label("Type");
                            egui::ComboBox::from_id_salt("filter_kind")
                                .selected_text(
                                    file_view::FILTERS
                                        .iter()
                                        .find(|(key, _)| *key == self.config.filter_kind)
                                        .map_or("All items", |(_, label)| *label),
                                )
                                .show_ui(ui, |ui| {
                                    for &(key, label) in file_view::FILTERS {
                                        ui.selectable_value(
                                            &mut self.config.filter_kind,
                                            key.into(),
                                            label,
                                        );
                                    }
                                });
                            ui.label("Size · files only");
                            egui::ComboBox::from_id_salt("filter_size")
                                .selected_text(
                                    file_view::SIZE_FILTERS
                                        .iter()
                                        .find(|(key, _)| *key == self.config.filter_size)
                                        .map_or("Any size", |(_, label)| *label),
                                )
                                .show_ui(ui, |ui| {
                                    for &(key, label) in file_view::SIZE_FILTERS {
                                        ui.selectable_value(
                                            &mut self.config.filter_size,
                                            key.into(),
                                            label,
                                        );
                                    }
                                });
                            ui.label("Modified");
                            egui::ComboBox::from_id_salt("filter_modified")
                                .selected_text(
                                    file_view::MODIFIED_FILTERS
                                        .iter()
                                        .find(|(key, _)| *key == self.config.filter_modified)
                                        .map_or("Any date", |(_, label)| *label),
                                )
                                .show_ui(ui, |ui| {
                                    for &(key, label) in file_view::MODIFIED_FILTERS {
                                        ui.selectable_value(
                                            &mut self.config.filter_modified,
                                            key.into(),
                                            label,
                                        );
                                    }
                                });
                            ui.collapsing("Custom pattern and ranges", |ui| {
                                ui.label("Filename glob (* = any, ? = one character)");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.config.filter_name_glob)
                                        .char_limit(120)
                                        .desired_width(230.0)
                                        .hint_text("e.g. report-*.pdf"),
                                );
                                ui.label("Filename regex (optional, case insensitive)");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.config.filter_name_regex)
                                        .char_limit(120)
                                        .desired_width(230.0)
                                        .hint_text(r"e.g. ^report-\d+\.pdf$"),
                                );
                                if let Err(error) = file_view::filename_regex(&self.config.filter_name_regex) {
                                    ui.colored_label(ui.visuals().error_fg_color, error);
                                }
                                ui.horizontal(|ui| {
                                    let mut active = self.config.filter_min_size_mib.is_some();
                                    if ui.checkbox(&mut active, "Minimum MiB").changed() {
                                        self.config.filter_min_size_mib = active.then_some(0);
                                    }
                                    if let Some(limit) = self.config.filter_min_size_mib.as_mut() {
                                        ui.add(egui::DragValue::new(limit).range(0..=u64::MAX));
                                    }
                                });
                                ui.horizontal(|ui| {
                                    let mut active = self.config.filter_max_size_mib.is_some();
                                    if ui.checkbox(&mut active, "Maximum MiB").changed() {
                                        self.config.filter_max_size_mib = active.then_some(100);
                                    }
                                    if let Some(limit) = self.config.filter_max_size_mib.as_mut() {
                                        ui.add(egui::DragValue::new(limit).range(0..=u64::MAX));
                                    }
                                });
                                ui.horizontal(|ui| {
                                    let mut active = self.config.filter_newer_days.is_some();
                                    if ui.checkbox(&mut active, "Within past days").changed() {
                                        self.config.filter_newer_days = active.then_some(30);
                                    }
                                    if let Some(limit) = self.config.filter_newer_days.as_mut() {
                                        ui.add(egui::DragValue::new(limit).range(0..=365_000));
                                    }
                                });
                                ui.horizontal(|ui| {
                                    let mut active = self.config.filter_older_days.is_some();
                                    if ui.checkbox(&mut active, "At least days old").changed() {
                                        self.config.filter_older_days = active.then_some(7);
                                    }
                                    if let Some(limit) = self.config.filter_older_days.as_mut() {
                                        ui.add(egui::DragValue::new(limit).range(0..=365_000));
                                    }
                                });
                                if self.config.filter_min_size_mib.zip(self.config.filter_max_size_mib)
                                    .is_some_and(|(min, max)| min > max)
                                    || self.config.filter_older_days.zip(self.config.filter_newer_days)
                                        .is_some_and(|(older, newer)| older > newer)
                                {
                                    ui.colored_label(egui::Color32::from_rgb(176, 72, 32), "Conflicting bounds: no files will match");
                                }
                            });
                            if active_filters > 0 && ui.button("Clear filters").clicked() {
                                self.config.clear_filters();
                            }
                            if old_filters != (
                                self.config.filter_kind.clone(),
                                self.config.filter_size.clone(),
                                self.config.filter_modified.clone(),
                                self.config.filter_name_glob.clone(),
                                self.config.filter_name_regex.clone(),
                                self.config.filter_min_size_mib,
                                self.config.filter_max_size_mib,
                                self.config.filter_newer_days,
                                self.config.filter_older_days,
                            ) {
                                self.dirty_config = true;
                                for index in 0..self.tabs.len() {
                                    self.rebuild_visible(index);
                                }
                            }
                        },
                    );
                    ui.menu_button("Saved searches", |ui| {
                        ui.set_min_width(240.0);
                        let current = self.current();
                        let can_save = current.remote_uri.is_none()
                            && !current.query.trim().is_empty()
                            && current.query.len() <= 120
                            && current.path.to_str().is_some();
                        if ui
                            .add_enabled(can_save, egui::Button::new("Save current local search"))
                            .on_hover_text("Explicitly save this folder, query and scope in your private local settings")
                            .clicked()
                        {
                            let tab = self.current();
                            let record = config::SavedSearch {
                                folder: tab.path.to_string_lossy().into_owned(),
                                query: tab.query.clone(),
                                subfolders: tab.recursive_search,
                                contents: tab.content_search,
                            };
                            if self.config.save_search(record) {
                                self.dirty_config = true;
                                self.status = Some(("Saved search to private preferences".into(), false));
                            } else {
                                self.status = Some(("Search is already saved or invalid".into(), false));
                            }
                            ui.close();
                        }
                        ui.separator();
                        let saved_searches = self.config.saved_searches();
                        if saved_searches.is_empty() {
                            ui.small("No saved searches yet");
                        }
                        for saved in saved_searches {
                            ui.horizontal(|ui| {
                                let label = format!(
                                    "{}  ·  {}{}",
                                    saved.query.chars().take(32).collect::<String>(),
                                    if saved.subfolders { "↳" } else { "·" },
                                    if saved.contents { " text" } else { " names" },
                                );
                                if ui
                                    .button(label)
                                    .on_hover_text(format!("{}\n{}", saved.query, saved.folder))
                                    .clicked()
                                {
                                    self.navigate(PathBuf::from(&saved.folder));
                                    let tab = self.current_mut();
                                    tab.query = saved.query.clone();
                                    tab.recursive_search = saved.subfolders;
                                    tab.content_search = saved.contents;
                                    self.scan(self.active);
                                    ui.close();
                                }
                                if ui.small_button("Remove").clicked() {
                                    self.config.saved_searches.retain(|entry| entry != &saved);
                                    self.dirty_config = true;
                                    ui.close();
                                }
                            });
                        }
                    });
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
                        self.toggle_hidden();
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
                                ui.separator();
                                if ui.button("Customize theme…").clicked() {
                                    self.dialog = Some(Dialog::ThemeEditor(Box::new(ThemeEditor::new(
                                        &self.theme,
                                        self.theme_file.as_deref(),
                                    ))));
                                    ui.close();
                                }
                                if self.theme_file.is_some()
                                    && ui.button("Reload external TOML").clicked()
                                {
                                    self.reload_theme_file(ui.ctx(), true);
                                    ui.close();
                                }
                            });
                    });
                });
                ui.separator();
                }

                if let Some((left, right)) = self.split_indices() {
                    let pane_ids = [left, right];
                    ui.columns(2, |columns| {
                        for (column, index) in columns.iter_mut().zip(pane_ids) {
                            let folder = self.tabs[index]
                                .path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "/".to_owned());
                            column.horizontal(|ui| {
                                if ui
                                    .add(egui::Button::new(if self.active == index {
                                        "Focused"
                                    } else {
                                        "Focus"
                                    }).selected(self.active == index))
                                    .on_hover_text("Make this folder the target for toolbar and keyboard actions")
                                    .clicked()
                                {
                                    self.tab_command(TabCommand::Activate(index));
                                }
                                ui.strong(shorten(&folder, 28));
                            });
                            column.separator();
                            self.file_area(column, index);
                        }
                    });
                } else {
                    self.file_area(ui, self.active);
                }
            });
    }

    fn file_area(&mut self, ui: &mut egui::Ui, index: usize) {
        let theme = self.theme.clone();
        self.drop_areas.push(DropArea {
            tab_id: self.tabs[index].id,
            destination: self.tabs[index].path.clone(),
            rect: ui.available_rect_before_wrap(),
            folder: false,
        });
        if let Some(warning) = self.tabs[index].scan_warning.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(168, 101, 14), warning);
                if ui.button("Retry listing").clicked() {
                    self.scan(index);
                }
            });
        }
        if let Some(error) = self.tabs[index].error.as_ref() {
            ui.colored_label(
                Color32::from_rgb(177, 43, 37),
                format!("Cannot open folder: {error}"),
            );
            if ui.button("Retry").clicked() {
                self.scan(index);
            }
        } else if self.tabs[index].loading && self.tabs[index].entries.is_empty() {
            ui.add(egui::Spinner::new());
            ui.label("Loading folder…");
        } else if self.tabs[index].visible.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(64.0);
                let empty_text = if !self.tabs[index].query.is_empty()
                    || self.config.active_filter_count() > 0
                {
                    "No matching files"
                } else {
                    "This folder is empty"
                };
                ui.label(egui::RichText::new(empty_text).size(18.0));
                ui.label(
                    egui::RichText::new("Try another folder or clear the search.")
                        .color(theme.palette.muted()),
                );
                if self.config.active_filter_count() > 0 && ui.button("Clear filters").clicked() {
                    self.config.clear_filters();
                    self.dirty_config = true;
                    for tab in 0..self.tabs.len() {
                        self.rebuild_visible(tab);
                    }
                }
            });
        } else {
            self.file_listing(ui, index);
        }
    }

    fn remote_panel(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let remote_webdav = self.current().remote_uri.as_ref().is_some_and(|uri| {
            matches!(Location::parse(uri.clone().into()),
                Ok(Location::Remote(location)) if matches!(location.scheme,
                    crate::location::RemoteScheme::Dav | crate::location::RemoteScheme::Davs))
        });
        let title = match theme.shell_kind() {
            ShellKind::Classic => "Network Neighborhood",
            ShellKind::Luna => "My Network Places",
            ShellKind::Aero => "Network",
            ShellKind::Ribbon => "Network location",
            ShellKind::Aqua => "Connected world",
            ShellKind::Chromeos => "Network storage",
            ShellKind::Finder => "Shared",
            ShellKind::Modern => "Network files",
        };
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.panel())
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(theme.palette.chrome())
                    .stroke(egui::Stroke::new(1.0, theme.palette.border()))
                    .corner_radius(theme.metrics.rounding)
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.heading(title);
                            ui.label(
                                egui::RichText::new(if remote_webdav {
                                    "READ-ONLY BROWSE · CONDITIONAL UPLOAD"
                                } else {
                                    "READ ONLY"
                                })
                                    .color(theme.palette.muted())
                                    .small(),
                            )
                        });
                        ui.label(
                            egui::RichText::new(
                                self.current().remote_uri.as_deref().unwrap_or_default(),
                            )
                            .monospace()
                            .color(theme.palette.text()),
                        );
                    });
                ui.add_space(5.0);
                let mut jump = None;
                let breadcrumbs = remote_breadcrumbs(
                    self.current().remote_uri.as_deref().unwrap_or_default(),
                );
                ui.horizontal_wrapped(|ui| {
                    for (index, (label, target)) in breadcrumbs.iter().enumerate() {
                        if index != 0 {
                            ui.label("›");
                        }
                        let active = index + 1 == breadcrumbs.len();
                        if ui
                            .add_enabled(!active, egui::Button::new(shorten(label, 24)))
                            .on_hover_text(target)
                            .clicked()
                        {
                            jump = Some(target.clone());
                        }
                    }
                });
                if let Some(uri) = jump {
                    self.navigate_remote(uri);
                    return;
                }
                if self.remote_download_busy {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        let mib = self.remote_download_bytes.load(Ordering::Relaxed) as f64
                            / (1024.0 * 1024.0);
                        ui.strong(format!("Downloading · {mib:.1} MiB received"));
                        ui.small("8 GiB maximum per transfer");
                        if ui.button("Cancel download").clicked() {
                            self.remote_download_cancel.store(true, Ordering::Relaxed);
                        }
                    });
                } else if !self.remote_download_queue.is_empty() {
                    ui.small(format!(
                        "{} downloads queued{} — expand Downloads & transfers to manage them",
                        self.remote_download_queue.len(),
                        if self.remote_download_queue_paused { " (paused)" } else { "" },
                    ));
                }
                if self.remote_upload_busy {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.strong("Verifying WebDAV server and staging upload…");
                        if ui.button("Request cancellation").clicked() {
                            self.remote_upload_cancel.store(true, Ordering::Relaxed);
                        }
                        ui.small("An already-started publish cannot safely be cancelled.");
                    });
                }
                ui.collapsing(format!(
                    "Downloads & transfers{}",
                    if self.remote_download_queue.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", self.remote_download_queue.len())
                    }
                ), |ui| {
                ui.horizontal(|ui| {
                    ui.label("Download to:");
                    let edit = ui.add(egui::TextEdit::singleline(&mut self.remote_download_directory)
                        .desired_width((ui.available_width() - 20.0).max(160.0))
                        .hint_text("Absolute local directory"));
                    if edit.lost_focus()
                        && let Ok(directory) = remote_download::download_directory(&self.remote_download_directory)
                        && self.config.remote_download_directory.as_ref() != Some(&directory)
                    {
                        self.config.remote_download_directory = Some(directory);
                        self.dirty_config = true;
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.remote_staging_busy && !self.remote_download_busy,
                        egui::Button::new("Inspect interrupted downloads"))
                        .on_hover_text("Read-only scan for private staging folders left by an interrupted download")
                        .clicked() {
                        self.inspect_remote_staging();
                    }
                    if self.remote_staging_busy {
                        ui.spinner();
                        ui.small("Inspecting local download folder…");
                    }
                });
                let mut open_staging = None;
                if let Some((folder, result)) = &self.remote_staging_result
                    && folder.to_string_lossy() == self.remote_download_directory {
                    match result {
                        Ok(inspection) if inspection.directories.is_empty() => {
                            ui.small("No private RiceField download staging folders found.");
                        }
                        Ok(inspection) => {
                            ui.colored_label(Color32::YELLOW,
                                "Possible interrupted downloads (or downloads running in another RiceField instance). These are incomplete; nothing has been removed or resumed.");
                            egui::ScrollArea::vertical().max_height(155.0).show(ui, |ui| {
                                for path in &inspection.directories {
                                    ui.horizontal(|ui| {
                                        ui.small(path.file_name().unwrap_or_default().to_string_lossy());
                                        if ui.small_button("Inspect folder").clicked() {
                                            open_staging = Some(path.clone());
                                        }
                                    });
                                }
                            });
                            if inspection.truncated {
                                ui.small("More staging folders exist; only the first 64 are listed.");
                            }
                            ui.small("Inspect manually before deciding what to keep. RiceField never deletes crash remnants automatically.");
                        }
                        Err(error) => { ui.colored_label(Color32::YELLOW, format!("Cannot inspect downloads: {error}")); }
                    }
                }
                if let Some(path) = open_staging {
                    self.navigate(path);
                }
                if !self.remote_download_queue.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(format!("Waiting network downloads: {}", self.remote_download_queue.len()));
                        if self.remote_download_queue_paused {
                            ui.colored_label(Color32::YELLOW, "Queue paused");
                            if ui.button("Resume queue").clicked() {
                                self.remote_download_queue_paused = false;
                                self.start_next_remote_download();
                            }
                        } else if ui.button("Pause after current").clicked() {
                            self.remote_download_queue_paused = true;
                        }
                    });
                    let mut remove_index = None;
                    egui::ScrollArea::vertical().max_height(105.0).show(ui, |ui| {
                        for (index, job) in self.remote_download_queue.iter().enumerate() {
                            ui.horizontal_wrapped(|ui| {
                                ui.small(format!("{}. {}", index + 1, volume::display_remote_name(&job.encoded_name)));
                                if ui.small_button("Remove").clicked() {
                                    remove_index = Some(index);
                                }
                            });
                        }
                    });
                    if let Some(index) = remove_index {
                        self.remote_download_queue.remove(index);
                        if self.remote_download_queue.is_empty() {
                            self.remote_download_queue_paused = false;
                        }
                    }
                }
                });
                ui.horizontal(|ui| {
                    if ui.button("Parent").clicked() {
                        self.parent();
                    }
                    if ui.button("Refresh").clicked() {
                        self.scan(self.active);
                    }
                    if ui
                        .add_enabled(!self.volume_busy, egui::Button::new("Connect / Sign in"))
                        .clicked()
                    {
                        self.open_network_login(
                            self.current().remote_uri.clone().unwrap_or_default(),
                        );
                    }
                    let uri = self.current().remote_uri.clone().unwrap_or_default();
                    let saved = self.config.remote_favorites.contains(&uri);
                    if ui
                        .button(if saved {
                            "Remove favorite"
                        } else {
                            "Save favorite"
                        })
                        .clicked()
                    {
                        if saved {
                            self.config.remote_favorites.retain(|entry| entry != &uri);
                            self.dirty_config = true;
                        } else if uri.len() <= 2048 && self.config.remote_favorites.len() < 32 {
                            self.config.remote_favorites.push(uri);
                            self.dirty_config = true;
                        }
                    }
                    if ui.button("Back to local folder").clicked() {
                        self.navigate(self.current().path.clone());
                    }
                });
                ui.separator();
                let visible = visible_remote_indices(&self.current().remote_entries, &self.current().query);
                let shown_count = visible.as_ref().map_or(self.current().remote_entries.len(), Vec::len);
                let selected = self.current().remote_selected_uri.as_ref().and_then(|uri| {
                    self.current()
                        .remote_entries
                        .iter()
                        .find(|entry| &entry.uri == uri && remote_matches_query(entry, &self.current().query))
                }).cloned();
                let mut selected_action = None;
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(remote_webdav && !self.remote_upload_busy,
                        egui::Button::new("Upload file…"))
                        .on_hover_text(if remote_webdav {
                            "Check server collision safety, then upload a pinned local file"
                        } else {
                            "This backend cannot guarantee an atomic no-replace publish yet"
                        }).clicked() {
                        self.open_remote_upload();
                    }
                    if ui.add_enabled(selected.as_ref().is_some_and(|entry| entry.is_directory),
                        egui::Button::new("Open folder")).clicked() {
                        selected_action = selected.as_ref().map(|entry| (entry.uri.clone(), false));
                    }
                    if ui.add_enabled(selected.as_ref().is_some_and(|entry| entry.is_directory),
                        egui::Button::new("Open in new tab")).clicked() {
                        selected_action = selected.as_ref().map(|entry| (entry.uri.clone(), true));
                    }
                    if ui.add_enabled(selected.as_ref().is_some_and(|entry| entry.is_regular_file),
                        egui::Button::new("Preview")).clicked()
                        && let Some(entry) = &selected {
                        self.start_remote_preview(entry.uri.clone());
                    }
                    if ui.add_enabled(selected.as_ref().is_some_and(|entry| entry.is_directory || entry.is_regular_file),
                        egui::Button::new("Download selected")).clicked()
                        && let Some(entry) = &selected {
                        self.start_remote_download(entry.uri.clone(), entry.name.clone(), entry.is_directory);
                    }
                    ui.separator();
                    ui.small(format!("{} shown / {} total · {}",
                        shown_count, self.current().remote_entries.len(),
                        if selected.is_some() { "1 selected" } else { "Select a row for actions" }));
                });
                if let Some((uri, new_tab)) = selected_action {
                    if new_tab { self.new_remote_tab(uri); } else { self.navigate_remote(uri); }
                    return;
                }
                if self.current().loading {
                    ui.spinner();
                    ui.label("Loading network directory…");
                } else if let Some(error) = &self.current().error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                    if ui.button("Retry connection").clicked() {
                        self.scan(self.active);
                    }
                } else {
                    if let Some(warning) = &self.current().scan_warning {
                        ui.colored_label(Color32::YELLOW, warning);
                    }
                    if shown_count == 0 {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.heading(if self.current().query.is_empty() { "This network folder is empty" } else { "No matching filenames" });
                            if self.current().query.is_empty() {
                                ui.small("Navigate to another folder, or refresh the network listing.");
                            } else {
                                ui.small("Searches filenames, with optional bounded subfolder search.");
                                if ui.button("Clear search").clicked() {
                                    self.current_mut().query.clear();
                                    self.scan(self.active);
                                }
                            }
                        });
                    } else {
                    let current_sort = self.current().remote_sort;
                    let descending = self.current().remote_sort_descending;
                    if let Some(sort) = remote_sort_header(ui, &theme, current_sort, descending) {
                        let tab = self.current_mut();
                        if tab.remote_sort == sort {
                            tab.remote_sort_descending = !tab.remote_sort_descending;
                        } else {
                            tab.remote_sort = sort;
                            tab.remote_sort_descending = false;
                        }
                        sort_remote_entries(&mut tab.remote_entries, tab.remote_sort, tab.remote_sort_descending);
                    }
                    let mut open = None;
                    let mut open_new_tab = None;
                    let mut preview = None;
                    let mut download = None;
                    let mut choose = None;
                    let active_uri = self.current().remote_selected_uri.clone();
                    let entries = &self.current().remote_entries;
                    let list_height = if self.current().remote_preview_uri.is_some() {
                        (ui.available_height() * 0.45).max(120.0)
                    } else {
                        ui.available_height().max(120.0)
                    };
                    let row_height = theme.metrics.row_height.max(28.0);
                    egui::ScrollArea::vertical().max_height(list_height).show_rows(ui, row_height, shown_count, |ui, rows| {
                        for row in rows {
                            let index = visible.as_ref().map_or(row, |indices| indices[row]);
                            let entry = &entries[index];
                            let display_name = volume::display_remote_name(&entry.name);
                            let response = shell_renderer::remote_row(
                                ui, &theme, shell_renderer::RemoteRowVisual {
                                    name: &display_name,
                                    is_directory: entry.is_directory,
                                    is_regular_file: entry.is_regular_file,
                                    reported_size: entry.reported_size,
                                    selected: active_uri.as_deref() == Some(entry.uri.as_str()),
                                }, row_height,
                            ).on_hover_text(&entry.uri);
                            if response.clicked() {
                                choose = Some(entry.uri.clone());
                            }
                            if response.double_clicked() {
                                if entry.is_directory { open = Some(entry.uri.clone()); }
                                else if entry.is_regular_file { preview = Some(entry.uri.clone()); }
                            }
                            response.context_menu(|ui| {
                                if entry.is_directory && ui.button("Open folder").clicked() {
                                    open = Some(entry.uri.clone());
                                    ui.close();
                                }
                                if entry.is_directory && ui.button("Open in new tab").clicked() {
                                    open_new_tab = Some(entry.uri.clone());
                                    ui.close();
                                }
                                if entry.is_regular_file && ui.button("Preview file").clicked() {
                                    preview = Some(entry.uri.clone());
                                    ui.close();
                                }
                                if (entry.is_directory || entry.is_regular_file) && ui.button("Download to local folder").clicked() {
                                    download = Some((entry.uri.clone(), entry.name.clone(), entry.is_directory));
                                    ui.close();
                                }
                            });
                        }
                    });
                    if let Some(uri) = choose {
                        self.current_mut().remote_selected_uri = Some(uri);
                    }
                    if let Some(uri) = open_new_tab {
                        self.new_remote_tab(uri);
                    } else if let Some(uri) = open {
                        self.navigate_remote(uri);
                    } else if let Some(uri) = preview {
                        self.start_remote_preview(uri);
                    }
                    if let Some((uri, name, is_directory)) = download {
                        self.start_remote_download(uri, name, is_directory);
                    }
                    }
                }
                if let Some(uri) = self.current().remote_preview_uri.clone() {
                    ui.separator();
                    let mut close = false;
                    ui.horizontal(|ui| {
                        ui.strong("Read-only network preview (32 KiB prefix)");
                        close = ui.small_button("Close preview").clicked();
                    });
                    if close {
                        self.current_mut().clear_remote_preview();
                    } else {
                        ui.small(uri);
                        match &self.current().remote_preview_result {
                            None => {
                                ui.spinner();
                                ui.label("Reading remote file…");
                            }
                            Some(Err(error)) => {
                                ui.colored_label(Color32::LIGHT_RED, error);
                            }
                            Some(Ok(content)) if content.is_empty() => {
                                ui.label("The remote file is empty.");
                            }
                            Some(Ok(content)) => {
                                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                                    ui.monospace(content);
                                });
                            }
                        }
                    }
                }
            });
    }

    fn file_listing(&mut self, ui: &mut egui::Ui, tab_index: usize) {
        let grid = self.config.view == "grid";
        let width = ui.available_width();
        let tile = self.theme.metrics.tile_width;
        let columns = if grid {
            ((width / (tile + 8.0)).floor() as usize).max(1)
        } else {
            1
        };
        let visible = self.tabs[tab_index].visible.len();
        let row_height = if grid {
            98.0
        } else {
            self.theme.metrics.row_height
        };
        let rows = visible.div_ceil(columns);
        if !grid {
            let change_sort = shell_renderer::sort_header(
                ui,
                &self.theme,
                &self.config.sort_by,
                self.config.descending,
            );
            if let Some(sort) = change_sort {
                if self.config.sort_by == sort {
                    self.config.descending = !self.config.descending;
                } else {
                    self.config.sort_by = sort.into();
                    self.config.descending = false;
                }
                self.dirty_config = true;
                for index in 0..self.tabs.len() {
                    self.rebuild_visible(index);
                }
            }
            ui.separator();
        }
        egui::ScrollArea::vertical()
            .id_salt(("file_scroller", self.tabs[tab_index].id))
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows, |ui, range| {
                for row in range {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for col in 0..columns {
                            let visible_index = row * columns + col;
                            let entry = self.tabs[tab_index]
                                .visible
                                .get(visible_index)
                                .and_then(|index| self.tabs[tab_index].entries.get(*index))
                                .cloned();
                            if let Some(entry) = entry {
                                self.file_item(
                                    ui,
                                    &entry,
                                    visible_index,
                                    tab_index,
                                    FileCellLayout {
                                        width: if grid { tile } else { width },
                                        height: row_height,
                                        grid,
                                        row_index: visible_index,
                                    },
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
        tab_index: usize,
        item: FileCellLayout,
    ) {
        let selected = self.tabs[tab_index].selected.contains(&entry.path);
        let thumbnail = if item.grid {
            self.thumbnails.request(entry)
        } else {
            None
        };
        let (rect, response) =
            shell_renderer::file_cell(ui, entry, &self.theme, selected, item, thumbnail);
        if entry.is_dir {
            self.drop_areas.push(DropArea {
                tab_id: self.tabs[tab_index].id,
                destination: entry.path.clone(),
                rect,
                folder: true,
            });
        }
        if response.drag_started() {
            if self.active != tab_index {
                self.tab_command(TabCommand::Activate(tab_index));
            }
            if !selected {
                self.select_entry(entry, index, Modifiers::default());
            }
            self.drag_files = Some(DragFiles {
                source_tab: self.tabs[tab_index].id,
                paths: self.selected_paths(),
            });
        }
        let double = response.double_clicked();
        let clicked = response.clicked();
        if (clicked || double || response.secondary_clicked()) && self.active != tab_index {
            self.tab_command(TabCommand::Activate(tab_index));
        }
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
            if !entry.is_dir && ui.button("Open With…").clicked() {
                self.choose_open_with_path(entry.path.clone());
                ui.close();
            }
            if ui.button("Preview").clicked() {
                self.open_preview(entry.path.clone());
                ui.close();
            }
            if ui.button("Open in new tab").clicked() {
                if entry.is_dir {
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
            if ui.button("Drag to another app…").clicked() {
                if !self.current().selected.contains(&entry.path) {
                    self.select_entry(entry, index, Modifiers::default());
                }
                self.status = Some(match crate::external_drag::launch(&self.selected_paths()) {
                    Ok(helper) => (
                        format!("Opened {helper} drag source; drop files into the other app."),
                        false,
                    ),
                    Err(error) => (error, true),
                });
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
                if theme.shell_kind() == ShellKind::Aero {
                    let chosen = self
                        .current()
                        .entries
                        .iter()
                        .find(|entry| self.current().selected.contains(&entry.path))
                        .cloned();
                    ui.horizontal(|ui| {
                        let (title, detail) = if let Some(entry) = chosen {
                            let title = entry.name.clone();
                            let detail = format!(
                                "{}    {}    Modified {}",
                                entry.kind(),
                                if entry.is_dir {
                                    "File folder".to_owned()
                                } else {
                                    filesystem::human_size(entry.size)
                                },
                                filesystem::age(entry.modified)
                            );
                            (title, detail)
                        } else if let Some(uri) = &self.current().remote_uri {
                            (
                                "Network location".into(),
                                format!(
                                    "{} network items · Read-only browser · {uri}",
                                    self.current().remote_entries.len()
                                ),
                            )
                        } else {
                            let title = self
                                .current()
                                .path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "Computer".into());
                            (
                                title,
                                format!(
                                    "{} items · Choose an item to see its details",
                                    self.current().visible.len()
                                ),
                            )
                        };
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(34.0, 31.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            rect.shrink(3.0),
                            3,
                            Color32::from_rgb(125, 186, 222),
                        );
                        ui.painter().rect_stroke(
                            rect.shrink(3.0),
                            3,
                            egui::Stroke::new(1.0, Color32::from_rgb(64, 124, 170)),
                            egui::StrokeKind::Inside,
                        );
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(shorten(&title, 60)).strong().size(12.5));
                            ui.label(
                                egui::RichText::new(detail)
                                    .size(11.2)
                                    .color(theme.palette.muted()),
                            );
                        });
                    });
                    ui.separator();
                }
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
                    if theme.shell_kind() == ShellKind::Ribbon && ui.available_width() > 130.0 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new("Icons").selected(self.config.view == "grid"),
                                )
                                .on_hover_text("Large icons")
                                .clicked()
                            {
                                self.win10_command(RibbonAction::LargeIcons, ui.ctx());
                            }
                            if ui
                                .add(
                                    egui::Button::new("Details")
                                        .selected(self.config.view == "list"),
                                )
                                .on_hover_text("Details list")
                                .clicked()
                            {
                                self.win10_command(RibbonAction::Details, ui.ctx());
                            }
                        });
                    }
                });
            });
    }

    fn archive_window(&mut self, context: &egui::Context) {
        let Some(mut dialog) = self.archive_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut requested = None;
        let title = match dialog.mode {
            ArchiveMode::Browse { .. } => "Archive browser · 0.5 preview",
            ArchiveMode::Create { .. } => "Create archive · 0.5 preview",
        };
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(590.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(context, |ui| {
                match &mut dialog.mode {
                    ArchiveMode::Browse { path, destination, entries, selected, folder_name, password } => {
                        ui.label(format!("Source: {}", path.display()));
                        ui.label("All paths are checked for traversal, links and special files before extraction.");
                        ui.horizontal(|ui| {
                            ui.label("Password (if encrypted):");
                            ui.add_sized([245.0, 24.0], egui::TextEdit::singleline(password)
                                .password(true).hint_text("Session only · never saved"));
                            if ui.add_enabled(!dialog.running, egui::Button::new("Reload list")).clicked() {
                                *entries = None;
                                selected.clear();
                                requested = Some(ArchiveOperation::List {
                                    path: path.clone(),
                                    password: (!password.is_empty()).then(|| password.clone()),
                                });
                            }
                        });
                        ui.small("Encrypted ZIP and supported 7z/RAR archives may need a password; ZIP filenames can be listed while contents remain locked.");
                        if let Some(entries) = entries {
                            ui.horizontal(|ui| {
                                ui.label(format!("{} entries", entries.len()));
                                if ui.small_button("Select all").clicked() { *selected = (0..entries.len()).collect(); }
                                if ui.small_button("Select none").clicked() { selected.clear(); }
                            });
                            egui::ScrollArea::vertical().max_height(300.0)
                                .show_rows(ui, 23.0, entries.len(), |ui, range| {
                                    for index in range {
                                        let entry = &entries[index];
                                        let mut checked = selected.contains(&index);
                                        ui.horizontal(|ui| {
                                            if ui.checkbox(&mut checked, "").changed() {
                                                if checked { selected.insert(index); } else { selected.remove(&index); }
                                            }
                                            let label = entry.path.display().to_string();
                                            ui.label(shorten(&label, 68)).on_hover_text(label);
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if entry.directory { ui.small("folder"); }
                                                else { ui.small(filesystem::human_size(entry.bytes)); }
                                            });
                                        });
                                    }
                                });
                            ui.horizontal(|ui| {
                                ui.label("Extract into new folder:");
                                ui.add_sized([245.0, 24.0], egui::TextEdit::singleline(folder_name));
                            });
                            ui.small(format!("Parent: {} · Existing destinations will not be overwritten", destination.display()));
                            ui.horizontal(|ui| {
                                if ui.add_enabled(!dialog.running, egui::Button::new("Verify integrity")).clicked() {
                                    requested = Some(ArchiveOperation::Verify { path: path.clone(),
                                        password: (!password.is_empty()).then(|| password.clone()) });
                                }
                                if ui.add_enabled(!dialog.running && !selected.is_empty(),
                                    egui::Button::new(format!("Extract {} selected", selected.len()))).clicked() {
                                    requested = Some(ArchiveOperation::Extract { path: path.clone(),
                                        destination: destination.clone(), folder: folder_name.clone(),
                                        selected: selected.iter().map(|&index| entries[index].path.clone()).collect(),
                                        password: (!password.is_empty()).then(|| password.clone()) });
                                }
                            });
                        } else if dialog.running { ui.spinner(); ui.label("Reading archive directory…"); }
                        else { ui.label("Enter a password and select Reload list if this archive is locked."); }
                    }
                    ArchiveMode::Create { sources, destination, stem, format, encrypt, password } => {
                        ui.label(format!("{} selected source(s)", sources.len()));
                        ui.label(format!("Destination folder: {}", destination.display()));
                        ui.horizontal(|ui| {
                            ui.label("Archive name:");
                            ui.add_sized([250.0, 24.0], egui::TextEdit::singleline(stem));
                        });
                        egui::ComboBox::from_label("Format")
                            .selected_text(format.extension())
                            .show_ui(ui, |ui| {
                                for candidate in archive::Format::ALL {
                                    ui.selectable_value(format, candidate, candidate.extension());
                                }
                            });
                        ui.checkbox(encrypt, "Encrypt ZIP with AES-256");
                        if *encrypt {
                            ui.horizontal(|ui| {
                                ui.label("Password:");
                                ui.add_sized([260.0, 24.0], egui::TextEdit::singleline(password)
                                    .password(true).hint_text("Session only · never saved"));
                            });
                            if *format != archive::Format::Zip {
                                ui.colored_label(Color32::LIGHT_RED, "Encryption is available only for ZIP archives.");
                            }
                        }
                        ui.small(format!("Output: {}.{} · will never overwrite", stem, format.extension()));
                        if ui.add_enabled(!dialog.running && (!*encrypt || (*format == archive::Format::Zip && !password.is_empty())),
                            egui::Button::new("Create archive")).clicked() {
                            requested = Some(ArchiveOperation::Create { sources: sources.clone(),
                                destination: destination.clone(), stem: stem.clone(), format: *format,
                                password: (*encrypt).then(|| password.clone()) });
                        }
                    }
                }
                if dialog.running {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Working · {} processed", filesystem::human_size(dialog.transferred)));
                        if ui.button("Cancel").clicked() { cancel = true; }
                    });
                    if !dialog.current_file.is_empty() { ui.small(shorten(&dialog.current_file, 95)); }
                }
                if let Some((message, error)) = &dialog.message {
                    ui.separator();
                    ui.colored_label(if *error { Color32::LIGHT_RED } else { Color32::LIGHT_GREEN }, message);
                }
            });
        if cancel {
            self.archive_cancel.store(true, Ordering::Relaxed);
        }
        if open && !context.input(|input| input.key_pressed(Key::Escape)) {
            self.archive_dialog = Some(dialog);
            if let Some(operation) = requested {
                self.start_archive_operation(operation);
            }
        } else {
            self.archive_cancel.store(true, Ordering::Relaxed);
            // Do not lose the cancellation handshake: the worker must finish
            // cleaning its private stage before other file mutations start.
            if dialog.running {
                dialog.message = Some(("Cancelling; waiting for archive cleanup…".into(), false));
                self.archive_dialog = Some(dialog);
            } else {
                self.archive_generation = self.archive_generation.wrapping_add(1);
            }
        }
    }

    fn dialogs(&mut self, context: &egui::Context) {
        if self.dialog.is_some() && context.input(|input| input.key_pressed(Key::Escape)) {
            self.analysis_cancel.store(true, Ordering::Relaxed);
            self.duplicate_cancel.store(true, Ordering::Relaxed);
            self.open_with_cancel.store(true, Ordering::Relaxed);
            self.dialog = None;
            return;
        }
        let mut action = None;
        let mut dismiss = false;
        let mut refresh_paste_preview = None;
        let mut requested_analysis = None;
        let mut open_with_launch = None;
        let mut theme_editor_outcome = None;
        let analysis_busy = self.analysis_worker_running;
        if let Some(dialog) = self.dialog.as_mut() {
            match dialog {
                Dialog::ThemeEditor(editor) => {
                    shell_components::show_dialog(
                        context,
                        "Customize RiceField",
                        true,
                        Some(680.0),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .max_height((context.content_rect().height() - 90.0).max(240.0))
                                .show(ui, |ui| theme_editor_outcome = editor.ui(ui));
                        },
                    );
                }
                Dialog::OpenWith { path, apps, .. } => {
                    shell_components::show_dialog(context, "Open With", true, Some(450.0), |ui| {
                        ui.label(format!("Choose an installed app for {}", path.display()));
                        match apps {
                            None => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Looking up MIME handlers…");
                                });
                            }
                            Some(Err(error)) => {
                                ui.colored_label(Color32::RED, error.as_str());
                            }
                            Some(Ok(handlers)) => {
                                egui::ScrollArea::vertical()
                                    .max_height(260.0)
                                    .show(ui, |ui| {
                                        for desktop_id in handlers.iter() {
                                            if ui
                                                .button(desktop_id.trim_end_matches(".desktop"))
                                                .on_hover_text(desktop_id)
                                                .clicked()
                                            {
                                                open_with_launch =
                                                    Some((desktop_id.clone(), path.clone()));
                                                dismiss = true;
                                            }
                                        }
                                    });
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            dismiss = true;
                        }
                    });
                }
                Dialog::Duplicates { result, .. } => {
                    shell_components::show_dialog(
                        context,
                        "Potential duplicate files",
                        true,
                        Some(620.0),
                        |ui| {
                            ui.small("Read-only SHA-256 comparison of visible local files. Verify files again before deleting anything.");
                            match result {
                                None => {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label("Scanning equal-size candidates (up to 256 files / 512 MiB / 45 seconds)…");
                                    });
                                }
                                Some(Err(error)) => {
                                    ui.colored_label(Color32::RED, error.as_str());
                                }
                                Some(Ok(report)) => {
                                    ui.label(format!(
                                        "Hashed {} files · {} skipped{}",
                                        report.hashed,
                                        report.skipped,
                                        if report.partial {
                                            " · partial results"
                                        } else {
                                            ""
                                        }
                                    ));
                                    if report.groups.is_empty() {
                                        ui.label("No matching hashes among inspected files.");
                                    }
                                    egui::ScrollArea::vertical()
                                        .max_height(320.0)
                                        .show(ui, |ui| {
                                            for (index, group) in report.groups.iter().enumerate() {
                                                ui.separator();
                                                ui.strong(format!(
                                                    "Group {} · {} files",
                                                    index + 1,
                                                    group.len()
                                                ));
                                                for path in group {
                                                    ui.monospace(path.display().to_string());
                                                }
                                            }
                                        });
                                }
                            }
                            if ui.button("Close / Cancel").clicked() {
                                dismiss = true;
                            }
                        },
                    );
                }
                Dialog::PasteOptions {
                    clipboard,
                    destination,
                    conflict,
                    preview,
                    ..
                } => {
                    shell_components::show_dialog(
                        context,
                        "Paste with options",
                        true,
                        None,
                        |ui| {
                            ui.label(format!(
                                "{} {} item(s) into {}",
                                if clipboard.cut { "Move" } else { "Copy" },
                                clipboard.paths.len(),
                                destination.display()
                            ));
                            if clipboard.cut {
                                ui.colored_label(
                                    Color32::LIGHT_YELLOW,
                                    "Across filesystems, Move makes a verified copy and keeps the original. Remove it manually after reviewing the copy.",
                                );
                            }
                            ui.separator();
                            let mut policy_changed = ui
                                .radio_value(
                                    conflict,
                                    ConflictPolicy::KeepBoth,
                                    "Keep both — rename incoming items when names collide",
                                )
                                .changed();
                            policy_changed |= ui.radio_value(
                                conflict,
                                ConflictPolicy::SkipExisting,
                                "Skip existing — leave any existing destination item untouched (no comparison)",
                            ).changed();
                            policy_changed |= ui.radio_value(
                                conflict,
                                ConflictPolicy::AbortOnConflict,
                                "Require no conflicts — cancel the entire batch if a name collides",
                            ).changed();
                            policy_changed |= ui.radio_value(
                                conflict,
                                ConflictPolicy::SkipIfIdentical,
                                "Skip only matching contents — reject different or unverifiable files",
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
                                            ui.small(
                                                "Checking all selected paths in the background…",
                                            );
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
                                        egui::ScrollArea::vertical().max_height(190.0).show_rows(
                                            ui,
                                            20.0,
                                            plan.items.len(),
                                            |ui, rows| {
                                                for planned in &plan.items[rows] {
                                                    let decision = if planned.disposition
                                                        == filesystem::PlannedDisposition::Skip
                                                    {
                                                        "Skip"
                                                    } else if planned.renamed {
                                                        "Rename incoming"
                                                    } else {
                                                        "Transfer"
                                                    };
                                                    let source =
                                                        planned.source.display().to_string();
                                                    ui.horizontal(|ui| {
                                                        ui.small(
                                                            if planned.collision && !planned.renamed
                                                            {
                                                                format!("{decision} · collision")
                                                            } else {
                                                                decision.to_owned()
                                                            },
                                                        );
                                                        ui.monospace(shorten(&source, 45))
                                                            .on_hover_text(format!(
                                                                "Source: {}\nDestination: {}",
                                                                planned.source.display(),
                                                                planned.target.display()
                                                            ));
                                                    });
                                                }
                                            },
                                        );
                                        true
                                    }
                                }
                            };
                            ui.small("Preview only: the worker rechecks paths before execution. Matching contents are compared read-only (up to 512 MiB / 10,000 entries); this is not a permanent integrity guarantee. No existing file is overwritten.");
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    dismiss = true;
                                }
                                if ui
                                    .add_enabled(can_start, egui::Button::new("Start transfer"))
                                    .clicked()
                                {
                                    action = Some(Self::paste_action(
                                        clipboard.clone(),
                                        destination.clone(),
                                        *conflict,
                                    ));
                                    dismiss = true;
                                }
                            });
                        },
                    );
                }
                Dialog::Name { mode, value } => {
                    let title = match mode {
                        NameMode::Folder(_) => "New folder",
                        NameMode::File(_) => "New empty file",
                        NameMode::Rename(_) => "Rename item",
                    };
                    shell_components::show_dialog(context, title, false, None, |ui| {
                        ui.label("Name");
                        let input = ui.add_sized(
                            [ui.available_width().min(340.0), 28.0],
                            egui::TextEdit::singleline(value),
                        );
                        let valid = filesystem::valid_name(value);
                        if !valid {
                            shell_components::small_error(
                                ui,
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
                    regex,
                    numbering,
                    number_start,
                    number_width,
                } => {
                    shell_components::show_dialog(
                        context,
                        format!("Batch rename ({} items)", paths.len()),
                        true,
                        Some(580.0),
                        |ui| {
                            ui.label("Preview every new name before applying the changes.");
                            ui.horizontal(|ui| {
                                ui.label("Prefix:");
                                ui.add_sized([220.0, 26.0], egui::TextEdit::singleline(prefix));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Suffix:");
                                ui.add_sized([220.0, 26.0], egui::TextEdit::singleline(suffix));
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label("Find:");
                                ui.add_sized(
                                    [ui.available_width().min(170.0), 26.0],
                                    egui::TextEdit::singleline(find),
                                );
                                ui.label("Replace:");
                                ui.add_sized(
                                    [ui.available_width().min(170.0), 26.0],
                                    egui::TextEdit::singleline(replace),
                                );
                            });
                            ui.checkbox(regex, "Regular expression (supports $1 capture replacements)")
                                .on_hover_text("Use Rust regular expressions. Names that are not valid UTF-8 cannot use search and replace.");
                            ui.horizontal_wrapped(|ui| {
                                ui.checkbox(numbering, "Add sequence numbers");
                                if *numbering {
                                    ui.label("Start:");
                                    ui.add(
                                        egui::DragValue::new(number_start).range(1..=99_999_999),
                                    );
                                    ui.label("Digits:");
                                    ui.add(egui::DragValue::new(number_width).range(1..=8));
                                }
                            });
                            let options = filesystem::BatchRenameOptions {
                                prefix,
                                suffix,
                                find,
                                replace,
                                regex: *regex,
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
                                    shell_components::small_error(ui, error.to_string());
                                    if ui.button("Cancel").clicked() {
                                        dismiss = true;
                                    }
                                }
                            }
                        },
                    );
                }
                Dialog::Trash(paths) => {
                    shell_components::show_dialog(context, "Move to Trash?", false, None, |ui| {
                        ui.label(format!("Move {} selected item(s) to Trash?", paths.len()));
                        ui.label("The items can usually be restored from your desktop Trash.");
                        removal_confirmation_paths(ui, paths);
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
                    shell_components::show_dialog(
                        context,
                        "Permanently delete items?",
                        false,
                        None,
                        |ui| {
                            ui.colored_label(
                                Color32::RED,
                                format!("Delete {} item(s) permanently?", paths.len()),
                            );
                            ui.label("This bypasses Trash. There is no undo.");
                            removal_confirmation_paths(ui, paths);
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
                        },
                    );
                }
                Dialog::Properties {
                    path,
                    mode,
                    owner,
                    group,
                    attribute_name,
                    attribute_value,
                    details,
                    analysis,
                    generation,
                    ..
                } => {
                    shell_components::show_dialog(context, "Properties", true, Some(460.0), |ui| {
                        match details {
                            Some(Ok(snapshot)) => {
                                for (name, value) in &snapshot.lines {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.strong(format!("{name}:"));
                                        ui.label(value);
                                    });
                                }
                            }
                            Some(Err(error)) => {
                                ui.colored_label(
                                    Color32::RED,
                                    format!("Cannot read properties: {error}"),
                                );
                            }
                            None => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Reading file properties…");
                                });
                            }
                        }
                        #[cfg(unix)]
                        if let Some(Ok(snapshot)) = details.as_ref()
                            && !snapshot.is_symlink
                        {
                            ui.separator();
                            let name = if snapshot.is_dir {
                                "Calculate folder size"
                            } else {
                                "Calculate SHA-256"
                            };
                            if ui
                                .add_enabled(!analysis_busy, egui::Button::new(name))
                                .clicked()
                            {
                                *analysis = None;
                                requested_analysis = Some((path.clone(), *generation));
                            }
                            if analysis_busy {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Analysing in background…");
                                });
                            }
                            if let Some(result) = analysis {
                                match result {
                                    Ok(result) => {
                                        ui.label(egui::RichText::new(result.as_str()).monospace());
                                    }
                                    Err(error) => {
                                        ui.colored_label(Color32::RED, error.as_str());
                                    }
                                }
                            }
                        }
                        #[cfg(unix)]
                        if let Some(Ok(snapshot)) = details.as_ref() {
                            ui.separator();
                            if snapshot.is_symlink {
                                ui.label("Symlink permissions cannot be changed here.");
                            }
                            ui.horizontal(|ui| {
                                ui.label("Mode (octal):");
                                ui.add_sized([100.0, 25.0], egui::TextEdit::singleline(mode));
                                let parsed = u32::from_str_radix(mode.trim(), 8)
                                    .ok()
                                    .filter(|number| *number <= 0o777);
                                if ui
                                    .add_enabled(
                                        parsed.is_some() && !snapshot.is_symlink,
                                        egui::Button::new("Apply"),
                                    )
                                    .clicked()
                                {
                                    action = Some(Action::SetMode {
                                        path: path.clone(),
                                        mode: parsed.expect("button only enabled for valid mode"),
                                        expected: snapshot.metadata.clone(),
                                    });
                                    dismiss = true;
                                }
                            });
                            #[cfg(target_os = "linux")]
                            ui.horizontal_wrapped(|ui| {
                                use std::os::unix::fs::MetadataExt;
                                ui.label("Owner UID:");
                                ui.add_sized([84.0, 25.0], egui::TextEdit::singleline(owner));
                                ui.label("Group GID:");
                                ui.add_sized([84.0, 25.0], egui::TextEdit::singleline(group));
                                let parsed = owner
                                    .trim()
                                    .parse::<u32>()
                                    .ok()
                                    .filter(|uid| *uid != u32::MAX)
                                    .zip(
                                        group
                                            .trim()
                                            .parse::<u32>()
                                            .ok()
                                            .filter(|gid| *gid != u32::MAX),
                                    );
                                let changed = parsed.is_some_and(|(uid, gid)| {
                                    uid != snapshot.metadata.uid() || gid != snapshot.metadata.gid()
                                });
                                if ui
                                    .add_enabled(
                                        changed && !snapshot.is_symlink,
                                        egui::Button::new("Apply owner/group"),
                                    )
                                    .clicked()
                                {
                                    let (uid, gid) =
                                        parsed.expect("owner/group button requires valid IDs");
                                    action = Some(Action::SetOwnership {
                                        path: path.clone(),
                                        uid,
                                        gid,
                                        expected: snapshot.metadata.clone(),
                                    });
                                    dismiss = true;
                                }
                            });
                            #[cfg(target_os = "linux")]
                            ui.small("Ownership changes follow your account's Linux permissions; no sudo or privilege escalation.");
                            #[cfg(target_os = "linux")]
                            ui.vertical(|ui| {
                                ui.separator();
                                ui.label("Add an extended attribute (existing values are never overwritten):");
                                ui.horizontal_wrapped(|ui| {
                                    ui.label("Name:");
                                    ui.add_sized([170.0, 25.0], egui::TextEdit::singleline(attribute_name));
                                    ui.label("Text value:");
                                    ui.add_sized([170.0, 25.0], egui::TextEdit::singleline(attribute_value));
                                });
                                let valid = attribute_name.starts_with("user.")
                                    && attribute_name.len() > 5
                                    && attribute_name.len() <= 255
                                    && !attribute_name.as_bytes().contains(&0)
                                    && attribute_value.len() <= 4096
                                    && !snapshot.is_symlink;
                                if ui.add_enabled(valid, egui::Button::new("Create attribute")).clicked() {
                                    action = Some(Action::CreateUserAttribute {
                                        path: path.clone(),
                                        name: attribute_name.clone(),
                                        value: attribute_value.clone(),
                                        expected: snapshot.metadata.clone(),
                                    });
                                    dismiss = true;
                                }
                                ui.small("Only user.* names and up to 4 KiB of UTF-8 text. A conflicting attribute is left untouched.");
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
            self.analysis_cancel.store(true, Ordering::Relaxed);
            self.duplicate_cancel.store(true, Ordering::Relaxed);
            self.open_with_cancel.store(true, Ordering::Relaxed);
            self.dialog = None;
        }
        if let Some(outcome) = theme_editor_outcome {
            match outcome {
                theme_editor::Outcome::Close => self.dialog = None,
                theme_editor::Outcome::Activate { path, theme } => {
                    self.config.theme = theme.id.clone();
                    self.config.custom_theme_file = Some(path.clone());
                    self.theme = *theme;
                    self.theme_file = Some(path.clone());
                    self.theme_file_state = None;
                    self.theme_reload_pending = None;
                    self.theme.apply(context);
                    self.dirty_config = true;
                    self.dialog = None;
                    self.status = Some((format!("Saved and activated {}", path.display()), false));
                }
            }
        }
        if let Some((desktop_id, path)) = open_with_launch {
            self.status = Some(match open_with::launch(&desktop_id, &path) {
                Ok(()) => (format!("Requested launch using {desktop_id}"), false),
                Err(error) => (format!("Cannot launch {desktop_id}: {error}"), true),
            });
        }
        if let Some((path, generation)) = requested_analysis {
            self.start_deep_analysis(path, generation);
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

impl Drop for Explorer {
    fn drop(&mut self) {
        self.analysis_cancel.store(true, Ordering::Relaxed);
        self.duplicate_cancel.store(true, Ordering::Relaxed);
        self.open_with_cancel.store(true, Ordering::Relaxed);
        volume::clear_password(&mut self.volume_connect_password);
        volume::clear_password(&mut self.volume_unlock_password);
        volume::clear_password(&mut self.remote_upload_password);
        self.remote_upload_cancel.store(true, Ordering::Relaxed);
        self.discovery_cancel.store(true, Ordering::Relaxed);
        // Drain pending completion records before exiting. A stalled XDG
        // state mount must never trap the desktop in an unbounded close.
        self.history_sender.take();
        if let Some(done) = self.history_done_receiver.take() {
            let _ = done.recv_timeout(Duration::from_secs(2));
        }
        self.config_writer.finish();
    }
}

impl eframe::App for Explorer {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        self.poll_background(&context);
        self.poll_desktop(&context);
        if self.theme_file.is_some() && self.theme_last_check.elapsed() >= Duration::from_secs(3) {
            self.theme_last_check = Instant::now();
            self.reload_theme_file(&context, false);
        }
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
                .filter(|tab| tab.remote_uri.is_none())
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
                    device_volumes: crate::udisks2::volumes(),
                    places: {
                        let home = config::home();
                        let mut places = vec![("Home", home.clone())];
                        for name in [
                            "Desktop",
                            "Documents",
                            "Downloads",
                            "Music",
                            "Pictures",
                            "Videos",
                        ] {
                            let path = home.join(name);
                            if path.is_dir() {
                                places.push((name, path));
                            }
                        }
                        places.push(("Filesystem", PathBuf::from("/")));
                        places
                    },
                });
            });
            self.last_refresh_check = Instant::now();
        }
        // Desktop shortcuts must not mutate the directory or switch tabs
        // behind a network login, mount confirmation, discovery or recovery
        // window. These windows own keyboard focus until dismissed.
        if self.dialog.is_none()
            && self.archive_dialog.is_none()
            && !self.volume_connect_open
            && self.volume_unlock_target.is_none()
            && !self.remote_upload_open
            && self.volume_confirm.is_none()
            && !self.discovery_open
            && !self.recovery_open
        {
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
        self.drop_areas.clear();
        self.main_panel(ui);
        self.thumbnails.dispatch();
        self.quick_look_window(&context);
        self.finish_drag(&context);
        self.accept_external_drop(&context);
        self.dialogs(&context);
        self.archive_window(&context);
        self.volume_dialog(&context);
        self.remote_upload_dialog(&context);
        self.network_browser(&context);
        self.recovery_dialog(&context);
        self.transfer_dialog(&context);
        if self.dirty_config && self.session_owner {
            if let Err(error) = self.config_writer.request(&self.config) {
                self.status = Some((format!("Cannot save preferences: {error}"), true));
            } else {
                self.dirty_config = false;
            }
        }
        let waiting_for_preview = matches!(
            &self.dialog,
            Some(Dialog::PasteOptions { preview: None, .. })
        );
        let waiting_for_properties =
            matches!(&self.dialog, Some(Dialog::Properties { details: None, .. }));
        let waiting_for_duplicates =
            matches!(&self.dialog, Some(Dialog::Duplicates { result: None, .. }));
        let waiting_for_open_with =
            matches!(&self.dialog, Some(Dialog::OpenWith { apps: None, .. }));
        let idle_delay = if self.busy
            || self
                .archive_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.running)
            || self.volume_busy
            || waiting_for_preview
            || waiting_for_properties
            || waiting_for_duplicates
            || waiting_for_open_with
            || self.folder_tree.has_pending()
            || self.thumbnails.has_pending()
            || self.remote_download_busy
            || self.tabs.iter().any(|tab| tab.loading)
        {
            Duration::from_millis(90)
        } else {
            Duration::from_secs(3)
        };
        let delay = next_search_repaint_delay(&self.tabs, Instant::now())
            .map_or(idle_delay, |search_delay| search_delay.min(idle_delay));
        context.request_repaint_after(delay);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Action, CLIPBOARD_WAIT, Clipboard, Dialog, ItemCounts, NameMode, PasteMode, PendingPaste,
        PropertyResult, PropertySnapshot, Tab, TabSnapshot, accept_property_result,
        active_after_swap, choose_paste_clipboard, elapsed_label, next_search_repaint_delay,
        read_property_snapshot, remaining_cut_paths, resolve_split_tabs, retain_visible_selection,
        transfer_progress_details,
    };
    use crate::clipboard;
    use crate::filesystem::{ItemResult, ItemState, Progress};
    use crate::location::Location;
    use crate::transfer_history;
    use crate::volume::RemoteEntry;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    fn remote_entry(name: &str, directory: bool, size: Option<u64>) -> RemoteEntry {
        RemoteEntry {
            uri: format!("smb://host/share/{name}"),
            name: name.to_owned(),
            is_directory: directory,
            is_regular_file: !directory,
            reported_size: size,
        }
    }

    #[test]
    fn remote_breadcrumbs_keep_encoded_path_segments_and_reject_local_paths() {
        let crumbs = super::remote_breadcrumbs("smb://nas.local/share/My%20Docs/a%2Fb");
        assert_eq!(crumbs.len(), 4);
        assert_eq!(crumbs[0], ("nas.local".into(), "smb://nas.local".into()));
        assert_eq!(
            crumbs[2],
            ("My Docs".into(), "smb://nas.local/share/My%20Docs".into())
        );
        assert_eq!(crumbs[3].1, "smb://nas.local/share/My%20Docs/a%2Fb");
        assert!(super::remote_breadcrumbs("/home/user").is_empty());
    }

    #[test]
    fn remote_sort_groups_folders_and_naturally_orders_filenames() {
        let mut entries = vec![
            remote_entry("file10.txt", false, Some(10)),
            remote_entry("Folder2", true, None),
            remote_entry("file2.txt", false, Some(2)),
            remote_entry("Folder10", true, None),
        ];
        super::sort_remote_entries(&mut entries, super::RemoteSort::Name, false);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["Folder2", "Folder10", "file2.txt", "file10.txt"]
        );
        super::sort_remote_entries(&mut entries, super::RemoteSort::Name, true);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["Folder10", "Folder2", "file10.txt", "file2.txt"]
        );
    }

    #[test]
    fn remote_size_sort_retains_folder_group_and_unknown_size() {
        let mut entries = vec![
            remote_entry("largest", false, Some(500)),
            remote_entry("folder", true, None),
            remote_entry("unknown", false, None),
            remote_entry("smallest", false, Some(1)),
        ];
        super::sort_remote_entries(&mut entries, super::RemoteSort::Size, false);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["folder", "unknown", "smallest", "largest"]
        );
        super::sort_remote_entries(&mut entries, super::RemoteSort::Size, true);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["folder", "largest", "smallest", "unknown"]
        );
    }

    #[test]
    fn remote_search_filters_display_names_without_changing_network_uris() {
        let entries = vec![
            remote_entry("Family%20Photos", true, None),
            remote_entry("notes.txt", false, Some(12)),
            remote_entry("FAMILY%20BUDGET.csv", false, Some(9)),
        ];
        let indices = super::visible_remote_indices(&entries, "  family ").unwrap();
        assert_eq!(indices, vec![0, 2]);
        assert_eq!(entries[indices[0]].uri, "smb://host/share/Family%20Photos");
        assert_eq!(
            super::visible_remote_indices(&entries, "no match"),
            Some(Vec::<usize>::new())
        );
        assert!(super::visible_remote_indices(&entries, "  ").is_none());
        assert_eq!(
            super::visible_remote_indices(&entries, "notes"),
            Some(vec![1])
        );
    }

    #[test]
    fn remote_keyboard_navigation_only_visits_visible_search_results() {
        let entries = vec![
            remote_entry("alpha", true, None),
            remote_entry("file-one.txt", false, Some(1)),
            remote_entry("beta", true, None),
            remote_entry("file-two.txt", false, Some(2)),
        ];
        let first = super::next_remote_selection(&entries, "file", None, true).unwrap();
        assert_eq!(first, entries[1].uri);
        let second = super::next_remote_selection(&entries, "file", Some(&first), true).unwrap();
        assert_eq!(second, entries[3].uri);
        assert_eq!(
            super::next_remote_selection(&entries, "file", Some(&second), true),
            Some(second.clone())
        );
        assert_eq!(
            super::next_remote_selection(&entries, "file", Some(&second), false),
            Some(first)
        );
        assert_eq!(
            super::next_remote_selection(&entries, "file", Some(&entries[0].uri), false),
            Some(second)
        );
        assert_eq!(
            super::next_remote_selection(&entries, "missing", None, true),
            None
        );
    }

    #[test]
    fn a_late_external_theme_result_cannot_replace_a_newer_theme_selection() {
        let path = PathBuf::from("/tmp/ricefield-theme-test.toml");
        let result = super::ThemeReloadResult {
            generation: 42,
            path: path.clone(),
            fingerprint: None,
            theme: Ok(None),
        };
        assert!(super::current_theme_reload(
            Some(path.as_path()),
            Some(42),
            &result
        ));
        assert!(!super::current_theme_reload(None, Some(42), &result));
        assert!(!super::current_theme_reload(
            Some(path.as_path()),
            Some(43),
            &result
        ));
        assert!(!super::current_theme_reload(
            Some(std::path::Path::new("/tmp/new-theme.toml")),
            Some(42),
            &result
        ));
    }

    #[test]
    fn external_theme_bootstraps_without_reading_custom_file() {
        let (theme, status) = super::starting_theme("win7", true);
        assert_eq!(theme.id, "win7");
        assert!(matches!(status, Some((message, false)) if message.contains("background")));
        let (fallback, status) = super::starting_theme("user-custom-theme", true);
        assert_eq!(fallback.id, "win7");
        assert!(matches!(status, Some((message, false)) if message.contains("background")));
        let (_, missing) = super::starting_theme("unknown", false);
        assert!(matches!(missing, Some((message, true)) if message.contains("Unknown")));
    }

    #[test]
    fn restored_transfer_history_is_read_only_and_never_exposes_undo() {
        let record = transfer_history::Record {
            id: 19,
            label: "Copy 2 items".into(),
            summary: "1 complete, 1 failed".into(),
            outcome: transfer_history::Outcome::Failed,
            finished_unix_seconds: 1_784_000_000,
            elapsed_millis: 900,
            counts: transfer_history::Counts {
                completed: 1,
                failed: 1,
                ..Default::default()
            },
            items: Vec::new(),
        };
        let history = super::CompletedOperation::from_history(record.clone());
        assert!(history.from_history);
        assert!(history.rename_undo.is_none());
        assert!(history.batch_rename_undo.is_none());
        assert!(history.items.is_empty());
        assert_eq!(history.history_record(), record);
    }

    #[test]
    fn transfer_history_prioritizes_failures_and_restores_only_read_only_evidence() {
        let items: Vec<_> = (0..42)
            .map(|index| ItemResult {
                source: PathBuf::from(format!("/tmp/source-{index}")),
                target: Some(PathBuf::from(format!("/tmp/target-{index}"))),
                state: if index == 41 {
                    ItemState::Failed
                } else {
                    ItemState::Completed
                },
                detail: (index == 41).then(|| "Permission denied".into()),
            })
            .collect();
        let captured = super::history_items(&items);
        assert_eq!(captured.len(), transfer_history::MAX_ITEMS_PER_RECORD);
        assert_eq!(captured[0].state, transfer_history::ItemOutcome::Failed);
        assert_eq!(captured[0].source, "/tmp/source-41");
        let record = transfer_history::Record {
            id: 42,
            label: "Copy 42 items".into(),
            summary: "41 complete, 1 failed".into(),
            outcome: transfer_history::Outcome::Failed,
            finished_unix_seconds: 1_784_000_000,
            elapsed_millis: 300,
            counts: transfer_history::Counts {
                completed: 41,
                failed: 1,
                ..Default::default()
            },
            items: captured,
        };
        let restored = super::CompletedOperation::from_history(record);
        assert!(restored.from_history);
        assert!(restored.rename_undo.is_none());
        assert!(restored.batch_rename_undo.is_none());
        assert_eq!(restored.items.len(), transfer_history::MAX_ITEMS_PER_RECORD);
        assert_eq!(restored.items[0].state, ItemState::Failed);
        assert_eq!(
            restored.items[0].detail.as_deref(),
            Some("Permission denied")
        );
    }

    #[test]
    fn shortcuts_keep_press_time_modifiers_when_a_chord_is_released_in_one_frame() {
        let context = eframe::egui::Context::default();
        let command = eframe::egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        let input = eframe::egui::RawInput {
            events: vec![
                eframe::egui::Event::ModifiersChanged(command),
                eframe::egui::Event::Key {
                    key: eframe::egui::Key::N,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: command,
                },
                eframe::egui::Event::Key {
                    key: eframe::egui::Key::N,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: command,
                },
                // The modifier has been released by the end of the frame.
                eframe::egui::Event::ModifiersChanged(Default::default()),
            ],
            ..Default::default()
        };
        context
            .run_pass(input, |context| {
                assert!(!context.input(|input| input.modifiers.command));
                assert_eq!(
                    super::pressed_keys(context),
                    vec![(eframe::egui::Key::N, command)]
                );
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn changing_filters_never_leaves_hidden_files_selected_for_mutation() {
        let mut tab = Tab::new(1, PathBuf::from("/example"));
        for name in ["hidden.txt", "visible.png"] {
            let path = tab.path.join(name);
            tab.selected.insert(path.clone());
            tab.entries.push(crate::filesystem::Entry {
                name: name.into(),
                path,
                is_dir: false,
                is_symlink: false,
                size: 5,
                modified: None,
            });
        }
        tab.visible = vec![1];
        retain_visible_selection(&mut tab);
        assert_eq!(tab.selected.len(), 1);
        assert!(tab.selected.contains(&tab.entries[1].path));
        assert!(!tab.selected.contains(&tab.entries[0].path));
    }

    #[test]
    fn split_view_resolves_stable_ids_after_tab_reordering_and_closure() {
        let mut tabs = vec![
            Tab::new(11, "/first".into()),
            Tab::new(22, "/second".into()),
            Tab::new(33, "/third".into()),
        ];
        assert_eq!(resolve_split_tabs(&tabs, Some((11, 33))), Some((0, 2)));
        tabs.swap(0, 2);
        assert_eq!(resolve_split_tabs(&tabs, Some((11, 33))), Some((2, 0)));
        assert_eq!(resolve_split_tabs(&tabs, Some((11, 11))), None);
        tabs.retain(|tab| tab.id != 33);
        assert_eq!(resolve_split_tabs(&tabs, Some((11, 33))), None);
    }

    #[test]
    fn duplicated_tab_retains_navigation_but_isolates_background_work() {
        let mut original = Tab::new(3, PathBuf::from("/offline"));
        original.visit(Location::parse("smb://nas/secret".into()).unwrap());
        original
            .back
            .push(Location::Local(PathBuf::from("/history")));
        original
            .forward
            .push(Location::Local(PathBuf::from("/forward")));
        original.query = "draft".into();
        original.recursive_search = true;
        original.content_search = true;
        original.generation = 5;
        original.selected.insert(PathBuf::from("/old-selection"));

        let duplicate = TabSnapshot::capture(&original).restore(19);
        assert_eq!(duplicate.id, 19);
        assert_eq!(duplicate.location(), original.location());
        assert_eq!(duplicate.back, original.back);
        assert_eq!(duplicate.forward, original.forward);
        assert_eq!(duplicate.query, "draft");
        assert!(duplicate.recursive_search);
        assert!(duplicate.content_search);
        assert!(duplicate.entries.is_empty());
        assert!(duplicate.selected.is_empty());
        assert_eq!(duplicate.generation, 0);
        assert!(!Arc::ptr_eq(&duplicate.scan_cancel, &original.scan_cancel));
        assert!(!Arc::ptr_eq(
            &duplicate.remote_preview_cancel,
            &original.remote_preview_cancel
        ));
        original.cancel_scan();
        assert!(!duplicate.scan_cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn tab_snapshot_preserves_non_utf8_local_paths_without_serializing_them() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/\xff".to_vec()));
        let original = Tab::new(1, path.clone());
        let restored = TabSnapshot::capture(&original).restore(2);
        assert_eq!(restored.path, path);
        assert_eq!(restored.location(), original.location());
    }

    #[test]
    fn swapping_tabs_keeps_active_identity_including_inactive_moves() {
        assert_eq!(active_after_swap(0, 0, 1), 1);
        assert_eq!(active_after_swap(1, 0, 1), 0);
        assert_eq!(active_after_swap(2, 0, 1), 2);
        assert_eq!(active_after_swap(2, 1, 3), 2);
        assert_eq!(active_after_swap(3, 1, 3), 1);
    }

    #[test]
    fn remote_tab_scan_tokens_reject_superseded_results() {
        let mut tab = Tab::new(5, PathBuf::from("/local"));
        tab.remote_uri = Some("smb://nas/share".into());
        tab.generation = 1;
        let old = tab.next_scan_token();
        tab.generation = 2;
        assert!(tab.accepts_scan_result(5, 2));
        let _new = tab.next_scan_token();
        tab.generation = 3;
        assert!(old.load(Ordering::Relaxed));
        assert!(!tab.accepts_scan_result(5, 2));
        assert!(!tab.accepts_scan_result(4, 2));
    }

    #[test]
    fn remote_preview_results_cannot_survive_new_selection_or_navigation() {
        let mut tab = Tab::new(5, PathBuf::from("/local"));
        tab.visit(Location::parse("smb://nas/share".into()).unwrap());
        let first = tab.next_remote_preview_token("smb://nas/share/a.txt".into());
        let generation = tab.remote_preview_generation;
        assert!(tab.accepts_remote_preview(5, generation, "smb://nas/share/a.txt"));
        assert!(!tab.accepts_remote_preview(7, generation, "smb://nas/share/a.txt"));
        let second = tab.next_remote_preview_token("smb://nas/share/b.txt".into());
        assert!(first.load(Ordering::Relaxed));
        assert!(!tab.accepts_remote_preview(5, generation, "smb://nas/share/a.txt"));
        tab.visit(Location::Local("/another".into()));
        assert!(second.load(Ordering::Relaxed));
        assert!(tab.remote_preview_uri.is_none());
    }

    #[test]
    fn a_new_network_tab_does_not_inherit_local_history_or_selection() {
        let mut tab = Tab::new(42, PathBuf::from("/local-fallback"));
        tab.set_location(Location::parse("smb://nas/share/Documents".into()).unwrap());
        assert_eq!(tab.remote_uri.as_deref(), Some("smb://nas/share/Documents"));
        assert!(tab.back.is_empty());
        assert!(tab.forward.is_empty());
        assert!(tab.selected.is_empty());
        assert!(tab.entries.is_empty());
        assert_eq!(tab.path, PathBuf::from("/local-fallback"));
    }

    #[test]
    fn one_history_tracks_local_and_network_steps_in_order() {
        let mut tab = Tab::new(5, PathBuf::from("/local"));
        let network = Location::parse("smb://nas/share".into()).unwrap();
        tab.visit(network.clone());
        tab.visit(Location::Local("/elsewhere".into()));
        assert_eq!(tab.location(), Location::Local("/elsewhere".into()));

        assert!(tab.travel(true));
        assert_eq!(tab.location(), network);
        assert!(tab.travel(true));
        assert_eq!(tab.location(), Location::Local("/local".into()));
        assert!(!tab.travel(true));
        assert!(tab.travel(false));
        assert!(tab.remote_uri.is_some());
        tab.visit(Location::Local("/new".into()));
        assert!(!tab.travel(false));
        assert_eq!(tab.location(), Location::Local("/new".into()));
    }

    #[test]
    fn properties_snapshot_reports_file_metadata_without_ui_state() {
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let snapshot = read_property_snapshot(&file).unwrap();
        assert!(!snapshot.is_symlink);
        assert!(snapshot.lines.iter().any(|(key, _)| key == "Type"));
        assert!(snapshot.lines.iter().any(|(key, _)| key == "Size"));
        #[cfg(unix)]
        assert_eq!(snapshot.mode.len(), 3);
    }

    #[test]
    fn property_dialog_ignores_superseded_and_other_path_results() {
        let path = PathBuf::from("/selected/file");
        let mut dialog = Some(Dialog::Properties {
            path: path.clone(),
            mode: String::new(),
            owner: String::new(),
            group: String::new(),
            attribute_name: "user.".into(),
            attribute_value: String::new(),
            generation: 8,
            details: None,
            analysis: None,
        });
        let result = || PropertySnapshot {
            lines: vec![("Type".into(), "File".into())],
            metadata: std::fs::metadata(std::env::temp_dir()).unwrap(),
            mode: "644".into(),
            owner: "1000".into(),
            group: "1000".into(),
            is_symlink: false,
            is_dir: false,
        };
        accept_property_result(
            &mut dialog,
            PropertyResult {
                generation: 7,
                path: path.clone(),
                result: Ok(result()),
            },
        );
        accept_property_result(
            &mut dialog,
            PropertyResult {
                generation: 8,
                path: PathBuf::from("/another/file"),
                result: Ok(result()),
            },
        );
        assert!(matches!(
            &dialog,
            Some(Dialog::Properties { details: None, mode, .. }) if mode.is_empty()
        ));
        accept_property_result(
            &mut dialog,
            PropertyResult {
                generation: 8,
                path,
                result: Ok(result()),
            },
        );
        assert!(matches!(
            &dialog,
            Some(Dialog::Properties { details: Some(Ok(_)), mode, .. }) if mode == "644"
        ));
    }

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
    fn source_retained_copy_consumes_cut_intent_without_hiding_failed_items() {
        let copied = ItemResult {
            source: PathBuf::from("/original/kept.txt"),
            target: Some(PathBuf::from("/destination/kept.txt")),
            state: ItemState::CopiedSourceRetained,
            detail: Some("Original retained".into()),
        };
        let failed = ItemResult {
            source: PathBuf::from("/original/failed.txt"),
            target: None,
            state: ItemState::Failed,
            detail: None,
        };
        assert_eq!(
            remaining_cut_paths(std::slice::from_ref(&copied)),
            Some(vec![])
        );
        assert_eq!(
            remaining_cut_paths(&[copied.clone(), failed]),
            Some(vec![PathBuf::from("/original/failed.txt")])
        );
        assert_eq!(ItemCounts::from_items(&[copied]).retained, 1);
    }

    #[test]
    fn stale_desktop_cut_cannot_replay_completed_sources() {
        let original = vec![
            PathBuf::from("/source/moved"),
            PathBuf::from("/source/pending"),
        ];
        let internal = Clipboard {
            paths: vec![original[1].clone()],
            cut: true,
        };
        let chosen = choose_paste_clipboard(
            Some(clipboard::Files {
                paths: original.clone(),
                cut: true,
            }),
            Some(internal),
            Some(&original),
        )
        .unwrap();
        assert_eq!(chosen.paths, vec![original[1].clone()]);
        let fresh = choose_paste_clipboard(
            Some(clipboard::Files {
                paths: vec![PathBuf::from("/external/new")],
                cut: false,
            }),
            Some(chosen),
            Some(&original),
        )
        .unwrap();
        assert_eq!(fresh.paths, vec![PathBuf::from("/external/new")]);
        assert!(!fresh.cut);
    }

    #[test]
    fn delayed_paste_cannot_target_an_old_tab_or_expired_read() {
        let now = Instant::now();
        let mut tab = Tab::new(5, PathBuf::from("/initial"));
        let pending = PendingPaste {
            id: 1,
            revision: 9,
            tab_id: tab.id,
            tab_generation: tab.generation,
            destination: tab.path.clone(),
            mode: PasteMode::Immediate,
            started: now,
        };
        assert!(pending.is_current(9, &tab, now));
        assert!(!pending.is_current(9, &tab, now + CLIPBOARD_WAIT));
        assert!(!pending.is_current(10, &tab, now));
        tab.generation += 1;
        assert!(!pending.is_current(9, &tab, now));
        tab.generation -= 1;
        tab.path = PathBuf::from("/other");
        assert!(!pending.is_current(9, &tab, now));
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

    #[test]
    fn item_progress_uses_counts_not_bytes_or_byte_eta() {
        for phase in ["Trashing", "Deleting", "Renaming", "Linking"] {
            let progress = Progress {
                transferred: 2,
                total: 3,
                current: PathBuf::from("/file"),
                phase,
            };
            let details = transfer_progress_details(&progress, Duration::from_secs(15));
            assert!(details.contains("2 / 3 items"));
            assert!(details.contains("15s elapsed"));
            assert!(!details.contains("/s avg"));
            assert!(!details.contains("remaining"));
        }
    }
}
