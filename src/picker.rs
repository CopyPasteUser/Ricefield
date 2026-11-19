//! Standalone RiceField file picker. The portal invokes this as a short-lived,
//! isolated UI process and exchanges bounded TOML over stdin/stdout.
//! It never writes selected files or changes Explorer's saved sessions.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    io::{self, Read},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Debug, Clone, Deserialize)]
struct Filter {
    name: String,
    #[serde(default)]
    rules: Vec<(u32, String)>,
}

#[derive(Debug, Clone, Deserialize)]
struct Choice {
    id: String,
    label: String,
    #[serde(default)]
    options: Vec<(String, String)>,
    selected: String,
}

#[derive(Debug, Deserialize)]
struct Request {
    mode: String,
    title: String,
    #[serde(default)]
    accept_label: String,
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    directory: bool,
    #[serde(default)]
    folder: String,
    #[serde(default)]
    current_name: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    filters: Vec<Filter>,
    #[serde(default)]
    selected_filter: Option<usize>,
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Serialize)]
struct Selection {
    id: String,
    value: String,
}

#[derive(Serialize)]
struct Reply {
    uris: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_filter: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    selections: Vec<Selection>,
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    directory: bool,
    name: String,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && name.len() <= 255
}

fn file_uri(path: &Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err("File picker requires an absolute path".into());
    }
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(*byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(uri)
}

fn initial_folder(uri: &str) -> PathBuf {
    let parsed = crate::location::Location::parse(uri.into());
    let candidate = match parsed {
        Ok(crate::location::Location::Local(path)) if path.is_absolute() => path,
        _ => PathBuf::new(),
    };
    if candidate.is_dir() {
        candidate
    } else {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .unwrap_or_else(|| PathBuf::from("/"))
    }
}

// Match the common GPattern-compatible *, ? patterns. Complex POSIX brackets
// are left unmatched rather than silently widening a restrictive file filter.
fn wildcard(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.as_bytes(), name.as_bytes());
    let (mut i, mut j, mut star, mut after_star) = (0, 0, None, 0);
    while j < n.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            after_star = j;
        } else if let Some(last_star) = star {
            i = last_star + 1;
            after_star += 1;
            j = after_star;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}

fn mime_globs() -> &'static HashMap<String, Vec<String>> {
    static GLOBS: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    GLOBS.get_or_init(|| {
        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for source in ["/usr/share/mime/globs2", "/usr/local/share/mime/globs2"] {
            let Ok(data) = std::fs::read_to_string(source) else {
                continue;
            };
            for line in data.lines() {
                let mut parts = line.splitn(4, ':');
                if let (Some(_weight), Some(mime), Some(pattern)) =
                    (parts.next(), parts.next(), parts.next())
                {
                    result
                        .entry(mime.to_owned())
                        .or_default()
                        .push(pattern.to_owned());
                }
            }
        }
        result
    })
}

fn matches_filter(name: &str, filter: &Filter) -> bool {
    if filter.rules.is_empty() {
        return true;
    }
    filter.rules.iter().any(|(kind, value)| match kind {
        0 => wildcard(value, name),
        1 => mime_globs().iter().any(|(mime, globs)| {
            (mime == value
                || value
                    .strip_suffix("/*")
                    .is_some_and(|family| mime.starts_with(&format!("{family}/"))))
                && globs.iter().any(|pattern| wildcard(pattern, name))
        }),
        _ => false,
    })
}

fn validate_request(request: &Request) -> Result<(), String> {
    if !matches!(request.mode.as_str(), "open" | "save" | "save_files") {
        return Err("Unknown picker mode".into());
    }
    if request.filters.len() > 64 || request.choices.len() > 32 {
        return Err("Too many picker controls".into());
    }
    if request.mode == "save_files"
        && (request.files.is_empty()
            || request.files.len() > 128
            || request.files.iter().any(|name| !valid_name(name)))
    {
        return Err("Invalid SaveFiles filenames".into());
    }
    Ok(())
}

struct Picker {
    request: Request,
    folder: PathBuf,
    address: String,
    entries: Vec<Entry>,
    selected: BTreeSet<PathBuf>,
    file_name: String,
    show_hidden: bool,
    current_filter: usize,
    choice_values: Vec<String>,
    message: String,
    overwrite_pending: Option<PathBuf>,
}

impl Picker {
    fn new(request: Request) -> Self {
        let folder = if request.folder.is_empty() {
            initial_folder("/")
        } else {
            initial_folder(&request.folder)
        };
        let current_filter = request
            .selected_filter
            .unwrap_or(0)
            .min(request.filters.len().saturating_sub(1));
        let choice_values = request
            .choices
            .iter()
            .map(|choice| {
                if choice.options.is_empty() && choice.selected.is_empty() {
                    "false".to_owned()
                } else {
                    choice.selected.clone()
                }
            })
            .collect();
        let file_name = request.current_name.clone();
        let mut picker = Self {
            request,
            address: folder.to_string_lossy().into_owned(),
            folder,
            entries: Vec::new(),
            selected: BTreeSet::new(),
            file_name,
            show_hidden: false,
            current_filter,
            choice_values,
            message: String::new(),
            overwrite_pending: None,
        };
        picker.refresh();
        picker
    }

    fn folder_mode(&self) -> bool {
        self.request.directory || self.request.mode == "save_files"
    }

    fn navigate(&mut self, path: PathBuf) {
        if !path.is_dir() {
            self.message = "Folder is not accessible".into();
            return;
        }
        self.folder = path;
        self.address = self.folder.to_string_lossy().into_owned();
        self.selected.clear();
        self.overwrite_pending = None;
        self.message.clear();
        self.refresh();
    }

    fn refresh(&mut self) {
        self.entries.clear();
        match std::fs::read_dir(&self.folder) {
            Ok(items) => {
                for item in items.take(5000) {
                    let Ok(item) = item else { continue };
                    let path = item.path();
                    let name = item.file_name().to_string_lossy().into_owned();
                    if !self.show_hidden && name.starts_with('.') {
                        continue;
                    }
                    let Ok(kind) = item.file_type() else {
                        continue;
                    };
                    let directory = kind.is_dir() || (kind.is_symlink() && path.is_dir());
                    if !directory && !kind.is_file() && !kind.is_symlink() {
                        continue;
                    }
                    if !directory && self.folder_mode() {
                        continue;
                    }
                    if !directory
                        && !self.request.filters.is_empty()
                        && !matches_filter(&name, &self.request.filters[self.current_filter])
                    {
                        continue;
                    }
                    self.entries.push(Entry {
                        path,
                        directory,
                        name,
                    });
                }
                self.entries.sort_by(|a, b| {
                    b.directory
                        .cmp(&a.directory)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            Err(error) => self.message = format!("Cannot list directory: {error}"),
        }
    }

    fn choose(&mut self, ctx: &egui::Context) {
        match self.build_reply() {
            Ok(reply) => {
                if let Ok(text) = toml::to_string(&reply) {
                    print!("{text}");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.message = "Cannot encode selection".into();
                }
            }
            Err(error) => self.message = error,
        }
    }

    fn build_reply(&mut self) -> Result<Reply, String> {
        let paths = match self.request.mode.as_str() {
            "save" => {
                if !valid_name(&self.file_name) {
                    return Err("Enter a valid filename".into());
                }
                let path = self.folder.join(&self.file_name);
                if path.exists() && path.is_dir() {
                    return Err("Cannot overwrite a directory".into());
                }
                if path.exists() && self.overwrite_pending.as_ref() != Some(&path) {
                    self.overwrite_pending = Some(path);
                    return Err(
                        "File exists. Click Replace to confirm (no file is changed yet).".into(),
                    );
                }
                vec![path]
            }
            "save_files" => {
                let destination = self
                    .selected
                    .iter()
                    .next()
                    .filter(|path| path.is_dir())
                    .unwrap_or(&self.folder);
                if !destination.is_dir() {
                    return Err("Choose a valid destination folder".into());
                }
                let paths = self
                    .request
                    .files
                    .iter()
                    .map(|name| destination.join(name))
                    .collect::<Vec<_>>();
                if paths.iter().any(|path| path.exists()) {
                    return Err(
                        "Destination contains a requested filename. Choose another folder.".into(),
                    );
                }
                paths
            }
            _ if self.folder_mode() => {
                if self.selected.is_empty() {
                    vec![self.folder.clone()]
                } else {
                    self.selected
                        .iter()
                        .filter(|path| path.is_dir())
                        .cloned()
                        .collect()
                }
            }
            _ => self
                .selected
                .iter()
                .filter(|path| path.is_file())
                .cloned()
                .collect(),
        };
        if paths.is_empty() {
            return Err("Choose at least one file".into());
        }
        if !self.request.multiple && self.request.mode == "open" && paths.len() != 1 {
            return Err("Select one item".into());
        }
        let uris = paths
            .iter()
            .map(|path| file_uri(path))
            .collect::<Result<Vec<_>, _>>()?;
        let selections = self
            .request
            .choices
            .iter()
            .zip(&self.choice_values)
            .map(|(choice, value)| Selection {
                id: choice.id.clone(),
                value: value.clone(),
            })
            .collect();
        Ok(Reply {
            uris,
            selected_filter: (!self.request.filters.is_empty()).then_some(self.current_filter),
            selections,
        })
    }
}

// The portal picker is a transient dialog, not another Explorer window.
// Use a coherent Luna-inspired layout while leaving the portal wire protocol
// and the host compositor's actual window decorations untouched.
const PICKER_BLUE: egui::Color32 = egui::Color32::from_rgb(30, 76, 158);
const PICKER_NAVY: egui::Color32 = egui::Color32::from_rgb(22, 55, 118);
const PICKER_LINE: egui::Color32 = egui::Color32::from_rgb(187, 204, 230);
const PICKER_MUTED: egui::Color32 = egui::Color32::from_rgb(81, 101, 131);

fn picker_style(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.visuals = egui::Visuals::light();
    style.visuals.panel_fill = egui::Color32::from_rgb(236, 242, 252);
    style.visuals.window_fill = egui::Color32::from_rgb(236, 242, 252);
    style.visuals.selection.bg_fill = PICKER_BLUE;
    style.visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(206, 224, 251);
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(225, 236, 252);
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(9.0, 5.0);
    ctx.set_style_of(egui::Theme::Light, style);
    ctx.set_theme(egui::Theme::Light);
}

impl Picker {
    fn paint_header(&self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(PICKER_BLUE)
            .inner_margin(egui::Margin::symmetric(16, 13))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(if self.request.title.trim().is_empty() {
                        "Choose a file"
                    } else {
                        &self.request.title
                    })
                    .size(19.0)
                    .strong()
                    .color(egui::Color32::WHITE),
                );
                let hint = match self.request.mode.as_str() {
                    "save" => "Choose where to save your file",
                    "save_files" => "Choose a destination folder",
                    _ if self.folder_mode() => "Choose a folder",
                    _ if self.request.multiple => "Select one or more files",
                    _ => "Select a file to open",
                };
                ui.label(
                    egui::RichText::new(hint)
                        .size(12.0)
                        .color(egui::Color32::from_rgb(226, 237, 255)),
                );
            });
    }

    fn paint_navigation(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let parent = self.folder.parent().map(Path::to_path_buf);
            if ui
                .add_enabled(parent.is_some(), egui::Button::new("↑  Up"))
                .on_hover_text("Go to parent folder (Alt+Up)")
                .clicked()
                && let Some(path) = parent
            {
                self.navigate(path);
            }
            ui.label(egui::RichText::new("Location").color(PICKER_MUTED));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.address)
                    .desired_width((ui.available_width() - 113.0).max(100.0))
                    .id_salt("picker_location"),
            );
            let go = ui.button("Go").clicked();
            if go
                || (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
            {
                self.navigate(PathBuf::from(&self.address));
            }
            if ui.button("↻").on_hover_text("Refresh (F5)").clicked() {
                self.refresh();
            }
        });
    }

    fn paint_places(&mut self, ui: &mut egui::Ui, height: f32) {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(221, 233, 252))
            .stroke(egui::Stroke::new(1.0, PICKER_LINE))
            .inner_margin(egui::Margin::symmetric(10, 12))
            .show(ui, |ui| {
                ui.set_width(155.0);
                ui.set_min_height(height);
                ui.label(
                    egui::RichText::new("PLACES")
                        .size(11.0)
                        .strong()
                        .color(PICKER_NAVY),
                );
                ui.add_space(8.0);
                let home = std::env::var_os("HOME").map(PathBuf::from);
                let places = [
                    ("⌂  Home", home.clone()),
                    ("▣  Desktop", home.as_ref().map(|h| h.join("Desktop"))),
                    ("▤  Documents", home.as_ref().map(|h| h.join("Documents"))),
                    ("↓  Downloads", home.as_ref().map(|h| h.join("Downloads"))),
                    ("▧  Pictures", home.as_ref().map(|h| h.join("Pictures"))),
                    ("◫  Computer", Some(PathBuf::from("/"))),
                ];
                for (label, path) in places {
                    if let Some(path) = path
                        && path.is_dir()
                        && ui
                            .add_sized(
                                [ui.available_width(), 31.0],
                                egui::Button::new(label).selected(self.folder == path),
                            )
                            .clicked()
                    {
                        self.navigate(path);
                    }
                }
                ui.add_space(10.0);
                ui.separator();
                if ui
                    .checkbox(&mut self.show_hidden, "Show hidden files")
                    .changed()
                {
                    self.refresh();
                }
            });
    }

    fn paint_files(&mut self, ui: &mut egui::Ui, height: f32, ctx: &egui::Context) {
        // Defer actions until after the immutable rows are painted. A large
        // directory should never clone 5,000 entries on every render frame.
        let mut activated: Option<(PathBuf, bool)> = None;
        let mut clicked: Option<(PathBuf, bool, String, bool)> = None;
        egui::Frame::new()
            .fill(egui::Color32::WHITE)
            .stroke(egui::Stroke::new(1.0, PICKER_LINE))
            .inner_margin(egui::Margin::symmetric(10, 10))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("NAME")
                            .strong()
                            .size(11.0)
                            .color(PICKER_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{} items", self.entries.len()))
                                .size(11.0)
                                .color(PICKER_MUTED),
                        );
                    });
                });
                ui.separator();
                if self.entries.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("This folder is empty")
                                .size(15.0)
                                .color(PICKER_MUTED),
                        );
                        ui.label("Choose another location or adjust the file filter.");
                    });
                }
                egui::ScrollArea::vertical()
                    .max_height((height - 48.0).max(100.0))
                    .auto_shrink([false, false])
                    .show_rows(ui, 30.0, self.entries.len(), |ui, rows| {
                        for index in rows {
                            let entry = &self.entries[index];
                            let marked = self.selected.contains(&entry.path);
                            let symbol = if entry.directory { "📁" } else { "📄" };
                            let response = ui
                                .add_sized(
                                    [ui.available_width(), 28.0],
                                    egui::Button::new(
                                        egui::RichText::new(format!("{symbol}  {}", entry.name))
                                            .size(13.0),
                                    )
                                    .selected(marked)
                                    .truncate(),
                                )
                                .on_hover_text(entry.name.as_str());
                            if response.double_clicked() {
                                activated = Some((entry.path.clone(), entry.directory));
                            } else if response.clicked() {
                                clicked = Some((
                                    entry.path.clone(),
                                    entry.directory,
                                    entry.name.clone(),
                                    ui.input(|i| i.modifiers.ctrl),
                                ));
                            }
                        }
                    });
            });
        if let Some((path, directory)) = activated {
            if directory {
                self.navigate(path);
            } else {
                self.selected.clear();
                if self.request.mode == "save" {
                    self.file_name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.overwrite_pending = None;
                }
                self.selected.insert(path);
                if self.request.mode == "open" && !self.folder_mode() {
                    self.choose(ctx);
                }
            }
        } else if let Some((path, directory, name, ctrl)) = clicked {
            if directory && !self.folder_mode() {
                self.selected.clear();
            } else if self.request.multiple && ctrl {
                if !self.selected.remove(&path) {
                    self.selected.insert(path);
                }
            } else {
                self.selected.clear();
                self.selected.insert(path);
                if self.request.mode == "save" && !directory {
                    self.file_name = name;
                    self.overwrite_pending = None;
                }
            }
        }
    }

    fn paint_footer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(248, 250, 255))
            .stroke(egui::Stroke::new(1.0, PICKER_LINE))
            .inner_margin(egui::Margin::symmetric(13, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if self.request.mode == "save" {
                    ui.horizontal(|ui| {
                        ui.label("File name");
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.file_name)
                                    .desired_width(ui.available_width() - 12.0),
                            )
                            .changed()
                        {
                            self.overwrite_pending = None;
                        }
                    });
                }
                if !self.request.filters.is_empty() {
                    let mut next = self.current_filter;
                    egui::ComboBox::from_label("File type")
                        .selected_text(&self.request.filters[next].name)
                        .show_ui(ui, |ui| {
                            for (idx, filter) in self.request.filters.iter().enumerate() {
                                ui.selectable_value(&mut next, idx, &filter.name);
                            }
                        });
                    if next != self.current_filter {
                        self.current_filter = next;
                        self.selected.clear();
                        self.refresh();
                    }
                }
                if !self.request.choices.is_empty() {
                    egui::ScrollArea::vertical()
                        .max_height(105.0)
                        .show(ui, |ui| {
                            for (idx, choice) in self.request.choices.iter().enumerate() {
                                if choice.options.is_empty() {
                                    let mut selected = self.choice_values[idx] == "true";
                                    if ui.checkbox(&mut selected, &choice.label).changed() {
                                        self.choice_values[idx] = selected.to_string();
                                    }
                                } else {
                                    egui::ComboBox::from_label(&choice.label)
                                        .selected_text(
                                            choice
                                                .options
                                                .iter()
                                                .find(|(value, _)| {
                                                    value == &self.choice_values[idx]
                                                })
                                                .map(|(_, label)| label.as_str())
                                                .unwrap_or("Choose"),
                                        )
                                        .show_ui(ui, |ui| {
                                            for (value, label) in &choice.options {
                                                ui.selectable_value(
                                                    &mut self.choice_values[idx],
                                                    value.clone(),
                                                    label,
                                                );
                                            }
                                        });
                                }
                            }
                        });
                }
                if !self.message.is_empty() {
                    ui.label(
                        egui::RichText::new(&self.message)
                            .color(egui::Color32::from_rgb(165, 38, 38))
                            .strong(),
                    );
                }
                ui.separator();
                ui.horizontal(|ui| {
                    let summary = if self.selected.is_empty() {
                        if self.folder_mode() {
                            "Current folder"
                        } else {
                            "No file selected"
                        }
                    } else if self.selected.len() == 1 {
                        "1 item selected"
                    } else {
                        "Multiple items selected"
                    };
                    ui.label(egui::RichText::new(summary).size(12.0).color(PICKER_MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([90.0, 32.0], egui::Button::new("Cancel"))
                            .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        let label = if self.overwrite_pending.is_some() {
                            "Replace"
                        } else if !self.request.accept_label.is_empty() {
                            &self.request.accept_label
                        } else if self.request.mode == "open" {
                            "Open"
                        } else {
                            "Save"
                        };
                        if ui
                            .add_sized(
                                [100.0, 32.0],
                                egui::Button::new(
                                    egui::RichText::new(label)
                                        .color(egui::Color32::WHITE)
                                        .strong(),
                                )
                                .fill(PICKER_BLUE),
                            )
                            .clicked()
                        {
                            self.choose(ctx);
                        }
                    });
                });
            });
    }
}

impl eframe::App for Picker {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowUp))
            && let Some(parent) = self.folder.parent()
        {
            self.navigate(parent.to_path_buf());
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.refresh();
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::H)) {
            self.show_hidden = !self.show_hidden;
            self.refresh();
        }

        ui.vertical(|ui| {
            self.paint_header(ui);
            ui.add_space(7.0);
            self.paint_navigation(ui);
            ui.add_space(8.0);

            let footer_reserve = if self.request.mode == "save" {
                185.0
            } else {
                144.0
            };
            let height = (ui.available_height() - footer_reserve).max(152.0);
            ui.horizontal(|ui| {
                self.paint_places(ui, height);
                ui.add_space(3.0);
                ui.vertical(|ui| {
                    ui.set_min_width(ui.available_width());
                    self.paint_files(ui, height, &ctx);
                });
            });
            ui.add_space(10.0);
            self.paint_footer(ui, &ctx);
        });
    }
}

pub fn run() -> eframe::Result {
    let mut input = String::new();
    // Bounded protocol. No filesystem paths or UI state persist across calls.
    if let Err(error) = io::stdin().take(65537).read_to_string(&mut input) {
        eprintln!("ricefield picker: {error}");
        std::process::exit(2);
    }
    if input.len() > 65536 {
        eprintln!("ricefield picker: request too large");
        std::process::exit(2);
    }
    let request: Request = match toml::from_str(&input) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("ricefield picker: invalid input: {error}");
            std::process::exit(2);
        }
    };
    if let Err(error) = validate_request(&request) {
        eprintln!("ricefield picker: {error}");
        std::process::exit(2);
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([840.0, 620.0])
            .with_min_inner_size([600.0, 380.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RiceField • File Picker",
        options,
        Box::new(move |creation| {
            picker_style(&creation.egui_ctx);
            Ok(Box::new(Picker::new(request)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_uris_are_safe() {
        for invalid in [
            "",
            ".",
            "..",
            "/tmp/secret",
            "../escape",
            "a\0b",
            "foo\\bar",
        ] {
            assert!(!valid_name(invalid));
        }
        assert!(valid_name("Bericht 2026.docx"));
        assert_eq!(
            file_uri(Path::new("/tmp/a b#c")).unwrap(),
            "file:///tmp/a%20b%23c"
        );
        assert!(file_uri(Path::new("relative")).is_err());
    }

    #[test]
    fn filters_match_globs_and_do_not_match_other_files() {
        assert!(wildcard("*.docx", "report.docx"));
        assert!(!wildcard("*.docx", "report.txt"));
        assert!(matches_filter(
            "a.txt",
            &Filter {
                name: "Text".into(),
                rules: vec![(0, "*.txt".into())],
            }
        ));
    }

    #[test]
    fn selection_rejects_traversal_in_save_files() {
        let request = Request {
            mode: "save_files".into(),
            title: String::new(),
            accept_label: String::new(),
            multiple: false,
            directory: false,
            folder: String::new(),
            current_name: String::new(),
            files: vec!["../escape".into()],
            filters: vec![],
            selected_filter: None,
            choices: vec![],
        };
        assert!(validate_request(&request).is_err());
    }
}
