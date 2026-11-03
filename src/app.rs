use crate::{
    config::{self, Config},
    filesystem::{self, Action, Entry},
    theme::{BUILTIN_IDS, Layout, Theme},
};
use eframe::egui::{
    self, Align2, Color32, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

struct Tab {
    id: u64,
    path: PathBuf,
    address: String,
    query: String,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    entries: Vec<Entry>,
    visible: Vec<usize>,
    selected: BTreeSet<PathBuf>,
    last_selected: Option<usize>,
    generation: u64,
    loading: bool,
    error: Option<String>,
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
            visible: Vec::new(),
            selected: BTreeSet::new(),
            last_selected: None,
            generation: 0,
            loading: false,
            error: None,
        }
    }
}

struct ScanResult {
    tab: u64,
    generation: u64,
    result: std::io::Result<Vec<Entry>>,
}

struct OperationResult {
    result: std::io::Result<String>,
    clear_clipboard: bool,
}

#[derive(Clone)]
struct Clipboard {
    paths: Vec<PathBuf>,
    cut: bool,
}

enum NameMode {
    Folder,
    File,
    Rename(PathBuf),
}

enum Dialog {
    Name { mode: NameMode, value: String },
    Trash(Vec<PathBuf>),
}

pub struct Explorer {
    config: Config,
    theme: Theme,
    tabs: Vec<Tab>,
    active: usize,
    next_tab: u64,
    scan_sender: Sender<ScanResult>,
    scan_receiver: Receiver<ScanResult>,
    op_sender: Sender<OperationResult>,
    op_receiver: Receiver<OperationResult>,
    clipboard: Option<Clipboard>,
    dialog: Option<Dialog>,
    status: Option<(String, bool)>,
    busy: bool,
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
        let (op_sender, op_receiver) = mpsc::channel();
        let mut explorer = Self {
            config,
            theme,
            tabs: Vec::new(),
            active: 0,
            next_tab: 1,
            scan_sender,
            scan_receiver,
            op_sender,
            op_receiver,
            clipboard: None,
            dialog: None,
            status,
            busy: false,
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
        explorer
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
        let sender = self.scan_sender.clone();
        thread::spawn(move || {
            let _ = sender.send(ScanResult {
                tab: task.0,
                generation: task.1,
                result: filesystem::scan(&task.2),
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
                    && entry.name.to_lowercase().contains(&search)
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
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            let ordered = if config.descending {
                comparison.reverse()
            } else {
                comparison
            };
            ordered.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        tab.last_selected = None;
    }

    fn poll_background(&mut self) {
        while let Ok(message) = self.scan_receiver.try_recv() {
            if let Some(index) = self
                .tabs
                .iter()
                .position(|tab| tab.id == message.tab && tab.generation == message.generation)
            {
                let tab = &mut self.tabs[index];
                tab.loading = false;
                match message.result {
                    Ok(entries) => {
                        tab.entries = entries;
                        tab.error = None;
                        tab.selected
                            .retain(|path| tab.entries.iter().any(|entry| &entry.path == path));
                        self.rebuild_visible(index);
                    }
                    Err(error) => {
                        tab.entries.clear();
                        tab.visible.clear();
                        tab.error = Some(error.to_string());
                    }
                }
            }
        }
        while let Ok(message) = self.op_receiver.try_recv() {
            self.busy = false;
            match message.result {
                Ok(success) => {
                    self.status = Some((success, false));
                    if message.clear_clipboard {
                        self.clipboard = None;
                    }
                }
                Err(error) => self.status = Some((format!("Operation failed: {error}"), true)),
            }
            self.refresh_all();
        }
    }

    fn run_action(&mut self, action: Action) {
        if self.busy {
            self.status = Some(("Wait for the current operation to finish".into(), true));
            return;
        }
        self.busy = true;
        let clear_clipboard = matches!(action, Action::Move { .. });
        let sender = self.op_sender.clone();
        thread::spawn(move || {
            let result = filesystem::perform(&action);
            let _ = sender.send(OperationResult {
                result,
                clear_clipboard,
            });
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
        self.clipboard = Some(Clipboard {
            paths: paths.clone(),
            cut,
        });
        self.status = Some((
            format!(
                "{} {} item(s)",
                if cut { "Cut" } else { "Copied" },
                paths.len()
            ),
            false,
        ));
    }

    fn paste(&mut self) {
        if let Some(clipboard) = self.clipboard.clone() {
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
            )
        });
        let (ctrl, alt, l, f, t, w, c, x, v, h, r, n, f2, delete, left, right, up) = keys;
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
            if ctrl && c {
                self.copy_selection(false);
            }
            if ctrl && x {
                self.copy_selection(true);
            }
            if ctrl && v {
                self.paste();
            }
            if ctrl && n {
                self.dialog = Some(Dialog::Name {
                    mode: NameMode::Folder,
                    value: String::new(),
                });
            }
            if f2 {
                self.rename_selected();
            }
            if delete {
                self.ask_trash();
            }
            if context.input(|input| input.key_pressed(Key::Enter))
                && self.current().selected.len() == 1
            {
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
                self.rebuild_visible(index);
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

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        egui::Panel::top("chrome")
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.chrome())
                    .inner_margin(egui::Margin::same(7)),
            )
            .show(ui, |ui| {
                if theme.layout == Layout::Classic {
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
                                    mode: NameMode::Folder,
                                    value: String::new(),
                                });
                                ui.close();
                            }
                            if ui.button("New empty file").clicked() {
                                self.dialog = Some(Dialog::Name {
                                    mode: NameMode::File,
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
                                    self.rebuild_visible(index);
                                }
                                self.dirty_config = true;
                                ui.close();
                            }
                        });
                    });
                    ui.separator();
                }
                self.tabs_bar(ui);
                ui.add_space(4.0);
                self.navigation_bar(ui);
                if theme.layout == Layout::Aero {
                    ui.add_space(3.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("ORGANIZE")
                                .small()
                                .strong()
                                .color(theme.palette.text()),
                        );
                        ui.separator();
                        if ui.small_button("New folder").clicked() {
                            self.dialog = Some(Dialog::Name {
                                mode: NameMode::Folder,
                                value: String::new(),
                            });
                        }
                        if ui.small_button("Add to favorites").clicked() {
                            self.toggle_bookmark();
                        }
                        if ui.small_button("Refresh").clicked() {
                            self.scan(self.active);
                        }
                    });
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
                    .add(egui::Button::new(format!("▣  {label}")).selected(selected))
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
                .add_enabled(!self.current().back.is_empty(), egui::Button::new("◀"))
                .on_hover_text("Back (Alt+Left)")
                .clicked()
            {
                self.history(true);
            }
            if ui
                .add_enabled(!self.current().forward.is_empty(), egui::Button::new("▶"))
                .on_hover_text("Forward (Alt+Right)")
                .clicked()
            {
                self.history(false);
            }
            if ui
                .button("↑")
                .on_hover_text("Parent folder (Alt+Up)")
                .clicked()
            {
                self.parent();
            }
            if ui.button("⌂").on_hover_text("Home").clicked() {
                self.navigate(config::home());
            }

            let search_width = if ui.available_width() < 620.0 {
                100.0
            } else {
                190.0
            };
            let address_width = (ui.available_width() - search_width - 174.0).max(105.0);
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
            if ui.button("↻").on_hover_text("Refresh (Ctrl+R)").clicked() {
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
                self.rebuild_visible(self.active);
            }
            if !self.current().query.is_empty()
                && ui.small_button("×").on_hover_text("Clear search").clicked()
            {
                self.current_mut().query.clear();
                self.rebuild_visible(self.active);
                ui.memory_mut(|memory| memory.request_focus(search.id));
            }
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        if !self.config.show_sidebar {
            return;
        }
        let theme = self.theme.clone();
        egui::Panel::left("places")
            .resizable(true)
            .default_size(theme.metrics.sidebar_width)
            .min_size(132.0)
            .frame(
                egui::Frame::new()
                    .fill(theme.palette.sidebar())
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("PLACES")
                        .small()
                        .strong()
                        .color(theme.palette.muted()),
                );
                ui.add_space(8.0);
                let home = config::home();
                for (label, path) in [
                    ("⌂  Home", home.clone()),
                    ("▤  Desktop", home.join("Desktop")),
                    ("▤  Documents", home.join("Documents")),
                    ("↓  Downloads", home.join("Downloads")),
                    ("♫  Music", home.join("Music")),
                    ("▧  Pictures", home.join("Pictures")),
                    ("▣  Videos", home.join("Videos")),
                    ("▣  Filesystem", PathBuf::from("/")),
                ] {
                    if path.is_dir()
                        && ui
                            .add_sized(
                                [ui.available_width(), 29.0],
                                egui::Button::new(label).selected(self.current().path == path),
                            )
                            .clicked()
                    {
                        self.navigate(path);
                    }
                }
                ui.add_space(14.0);
                ui.separator();
                ui.label(
                    egui::RichText::new("FAVORITES")
                        .small()
                        .strong()
                        .color(theme.palette.muted()),
                );
                let bookmarks = self.config.bookmarks.clone();
                for path in bookmarks {
                    let label = path.file_name().unwrap_or_default().to_string_lossy();
                    if ui
                        .add_sized(
                            [ui.available_width(), 29.0],
                            egui::Button::new(format!("★  {label}"))
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
                            .small_button("☰")
                            .on_hover_text("Toggle sidebar")
                            .clicked()
                        {
                            self.config.show_sidebar = !self.config.show_sidebar;
                            self.dirty_config = true;
                        }
                        if ui
                            .add(egui::Button::new("▦").selected(self.config.view == "grid"))
                            .on_hover_text("Icon view")
                            .clicked()
                        {
                            self.config.view = "grid".into();
                            self.dirty_config = true;
                        }
                        if ui
                            .add(egui::Button::new("≡").selected(self.config.view == "list"))
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
                        .small_button(if self.config.descending { "↓" } else { "↑" })
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
                        .small_button(if self.config.show_hidden {
                            "● Hidden"
                        } else {
                            "○ Hidden"
                        })
                        .on_hover_text("Toggle hidden files (Ctrl+H)")
                        .clicked()
                    {
                        self.config.show_hidden = !self.config.show_hidden;
                        self.dirty_config = true;
                        for i in 0..self.tabs.len() {
                            self.rebuild_visible(i);
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
            if ui.button("Move to Trash").clicked() {
                if !self.current().selected.contains(&entry.path) {
                    self.select_entry(entry, index, Modifiers::default());
                }
                self.ask_trash();
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
                        ui.label("Working…");
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
        let current_path = self.current().path.clone();
        let mut action = None;
        let mut dismiss = false;
        if let Some(dialog) = self.dialog.as_mut() {
            match dialog {
                Dialog::Name { mode, value } => {
                    let title = match mode {
                        NameMode::Folder => "New folder",
                        NameMode::File => "New empty file",
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
                                    action = Some(match mode {
                                        NameMode::Folder => Action::NewFolder {
                                            parent: current_path.clone(),
                                            name,
                                        },
                                        NameMode::File => Action::NewFile {
                                            parent: current_path.clone(),
                                            name,
                                        },
                                        NameMode::Rename(source) => Action::Rename {
                                            source: source.clone(),
                                            name,
                                        },
                                    });
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
        self.poll_background();
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
        self.status_bar(ui);
        self.sidebar(ui);
        self.main_panel(ui);
        self.dialogs(&context);
        if self.dirty_config {
            if let Err(error) = self.config.save() {
                self.status = Some((format!("Cannot save preferences: {error}"), true));
            }
            self.dirty_config = false;
        }
        if self.busy || self.tabs.iter().any(|tab| tab.loading) {
            context.request_repaint_after(Duration::from_millis(90));
        }
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
            "Audio" => "♪",
            "Video" => "VID",
            "Archive" => "ZIP",
            "PDF" => "PDF",
            "Code" => "</>",
            "Link" => "↗",
            _ => "—",
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
