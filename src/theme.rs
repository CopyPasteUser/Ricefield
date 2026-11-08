use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path};

const MAX_THEME_BYTES: u64 = 256 * 1024;

pub const BUILTIN_IDS: &[&str] = &[
    "win7",
    "xp-luna",
    "win98",
    "win10",
    "frutiger-aero",
    "chromeos",
    "macos",
];

/// Translate RiceBowl's shared rice names to RiceField's bundled profiles.
/// Unknown/generated rices must not silently select an unrelated style.
pub fn ricebowl_id(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
        "windows98" | "windows-98" | "win98" | "98" | "classic98" => Some("win98"),
        "xp" | "xp-luna" | "windows-xp" | "windowsxp" | "luna" => Some("xp-luna"),
        "windows7" | "windows-7" | "win7" | "7" => Some("win7"),
        "windows10" | "windows-10" | "win10" | "10" => Some("win10"),
        "frutiger-aero" | "frutigeraero" | "aero" => Some("frutiger-aero"),
        "chromeos" | "chrome-os" | "chrome" => Some("chromeos"),
        "macos" | "mac-os" | "mac" | "macrice" => Some("macos"),
        _ => None,
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub layout: Layout,
    #[serde(default = "theme_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub shell: ShellConfig,
    pub palette: Palette,
    pub metrics: Metrics,
}

fn theme_schema_version() -> u32 {
    1
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Classic,
    Aero,
    Modern,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShellKind {
    Classic,
    Luna,
    Aero,
    Ribbon,
    Aqua,
    Chromeos,
    Finder,
    Modern,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChromeComponent {
    MenuBar,
    Tabs,
    Navigation,
    CommandBar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarPosition {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarSection {
    Tasks,
    Places,
    Favorites,
    Volumes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarSectionMode {
    Flat,
    Collapsible,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SidebarConfig {
    pub position: Option<SidebarPosition>,
    pub width: Option<f32>,
    pub sections: Option<Vec<SidebarSection>>,
    pub mode: Option<SidebarSectionMode>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewConfig {
    pub position: Option<SidebarPosition>,
    pub width: Option<f32>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShellConfig {
    pub kind: Option<ShellKind>,
    pub top: Option<Vec<ChromeComponent>>,
    pub bottom: Option<Vec<ChromeComponent>>,
    #[serde(default)]
    pub sidebar: SidebarConfig,
    #[serde(default)]
    pub preview: PreviewConfig,
    pub status_bar: Option<bool>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Palette {
    pub background: String,
    pub chrome: String,
    pub panel: String,
    pub sidebar: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    pub selection: String,
    pub border: String,
    pub folder: String,
    pub file: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Metrics {
    pub rounding: u8,
    pub row_height: f32,
    pub tile_width: f32,
    pub sidebar_width: f32,
    pub icon_size: f32,
}

impl Palette {
    pub fn color(&self, value: &str) -> Color32 {
        parse_color(value).unwrap_or(Color32::BLACK)
    }

    pub fn background(&self) -> Color32 {
        self.color(&self.background)
    }
    pub fn chrome(&self) -> Color32 {
        self.color(&self.chrome)
    }
    pub fn panel(&self) -> Color32 {
        self.color(&self.panel)
    }
    pub fn sidebar(&self) -> Color32 {
        self.color(&self.sidebar)
    }
    pub fn text(&self) -> Color32 {
        self.color(&self.text)
    }
    pub fn muted(&self) -> Color32 {
        self.color(&self.muted)
    }
    pub fn accent(&self) -> Color32 {
        self.color(&self.accent)
    }
    pub fn selection(&self) -> Color32 {
        self.color(&self.selection)
    }
    pub fn border(&self) -> Color32 {
        self.color(&self.border)
    }
    pub fn folder(&self) -> Color32 {
        self.color(&self.folder)
    }
    pub fn file(&self) -> Color32 {
        self.color(&self.file)
    }
}

pub fn parse_color(color: &str) -> Option<Color32> {
    let digits = color.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let rgb = u32::from_str_radix(digits, 16).ok()?;
    Some(Color32::from_rgb(
        (rgb >> 16) as u8,
        (rgb >> 8) as u8,
        rgb as u8,
    ))
}

impl Theme {
    pub fn shell_kind(&self) -> ShellKind {
        self.shell.kind.unwrap_or(match self.layout {
            Layout::Classic => ShellKind::Classic,
            Layout::Aero => ShellKind::Aero,
            Layout::Modern => ShellKind::Modern,
        })
    }

    pub fn top_components(&self) -> Vec<ChromeComponent> {
        let mut top = self
            .shell
            .top
            .clone()
            .unwrap_or_else(|| match self.shell_kind() {
                ShellKind::Classic | ShellKind::Luna => vec![
                    ChromeComponent::MenuBar,
                    ChromeComponent::Tabs,
                    ChromeComponent::Navigation,
                ],
                ShellKind::Aero | ShellKind::Aqua => vec![
                    ChromeComponent::Tabs,
                    ChromeComponent::Navigation,
                    ChromeComponent::CommandBar,
                ],
                ShellKind::Ribbon | ShellKind::Finder => vec![
                    ChromeComponent::Tabs,
                    ChromeComponent::CommandBar,
                    ChromeComponent::Navigation,
                ],
                ShellKind::Chromeos | ShellKind::Modern => {
                    vec![ChromeComponent::Tabs, ChromeComponent::Navigation]
                }
            });
        // Moving a default component to the lower region does not require
        // copying the entire default top list into a custom theme.
        if self.shell.top.is_none()
            && let Some(bottom) = &self.shell.bottom
        {
            top.retain(|component| !bottom.contains(component));
        }
        top
    }

    pub fn bottom_components(&self) -> Vec<ChromeComponent> {
        self.shell.bottom.clone().unwrap_or_default()
    }

    pub fn sidebar_sections(&self) -> Vec<SidebarSection> {
        self.shell.sidebar.sections.clone().unwrap_or_else(|| {
            if matches!(self.shell_kind(), ShellKind::Aero | ShellKind::Ribbon) {
                vec![
                    SidebarSection::Favorites,
                    SidebarSection::Places,
                    SidebarSection::Volumes,
                ]
            } else {
                vec![
                    SidebarSection::Places,
                    SidebarSection::Favorites,
                    SidebarSection::Volumes,
                ]
            }
        })
    }

    pub fn sidebar_section_mode(&self) -> SidebarSectionMode {
        self.shell.sidebar.mode.unwrap_or(SidebarSectionMode::Flat)
    }

    pub fn sidebar_position(&self) -> SidebarPosition {
        self.shell.sidebar.position.unwrap_or(SidebarPosition::Left)
    }

    pub fn sidebar_width(&self) -> f32 {
        self.shell
            .sidebar
            .width
            .unwrap_or(self.metrics.sidebar_width)
    }

    pub fn preview_position(&self) -> SidebarPosition {
        self.shell
            .preview
            .position
            .unwrap_or(SidebarPosition::Right)
    }

    pub fn preview_width(&self) -> f32 {
        self.shell.preview.width.unwrap_or(300.0)
    }

    pub fn builtin_source(id: &str) -> Option<&'static str> {
        let source = match id {
            "win7" => include_str!("../themes/win7.toml"),
            "xp-luna" => include_str!("../themes/xp-luna.toml"),
            "win98" => include_str!("../themes/win98.toml"),
            "win10" => include_str!("../themes/win10.toml"),
            "frutiger-aero" => include_str!("../themes/frutiger-aero.toml"),
            "chromeos" => include_str!("../themes/chromeos.toml"),
            "macos" => include_str!("../themes/macos.toml"),
            _ => return None,
        };
        Some(source)
    }

    pub fn builtin(id: &str) -> Option<Self> {
        Self::from_text(Self::builtin_source(id)?).ok()
    }

    pub fn from_text(source: &str) -> Result<Self, String> {
        if source.len() > MAX_THEME_BYTES as usize {
            return Err("Theme source exceeds 256 KiB limit".into());
        }
        let theme: Self = toml::from_str(source).map_err(|error| error.to_string())?;
        theme.validate()?;
        Ok(theme)
    }

    pub fn read_source(path: &Path) -> Result<String, String> {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("Theme must be a regular file".into());
        }
        if metadata.len() > MAX_THEME_BYTES {
            return Err("Theme file exceeds 256 KiB limit".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_THEME_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_THEME_BYTES {
            return Err("Theme file exceeds 256 KiB limit".into());
        }
        String::from_utf8(bytes).map_err(|error| error.to_string())
    }

    pub fn from_file(path: &Path) -> Result<Self, String> {
        Self::from_text(&Self::read_source(path)?)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "Unsupported theme schema version: {}",
                self.schema_version
            ));
        }
        let top = self.top_components();
        let bottom = self.bottom_components();
        if top.is_empty() && bottom.is_empty() {
            return Err("Shell must include at least one chrome component".into());
        }
        let mut seen = Vec::new();
        for component in top.iter().chain(bottom.iter()) {
            if seen.contains(component) {
                return Err("A shell component cannot appear in two regions".into());
            }
            seen.push(*component);
        }
        if !seen.contains(&ChromeComponent::Navigation) {
            return Err("Shell must expose the navigation component".into());
        }
        if let Some(sections) = &self.shell.sidebar.sections {
            if sections.is_empty() {
                return Err("Sidebar must contain at least one section".into());
            }
            if sections
                .iter()
                .enumerate()
                .any(|(index, item)| sections[..index].contains(item))
            {
                return Err("Duplicate sidebar sections".into());
            }
        }
        if self
            .shell
            .sidebar
            .width
            .is_some_and(|width| !(120.0..=420.0).contains(&width))
        {
            return Err("Sidebar width outside supported range".into());
        }
        if self
            .shell
            .preview
            .width
            .is_some_and(|width| !(210.0..=520.0).contains(&width))
        {
            return Err("Preview width outside supported range".into());
        }
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Err("Theme id and name must not be empty".into());
        }
        for color in [
            &self.palette.background,
            &self.palette.chrome,
            &self.palette.panel,
            &self.palette.sidebar,
            &self.palette.text,
            &self.palette.muted,
            &self.palette.accent,
            &self.palette.selection,
            &self.palette.border,
            &self.palette.folder,
            &self.palette.file,
        ] {
            if parse_color(color).is_none() {
                return Err(format!("Invalid theme color: {color}"));
            }
        }
        if !(24.0..=80.0).contains(&self.metrics.row_height)
            || !(80.0..=240.0).contains(&self.metrics.tile_width)
            || !(120.0..=420.0).contains(&self.metrics.sidebar_width)
            || !(12.0..=64.0).contains(&self.metrics.icon_size)
        {
            return Err("Theme metrics outside supported range".into());
        }
        Ok(())
    }

    pub fn apply(&self, context: &egui::Context) {
        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = self.palette.panel();
        visuals.window_fill = self.palette.panel();
        visuals.override_text_color = Some(self.palette.text());
        visuals.selection.bg_fill = self.palette.selection();
        visuals.selection.stroke.color = self.palette.accent();
        visuals.widgets.noninteractive.bg_fill = self.palette.background();
        visuals.widgets.noninteractive.fg_stroke.color = self.palette.text();
        visuals.widgets.noninteractive.bg_stroke.color = self.palette.border();
        visuals.widgets.inactive.bg_fill = self.palette.background();
        visuals.widgets.inactive.weak_bg_fill = self.palette.background();
        visuals.widgets.inactive.bg_stroke.color = self.palette.border();
        visuals.widgets.inactive.fg_stroke.color = self.palette.text();
        visuals.widgets.hovered.bg_fill = self.palette.selection();
        visuals.widgets.hovered.weak_bg_fill = self.palette.selection();
        visuals.widgets.hovered.bg_stroke.color = self.palette.accent();
        visuals.widgets.hovered.fg_stroke.color = self.palette.text();
        visuals.widgets.active.bg_fill = self.palette.selection();
        visuals.widgets.active.weak_bg_fill = self.palette.selection();
        visuals.widgets.active.fg_stroke.color = self.palette.accent();
        visuals.widgets.open.bg_fill = self.palette.selection();
        visuals.widgets.open.weak_bg_fill = self.palette.selection();
        visuals.widgets.noninteractive.corner_radius = self.metrics.rounding.into();
        visuals.widgets.inactive.corner_radius = self.metrics.rounding.into();
        visuals.widgets.hovered.corner_radius = self.metrics.rounding.into();
        visuals.widgets.active.corner_radius = self.metrics.rounding.into();
        visuals.widgets.open.corner_radius = self.metrics.rounding.into();
        context.set_visuals(visuals);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_theme_loader_rejects_over_limit_and_non_regular_files() {
        let name = format!(
            "rice-theme-size-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(name);
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_THEME_BYTES + 1).unwrap();
        assert!(Theme::from_file(&path).err().unwrap().contains("256 KiB"));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(Theme::from_file(&path).is_err());
        fs::remove_dir(&path).unwrap();
    }

    #[test]
    fn every_builtin_profile_is_valid() {
        for id in BUILTIN_IDS {
            let theme = Theme::builtin(id).unwrap();
            assert_eq!(theme.id, *id);
            assert_eq!(theme.schema_version, 1);
            assert!(theme.shell.kind.is_some());
            assert!(!theme.top_components().is_empty());
            theme.validate().unwrap();
        }
    }

    #[test]
    fn ricebowl_names_select_only_real_ricefield_profiles() {
        for (name, expected) in [
            ("Windows 98", "win98"),
            ("windows-xp", "xp-luna"),
            ("windows7", "win7"),
            ("Windows_10", "win10"),
            ("Frutiger Aero", "frutiger-aero"),
            ("chrome-os", "chromeos"),
            ("macrice", "macos"),
        ] {
            let selected = ricebowl_id(name);
            assert_eq!(selected, Some(expected));
            assert!(Theme::builtin(selected.unwrap()).is_some());
        }
        assert_eq!(ricebowl_id("custom-generated-rice"), None);
        assert_eq!(ricebowl_id(""), None);
    }

    #[test]
    fn bad_hex_and_metrics_are_rejected() {
        assert!(parse_color("#GGGGGG").is_none());
        assert!(parse_color("#12345").is_none());
        let mut theme = Theme::builtin("win7").unwrap();
        theme.metrics.row_height = 0.0;
        assert!(theme.validate().is_err());
    }

    #[test]
    fn old_theme_files_keep_their_legacy_layouts() {
        for (id, kind, top) in [
            (
                "win98",
                ShellKind::Classic,
                vec![
                    ChromeComponent::MenuBar,
                    ChromeComponent::Tabs,
                    ChromeComponent::Navigation,
                ],
            ),
            (
                "win7",
                ShellKind::Aero,
                vec![
                    ChromeComponent::Tabs,
                    ChromeComponent::Navigation,
                    ChromeComponent::CommandBar,
                ],
            ),
            (
                "macos",
                ShellKind::Modern,
                vec![ChromeComponent::Tabs, ChromeComponent::Navigation],
            ),
        ] {
            let mut theme = Theme::builtin(id).unwrap();
            theme.shell = ShellConfig::default();
            assert_eq!(theme.shell_kind(), kind);
            assert_eq!(theme.top_components(), top);
        }

        let mut value: toml::Value = toml::from_str(include_str!("../themes/win7.toml")).unwrap();
        let table = value.as_table_mut().unwrap();
        table.remove("schema_version");
        table.remove("shell");
        let old_file: Theme = toml::from_str(&toml::to_string(&value).unwrap()).unwrap();
        assert_eq!(old_file.schema_version, 1);
        assert_eq!(old_file.shell_kind(), ShellKind::Aero);
        old_file.validate().unwrap();
    }

    #[test]
    fn invalid_shell_settings_are_rejected() {
        let mut theme = Theme::builtin("win7").unwrap();
        theme.schema_version = 2;
        assert!(theme.validate().is_err());

        theme.schema_version = 1;
        theme.shell.top = Some(vec![]);
        assert!(theme.validate().is_err());

        theme.shell.top = Some(vec![ChromeComponent::Tabs, ChromeComponent::Tabs]);
        assert!(theme.validate().is_err());

        let bad_component =
            include_str!("../themes/win7.toml").replace("\"command-bar\"", "\"unknown-component\"");
        assert!(toml::from_str::<Theme>(&bad_component).is_err());
    }

    #[test]
    fn explicit_shell_kind_works_without_a_top_list() {
        let mut theme = Theme::builtin("win98").unwrap();
        theme.shell.kind = Some(ShellKind::Finder);
        theme.shell.top = None;
        assert_eq!(
            theme.top_components(),
            vec![
                ChromeComponent::Tabs,
                ChromeComponent::CommandBar,
                ChromeComponent::Navigation,
            ]
        );
    }

    #[test]
    fn custom_sidebar_and_status_bar_are_validated() {
        let source = include_str!("../themes/win7.toml");
        assert_eq!(
            Theme::builtin("win7").unwrap().sidebar_sections(),
            vec![
                SidebarSection::Favorites,
                SidebarSection::Places,
                SidebarSection::Volumes
            ]
        );
        let with_sidebar = format!(
            "{source}\n[shell.sidebar]\nposition = 'right'\nwidth = 280\nsections = ['volumes', 'places']\n"
        );
        let theme: Theme = toml::from_str(&with_sidebar).unwrap();
        assert_eq!(theme.sidebar_position(), SidebarPosition::Right);
        assert_eq!(theme.sidebar_width(), 280.0);
        assert_eq!(
            theme.sidebar_sections(),
            vec![SidebarSection::Volumes, SidebarSection::Places]
        );
        theme.validate().unwrap();

        let mut invalid = theme.clone();
        invalid.shell.sidebar.width = Some(999.0);
        assert!(invalid.validate().is_err());
        invalid.shell.sidebar.width = Some(200.0);
        invalid.shell.sidebar.sections = Some(vec![SidebarSection::Places, SidebarSection::Places]);
        assert!(invalid.validate().is_err());
        invalid.shell.sidebar.sections = Some(Vec::new());
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn shell_regions_support_real_layout_recomposition() {
        let mut theme = Theme::builtin("win7").unwrap();
        theme.shell.top = None;
        theme.shell.bottom = Some(vec![ChromeComponent::Tabs, ChromeComponent::CommandBar]);
        assert_eq!(theme.top_components(), vec![ChromeComponent::Navigation]);
        assert_eq!(
            theme.bottom_components(),
            vec![ChromeComponent::Tabs, ChromeComponent::CommandBar]
        );
        theme.validate().unwrap();

        // A bottom-only shell can place navigation below its file listing.
        theme.shell.top = Some(Vec::new());
        theme.shell.bottom = Some(vec![ChromeComponent::Navigation]);
        theme.validate().unwrap();
    }

    #[test]
    fn shell_regions_reject_duplicate_or_missing_navigation() {
        let mut theme = Theme::builtin("win10").unwrap();
        theme.shell.bottom = Some(vec![ChromeComponent::Navigation]);
        assert!(theme.validate().is_err());

        theme.shell.top = Some(vec![ChromeComponent::Tabs]);
        theme.shell.bottom = Some(vec![ChromeComponent::CommandBar]);
        assert!(theme.validate().is_err());

        theme.shell.bottom = Some(vec![ChromeComponent::Navigation]);
        theme.validate().unwrap();
    }

    #[test]
    fn invalid_or_unknown_shell_regions_fail_instead_of_being_ignored() {
        let source = include_str!("../themes/win7.toml");
        let unsupported = source.replace(
            "top = [\"tabs\", \"navigation\", \"command-bar\"]",
            "left-dock = ['navigation']",
        );
        assert!(toml::from_str::<Theme>(&unsupported).is_err());
        let impossible_preview =
            format!("{source}\n[shell.preview]\nwidth = 330\nposition = 'center'\n");
        assert!(toml::from_str::<Theme>(&impossible_preview).is_err());
    }

    #[test]
    fn preview_and_task_shelf_are_composable() {
        let mut theme = Theme::builtin("xp-luna").unwrap();
        theme.shell.preview.position = Some(SidebarPosition::Left);
        theme.shell.preview.width = Some(320.0);
        assert_eq!(theme.preview_position(), SidebarPosition::Left);
        assert_eq!(theme.preview_width(), 320.0);
        assert!(theme.sidebar_sections().contains(&SidebarSection::Tasks));
        assert_eq!(
            theme.sidebar_section_mode(),
            SidebarSectionMode::Collapsible
        );
        theme.validate().unwrap();

        theme.shell.preview.width = Some(190.0);
        assert!(theme.validate().is_err());
    }

    #[test]
    fn sidebar_section_mode_is_skin_specific_and_backwards_compatible() {
        let win7 = Theme::builtin("win7").unwrap();
        assert_eq!(win7.sidebar_section_mode(), SidebarSectionMode::Flat);
        let luna = Theme::builtin("xp-luna").unwrap();
        assert_eq!(luna.sidebar_section_mode(), SidebarSectionMode::Collapsible);

        let source = include_str!("../themes/win7.toml");
        let unknown = format!("{source}\n[shell.sidebar]\nmode = 'floating'\n");
        assert!(toml::from_str::<Theme>(&unknown).is_err());
    }

    #[test]
    fn bundled_rices_have_distinct_component_regions() {
        let xp = Theme::builtin("xp-luna").unwrap();
        let win10 = Theme::builtin("win10").unwrap();
        let frutiger = Theme::builtin("frutiger-aero").unwrap();
        assert!(xp.sidebar_sections().contains(&SidebarSection::Tasks));
        assert!(!win10.sidebar_sections().contains(&SidebarSection::Tasks));
        assert!(
            win10
                .top_components()
                .contains(&ChromeComponent::CommandBar)
        );
        assert!(
            !frutiger
                .top_components()
                .contains(&ChromeComponent::CommandBar)
        );
        assert_eq!(
            frutiger.bottom_components(),
            vec![ChromeComponent::CommandBar]
        );
    }

    #[test]
    fn win98_win7_and_macos_use_distinct_structural_shells() {
        let themes = ["win98", "win7", "macos"].map(|id| Theme::builtin(id).unwrap());
        for theme in &themes {
            theme.validate().unwrap();
            let top = theme.top_components();
            let bottom = theme.bottom_components();
            let navigation = top
                .iter()
                .chain(bottom.iter())
                .filter(|component| **component == ChromeComponent::Navigation)
                .count();
            assert_eq!(navigation, 1);
        }
        for left in 0..themes.len() {
            for right in (left + 1)..themes.len() {
                assert_ne!(themes[left].shell_kind(), themes[right].shell_kind());
                assert_ne!(
                    themes[left].top_components(),
                    themes[right].top_components()
                );
            }
        }
    }
}
