//! Presentation-only widgets shared by every rice.
//!
//! Widgets return semantic events; filesystem actions remain exclusively in
//! the Explorer controller. Themes may vary their layout without acquiring
//! write permissions or diverging in keyboard behavior.

use crate::theme::{ShellKind, SidebarSection, SidebarSectionMode, Theme};
use eframe::egui::{self, Align2, Color32, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    NewFolder,
    Copy,
    Cut,
    Paste,
    Rename,
    Trash,
    Properties,
    Bookmark,
    Refresh,
}

pub fn command_bar(ui: &mut egui::Ui, theme: &Theme, has_selection: bool) -> Option<ToolbarAction> {
    let mut action = None;
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
                action = Some(ToolbarAction::NewFolder);
            }
            if ui
                .add_enabled(has_selection, egui::Button::new("Copy"))
                .clicked()
            {
                action = Some(ToolbarAction::Copy);
            }
            if !finder
                && ui
                    .add_enabled(has_selection, egui::Button::new("Cut"))
                    .clicked()
            {
                action = Some(ToolbarAction::Cut);
            }
            if ui.small_button("Paste").clicked() {
                action = Some(ToolbarAction::Paste);
            }
            if ui
                .add_enabled(has_selection, egui::Button::new("Rename"))
                .clicked()
            {
                action = Some(ToolbarAction::Rename);
            }
            if finder {
                if ui
                    .add_enabled(has_selection, egui::Button::new("Info"))
                    .clicked()
                {
                    action = Some(ToolbarAction::Properties);
                }
            } else if ui
                .add_enabled(has_selection, egui::Button::new("Trash"))
                .clicked()
            {
                action = Some(ToolbarAction::Trash);
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
                action = Some(ToolbarAction::NewFolder);
            }
            if ui.small_button("Add to favorites").clicked() {
                action = Some(ToolbarAction::Bookmark);
            }
            if ui.small_button("Refresh").clicked() {
                action = Some(ToolbarAction::Refresh);
            }
        }
    });
    action
}

pub fn sidebar_section(
    ui: &mut egui::Ui,
    theme: &Theme,
    index: usize,
    section: SidebarSection,
    contents: impl FnOnce(&mut egui::Ui),
) {
    let title = match section {
        SidebarSection::Tasks => "FILE TASKS",
        SidebarSection::Places => "PLACES",
        SidebarSection::Favorites => "FAVORITES",
        SidebarSection::Volumes => "VOLUMES",
    };
    if theme.sidebar_section_mode() == SidebarSectionMode::Collapsible {
        egui::CollapsingHeader::new(title)
            .id_salt((theme.id.as_str(), index))
            .default_open(true)
            .show(ui, contents);
    } else {
        ui.label(
            egui::RichText::new(title)
                .small()
                .strong()
                .color(theme.palette.muted()),
        );
        contents(ui);
    }
}

/// Consistent desktop dialog container; never executes the confirmed action.
pub fn show_dialog(
    context: &egui::Context,
    title: impl Into<egui::WidgetText>,
    resizable: bool,
    default_width: Option<f32>,
    contents: impl FnOnce(&mut egui::Ui),
) {
    let available = context.available_rect().width();
    let mut window = egui::Window::new(title)
        .collapsible(false)
        .resizable(resizable)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .max_width((available - 20.0).max(180.0));
    if let Some(width) = default_width {
        window = window.default_width(width.min((available - 20.0).max(180.0)));
    }
    window.show(context, contents);
}

pub fn small_error(ui: &mut egui::Ui, message: impl Into<String>) {
    ui.colored_label(Color32::from_rgb(170, 48, 43), message.into());
}
