//! Explicit, non-destructive theme customization and private export.
//! A visual change regenerates TOML; manual edits require validation first.

use crate::{
    config,
    theme::{
        ChromeComponent, ShellKind, SidebarPosition, SidebarSection, SidebarSectionMode, Theme,
    },
};
use eframe::egui::{self, Color32};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub enum Outcome {
    Close,
    Activate { path: PathBuf, theme: Box<Theme> },
}

pub struct ThemeEditor {
    source: String,
    preview: Option<Theme>,
    save_name: String,
    import_path: String,
    message: Option<(String, bool)>,
}

impl ThemeEditor {
    pub fn new(theme: &Theme, active_file: Option<&Path>) -> Self {
        let source = active_file
            .and_then(|path| Theme::read_source(path).ok())
            .or_else(|| Theme::builtin_source(&theme.id).map(str::to_owned))
            .unwrap_or_default();
        let preview = Theme::from_text(&source).ok();
        let base = active_file
            .and_then(Path::file_stem)
            .and_then(|stem| stem.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}-custom", theme.id));
        let base = base
            .bytes()
            .filter(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_'))
            .take(36)
            .map(char::from)
            .collect::<String>();
        let directory = config::config_file()
            .parent()
            .map(|path| path.join("themes"));
        let save_name = suggest_name(&base, |name| {
            directory
                .as_ref()
                .is_some_and(|path| path.join(format!("{name}.toml")).exists())
        });
        Self {
            source,
            preview,
            save_name,
            import_path: String::new(),
            message: None,
        }
    }

    fn update_source(&mut self) {
        if let Some(preview) = &self.preview {
            match toml::to_string_pretty(preview) {
                Ok(source) => self.source = source,
                Err(error) => self.message = Some((error.to_string(), true)),
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Outcome> {
        ui.small("Customize independently of file operations. Saved themes stay private to your user account.");
        ui.horizontal(|ui| {
            ui.label("Import TOML path:");
            ui.add_sized(
                [275.0, 23.0],
                egui::TextEdit::singleline(&mut self.import_path),
            );
            if ui.button("Import").clicked() {
                let path = Path::new(self.import_path.trim());
                match import(path) {
                    Ok((source, theme)) => {
                        self.source = source;
                        self.preview = Some(theme);
                        self.message = Some((
                            "Theme imported into editor; original file unchanged".into(),
                            false,
                        ));
                    }
                    Err(error) => self.message = Some((error, true)),
                }
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Theme TOML");
            if ui.button("Validate").clicked() {
                match Theme::from_text(&self.source) {
                    Ok(theme) => {
                        self.preview = Some(theme);
                        self.message = Some(("Valid TOML · visual controls enabled".into(), false));
                    }
                    Err(error) => {
                        self.preview = None;
                        self.message = Some((format!("Invalid theme: {error}"), true));
                    }
                }
            }
        });
        let editor = egui::TextEdit::multiline(&mut self.source)
            .code_editor()
            .desired_rows(12)
            .desired_width(f32::INFINITY);
        if ui.add(editor).changed() {
            self.preview = None;
            self.message = Some((
                "TOML changed · validate before saving or visual editing".into(),
                false,
            ));
        }

        if let Some(theme) = &mut self.preview {
            ui.separator();
            ui.strong("Visual editor");
            let mut changed = false;
            egui::Grid::new("theme_editor_colors")
                .num_columns(4)
                .show(ui, |ui| {
                    for (index, (label, value)) in [
                        ("Accent", &mut theme.palette.accent),
                        ("Background", &mut theme.palette.background),
                        ("Chrome", &mut theme.palette.chrome),
                        ("Panel", &mut theme.palette.panel),
                        ("Text", &mut theme.palette.text),
                        ("Selection", &mut theme.palette.selection),
                        ("Sidebar", &mut theme.palette.sidebar),
                        ("Border", &mut theme.palette.border),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        ui.label(label);
                        let mut color = crate::theme::parse_color(value).unwrap_or(Color32::BLACK);
                        if ui.color_edit_button_srgba(&mut color).changed() {
                            *value =
                                format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
                            changed = true;
                        }
                        if index % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            changed |= ui
                .add(egui::Slider::new(&mut theme.metrics.icon_size, 12.0..=64.0).text("Icon size"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut theme.metrics.row_height, 24.0..=80.0)
                        .text("Row height"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut theme.metrics.sidebar_width, 120.0..=420.0)
                        .text("Sidebar width"),
                )
                .changed();
            if changed {
                theme.shell.sidebar.width = Some(theme.metrics.sidebar_width);
            }
            changed |= shell_composer(ui, theme);
            if changed {
                self.update_source();
                self.message = Some((
                    "Visual changes written to TOML (comments may be reformatted)".into(),
                    false,
                ));
            }
        } else {
            ui.small("Validate the TOML to unlock color and layout controls.");
        }

        if let Some((message, error)) = &self.message {
            ui.colored_label(
                if *error {
                    Color32::RED
                } else {
                    Color32::DARK_GREEN
                },
                message,
            );
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Save as:");
            ui.add_sized(
                [180.0, 23.0],
                egui::TextEdit::singleline(&mut self.save_name),
            );
            ui.label(".toml (a new private file; no overwrites)");
        });
        let mut outcome = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.preview.is_some(),
                    egui::Button::new("Save & use theme"),
                )
                .clicked()
            {
                match Theme::from_text(&self.source).and_then(|theme| {
                    save_private(&self.save_name, &self.source).map(|path| (path, theme))
                }) {
                    Ok((path, theme)) => {
                        outcome = Some(Outcome::Activate {
                            path,
                            theme: Box::new(theme),
                        });
                    }
                    Err(error) => self.message = Some((error, true)),
                }
            }
            if ui.button("Cancel").clicked() {
                outcome = Some(Outcome::Close);
            }
        });
        outcome
    }
}

fn suggest_name(base: &str, exists: impl Fn(&str) -> bool) -> String {
    let base = if base.is_empty() { "rice-custom" } else { base };
    for suffix in 1..=99 {
        let candidate = if suffix == 1 {
            base.to_owned()
        } else {
            format!("{base}-{suffix}")
        };
        if !exists(&candidate) {
            return candidate;
        }
    }
    format!("rice-{}", std::process::id())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Region {
    Off,
    Top,
    Bottom,
}

fn component_region(theme: &Theme, component: ChromeComponent) -> Region {
    if theme.top_components().contains(&component) {
        Region::Top
    } else if theme.bottom_components().contains(&component) {
        Region::Bottom
    } else {
        Region::Off
    }
}

fn set_component_region(theme: &mut Theme, component: ChromeComponent, region: Region) {
    let mut top = theme.top_components();
    let mut bottom = theme.bottom_components();
    top.retain(|item| *item != component);
    bottom.retain(|item| *item != component);
    match region {
        Region::Top => top.push(component),
        Region::Bottom => bottom.push(component),
        Region::Off => {}
    }
    // Store *both* lists explicitly, so a disabled default component does not
    // silently return through ShellKind's default composition.
    theme.shell.top = Some(top);
    theme.shell.bottom = Some(bottom);
}

fn shift_component(theme: &mut Theme, component: ChromeComponent, direction: i8) -> bool {
    let top = theme.top_components();
    let bottom = theme.bottom_components();
    let (region, mut components) = if top.contains(&component) {
        (Region::Top, top)
    } else if bottom.contains(&component) {
        (Region::Bottom, bottom)
    } else {
        return false;
    };
    let Some(index) = components.iter().position(|entry| *entry == component) else {
        return false;
    };
    let target = index as isize + direction as isize;
    if target < 0 || target >= components.len() as isize {
        return false;
    }
    components.swap(index, target as usize);
    match region {
        Region::Top => theme.shell.top = Some(components),
        Region::Bottom => theme.shell.bottom = Some(components),
        Region::Off => return false,
    }
    true
}

fn shell_composer(ui: &mut egui::Ui, theme: &mut Theme) -> bool {
    let mut changed = false;
    egui::CollapsingHeader::new("Shell composer · component layout")
        .default_open(false)
        .show(ui, |ui| {
            ui.small("Move actual toolbars between top and bottom; ↑/↓ changes the render order. Navigation is required.");
            let mut kind = theme.shell_kind();
            egui::ComboBox::from_id_salt("editor_shell_kind")
                .selected_text(format!("{:?}", kind))
                .show_ui(ui, |ui| {
                    for option in [
                        ShellKind::Classic,
                        ShellKind::Luna,
                        ShellKind::Aero,
                        ShellKind::Ribbon,
                        ShellKind::Aqua,
                        ShellKind::Chromeos,
                        ShellKind::Finder,
                        ShellKind::Modern,
                    ] {
                        ui.selectable_value(&mut kind, option, format!("{:?}", option));
                    }
                });
            if theme.shell_kind() != kind {
                theme.shell.kind = Some(kind);
                changed = true;
            }

            for (index, (label, component)) in [
                ("Menu bar", ChromeComponent::MenuBar),
                ("Tabs", ChromeComponent::Tabs),
                ("Navigation", ChromeComponent::Navigation),
                ("Command bar", ChromeComponent::CommandBar),
            ]
            .into_iter()
            .enumerate()
            {
                ui.horizontal(|ui| {
                    ui.label(label);
                    let original = component_region(theme, component);
                    let mut region = original;
                    egui::ComboBox::from_id_salt(("editor_component", index))
                        .selected_text(match region {
                            Region::Top => "Top",
                            Region::Bottom => "Bottom",
                            Region::Off => "Off",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut region, Region::Top, "Top");
                            ui.selectable_value(&mut region, Region::Bottom, "Bottom");
                            if component != ChromeComponent::Navigation {
                                ui.selectable_value(&mut region, Region::Off, "Off");
                            }
                        });
                    if region != original {
                        set_component_region(theme, component, region);
                        changed = true;
                    }
                    if ui.small_button("↑").on_hover_text("Move earlier").clicked() {
                        changed |= shift_component(theme, component, -1);
                    }
                    if ui.small_button("↓").on_hover_text("Move later").clicked() {
                        changed |= shift_component(theme, component, 1);
                    }
                });
            }

            ui.separator();
            ui.strong("Sidebar & preview");
            let mut sidebar_position = theme.sidebar_position();
            let mut preview_position = theme.preview_position();
            ui.horizontal(|ui| {
                ui.label("Sidebar");
                egui::ComboBox::from_id_salt("editor_sidebar_position")
                    .selected_text(if sidebar_position == SidebarPosition::Left {
                        "Left"
                    } else {
                        "Right"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut sidebar_position, SidebarPosition::Left, "Left");
                        ui.selectable_value(&mut sidebar_position, SidebarPosition::Right, "Right");
                    });
                ui.label("Preview pane");
                egui::ComboBox::from_id_salt("editor_preview_position")
                    .selected_text(if preview_position == SidebarPosition::Left {
                        "Left"
                    } else {
                        "Right"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut preview_position, SidebarPosition::Left, "Left");
                        ui.selectable_value(&mut preview_position, SidebarPosition::Right, "Right");
                    });
            });
            if sidebar_position != theme.sidebar_position() {
                theme.shell.sidebar.position = Some(sidebar_position);
                changed = true;
            }
            if preview_position != theme.preview_position() {
                theme.shell.preview.position = Some(preview_position);
                changed = true;
            }

            let mut sections = theme.sidebar_sections();
            ui.horizontal_wrapped(|ui| {
                for (label, section) in [
                    ("Tasks", SidebarSection::Tasks),
                    ("Places", SidebarSection::Places),
                    ("Favorites", SidebarSection::Favorites),
                    ("Volumes", SidebarSection::Volumes),
                ] {
                    let mut enabled = sections.contains(&section);
                    if ui
                        .add_enabled(
                            !enabled || sections.len() > 1,
                            egui::Checkbox::new(&mut enabled, label),
                        )
                        .changed()
                    {
                        if enabled {
                            sections.push(section);
                        } else if sections.len() > 1 {
                            sections.retain(|item| *item != section);
                        }
                        changed = true;
                    }
                }
            });
            if sections != theme.sidebar_sections() {
                theme.shell.sidebar.sections = Some(sections);
            }

            let mut collapsible = theme.sidebar_section_mode() == SidebarSectionMode::Collapsible;
            if ui.checkbox(&mut collapsible, "Collapsible sidebar groups").changed() {
                theme.shell.sidebar.mode = Some(if collapsible {
                    SidebarSectionMode::Collapsible
                } else {
                    SidebarSectionMode::Flat
                });
                changed = true;
            }
            let mut status_bar = theme.shell.status_bar.unwrap_or(true);
            if ui.checkbox(&mut status_bar, "Status bar").changed() {
                theme.shell.status_bar = Some(status_bar);
                changed = true;
            }
        });
    changed
}

fn import(path: &Path) -> Result<(String, Theme), String> {
    if !path.is_absolute() || path.as_os_str().len() > 4096 {
        return Err("Import requires a local absolute path (max 4096 bytes)".into());
    }
    let source = Theme::read_source(path)?;
    let theme = Theme::from_text(&source)?;
    Ok((source, theme))
}

fn save_private(name: &str, source: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || name.len() > 48
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("Use 1–48 letters, numbers, hyphens or underscores for the theme name".into());
    }
    Theme::from_text(source)?;
    let config_file = config::config_file();
    let parent = config_file
        .parent()
        .ok_or("Cannot locate user config folder")?;
    private_directory(parent)?;
    let directory = parent.join("themes");
    private_directory(&directory)?;
    let path = directory.join(format!("{name}.toml"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
    if let Err(error) = file
        .write_all(source.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(format!("Cannot save theme: {error}"));
    }
    Ok(path)
}

fn private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::symlink_metadata(path)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Theme directory must not be a symlink".into());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_name_cannot_escape_private_directory() {
        let valid = Theme::builtin_source("win7").unwrap();
        for name in ["../outside", "", ".hidden", "with space", "a/b", "--.."] {
            assert!(save_private(name, valid).is_err());
        }
    }

    #[test]
    fn imported_toml_must_be_absolute_and_validated() {
        assert!(import(Path::new("relative/theme.toml")).is_err());
        assert!(Theme::from_text("[palette]\naccent = '#fff'").is_err());
    }

    #[test]
    fn visual_edits_serialize_as_valid_theme_source() {
        let mut theme = Theme::builtin("win7").unwrap();
        theme.palette.accent = "#1234AB".into();
        theme.metrics.icon_size = 38.0;
        let source = toml::to_string_pretty(&theme).unwrap();
        let parsed = Theme::from_text(&source).unwrap();
        assert_eq!(parsed.palette.accent, "#1234AB");
        assert_eq!(parsed.metrics.icon_size, 38.0);
    }

    #[test]
    fn shell_composer_preserves_valid_navigation_and_region_order() {
        let mut theme = Theme::builtin("win7").unwrap();
        set_component_region(&mut theme, ChromeComponent::Navigation, Region::Bottom);
        set_component_region(&mut theme, ChromeComponent::MenuBar, Region::Top);
        set_component_region(&mut theme, ChromeComponent::Tabs, Region::Off);
        assert_eq!(
            component_region(&theme, ChromeComponent::Navigation),
            Region::Bottom
        );
        assert_eq!(component_region(&theme, ChromeComponent::Tabs), Region::Off);
        assert!(shift_component(&mut theme, ChromeComponent::MenuBar, -1));
        theme.validate().unwrap();
        let restored = Theme::from_text(&toml::to_string_pretty(&theme).unwrap()).unwrap();
        assert_eq!(restored.top_components(), theme.top_components());
        assert_eq!(restored.bottom_components(), theme.bottom_components());
    }

    #[test]
    fn editor_suggests_an_unused_name_without_overwriting() {
        assert_eq!(
            suggest_name("xp-custom", |name| name == "xp-custom"),
            "xp-custom-2"
        );
        assert_eq!(suggest_name("", |_| false), "rice-custom");
    }
}
