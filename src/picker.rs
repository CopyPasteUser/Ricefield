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
        && name.as_bytes().len() <= 255
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

impl eframe::App for Picker {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.heading(&self.request.title);
                ui.separator();
                ui.label("RiceField");
            });
            ui.horizontal(|ui| {
                if ui.button("↑").on_hover_text("Parent folder").clicked() {
                    if let Some(parent) = self.folder.parent() {
                        self.navigate(parent.to_path_buf());
                    }
                }
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.address).desired_width(f32::INFINITY),
                );
                let go = ui.button("Go").clicked();
                if go
                    || (response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                {
                    let input = PathBuf::from(&self.address);
                    self.navigate(input);
                }
            });
            ui.horizontal(|ui| {
                for (name, path) in [
                    ("Home", std::env::var_os("HOME").map(PathBuf::from)),
                    (
                        "Documents",
                        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Documents")),
                    ),
                    (
                        "Downloads",
                        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads")),
                    ),
                    ("Root", Some(PathBuf::from("/"))),
                ] {
                    if let Some(path) = path
                        && path.is_dir()
                        && ui.button(name).clicked()
                    {
                        self.navigate(path);
                    }
                }
                ui.separator();
                if ui.checkbox(&mut self.show_hidden, "Hidden files").changed() {
                    self.refresh();
                }
                if ui.button("↻").on_hover_text("Refresh").clicked() {
                    self.refresh();
                }
            });
        });
        ui.vertical(|ui| {
            egui::ScrollArea::vertical()
                .max_height((ui.available_height() - 180.0).max(180.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for entry in self.entries.clone() {
                        let marked = self.selected.contains(&entry.path);
                        let icon = if entry.directory { "📁" } else { "📄" };
                        let response =
                            ui.selectable_label(marked, format!("{icon}  {}", entry.name));
                        if response.double_clicked() {
                            if entry.directory {
                                self.navigate(entry.path);
                            } else {
                                self.selected.clear();
                                self.selected.insert(entry.path);
                                if self.request.mode == "open" && !self.folder_mode() {
                                    self.choose(&ctx);
                                }
                            }
                            break;
                        }
                        if response.clicked() {
                            if entry.directory && !self.folder_mode() {
                                self.selected.clear();
                            } else if self.request.multiple
                                && ui.input(|input| input.modifiers.ctrl)
                            {
                                if !self.selected.remove(&entry.path) {
                                    self.selected.insert(entry.path);
                                }
                            } else {
                                self.selected.clear();
                                self.selected.insert(entry.path.clone());
                                if self.request.mode == "save" && !entry.directory {
                                    self.file_name = entry.name;
                                    self.overwrite_pending = None;
                                }
                            }
                        }
                    }
                });
        });
        ui.vertical(|ui| {
            if self.request.mode == "save" {
                ui.horizontal(|ui| {
                    ui.label("Filename:");
                    if ui.text_edit_singleline(&mut self.file_name).changed() {
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
                                .find(|(value, _)| value == &self.choice_values[idx])
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
            if !self.message.is_empty() {
                ui.colored_label(egui::Color32::LIGHT_RED, &self.message);
            }
            ui.separator();
            ui.horizontal(|ui| {
                let label = if self.overwrite_pending.is_some() {
                    "Replace"
                } else if !self.request.accept_label.is_empty() {
                    &self.request.accept_label
                } else if self.request.mode == "open" {
                    "Open"
                } else {
                    "Save"
                };
                if ui.button(label).clicked() {
                    self.choose(ctx);
                }
                if ui.button("Cancel").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
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
            creation.egui_ctx.set_visuals(egui::Visuals::dark());
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
