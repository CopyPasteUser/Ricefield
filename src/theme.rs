use eframe::egui::{self, Color32};
use serde::Deserialize;
use std::{fs, path::Path};

pub const BUILTIN_IDS: &[&str] = &[
    "win7",
    "xp-luna",
    "win98",
    "win10",
    "frutiger-aero",
    "chromeos",
    "macos",
];

#[derive(Clone, Deserialize)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub layout: Layout,
    pub palette: Palette,
    pub metrics: Metrics,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Classic,
    Aero,
    Modern,
}

#[derive(Clone, Deserialize)]
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

#[derive(Clone, Deserialize)]
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
    pub fn builtin(id: &str) -> Option<Self> {
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
        toml::from_str(source).ok()
    }

    pub fn from_file(path: &Path) -> Result<Self, String> {
        let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
        let theme: Self = toml::from_str(&contents).map_err(|error| error.to_string())?;
        theme.validate()?;
        Ok(theme)
    }

    pub fn validate(&self) -> Result<(), String> {
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
    fn every_builtin_profile_is_valid() {
        for id in BUILTIN_IDS {
            let theme = Theme::builtin(id).unwrap();
            assert_eq!(theme.id, *id);
            theme.validate().unwrap();
        }
    }

    #[test]
    fn bad_hex_and_metrics_are_rejected() {
        assert!(parse_color("#GGGGGG").is_none());
        assert!(parse_color("#12345").is_none());
        let mut theme = Theme::builtin("win7").unwrap();
        theme.metrics.row_height = 0.0;
        assert!(theme.validate().is_err());
    }
}
