//! Presentation-only widgets shared by every rice.
//!
//! Widgets return semantic events; filesystem actions remain exclusively in
//! the Explorer controller. Themes may vary their layout without acquiring
//! write permissions or diverging in keyboard behavior.

use crate::theme::{ShellKind, SidebarSection, SidebarSectionMode, Theme};
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};

#[derive(Clone, Copy)]
pub enum LunaLinkIcon {
    Folder,
    File,
    Computer,
    Bookmark,
    Drive,
}

#[derive(Clone, Copy)]
pub enum LunaPanel {
    Search,
    FolderTree,
    Advanced,
}

#[derive(Clone, Copy)]
pub enum LunaToolbarIcon {
    Back,
    Forward,
    Up,
    Search,
    Folders,
}

pub fn luna_toolbar_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: LunaToolbarIcon,
    enabled: bool,
) -> bool {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(
            egui::RichText::new(format!("      {label}"))
                .size(12.5)
                .color(Color32::from_rgb(38, 45, 56)),
        )
        .min_size(Vec2::new(70.0, 35.0))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE),
    );
    let center = Pos2::new(response.rect.left() + 17.0, response.rect.center().y);
    let alpha = if enabled { 1.0 } else { 0.4 };
    let paint = ui.painter();
    match icon {
        LunaToolbarIcon::Back | LunaToolbarIcon::Forward => {
            paint.circle_filled(
                center,
                12.0,
                Color32::from_rgb(74, 160, 57).gamma_multiply(alpha),
            );
            paint.circle_stroke(
                center,
                12.0,
                Stroke::new(1.0, Color32::from_rgb(36, 109, 39).gamma_multiply(alpha)),
            );
            let direction = if matches!(icon, LunaToolbarIcon::Back) {
                -1.0
            } else {
                1.0
            };
            let vertices = vec![
                center + Vec2::new(direction * 8.0, 0.0),
                center + Vec2::new(-direction * 2.0, -7.0),
                center + Vec2::new(-direction * 2.0, 7.0),
            ];
            paint.add(egui::Shape::convex_polygon(
                vertices,
                Color32::WHITE.gamma_multiply(alpha),
                Stroke::NONE,
            ));
            paint.rect_filled(
                Rect::from_center_size(
                    center + Vec2::new(-direction * 5.0, 0.0),
                    Vec2::new(9.0, 6.0),
                ),
                0,
                Color32::WHITE.gamma_multiply(alpha),
            );
        }
        LunaToolbarIcon::Up => {
            paint.rect_filled(
                Rect::from_center_size(center + Vec2::new(0.0, 4.0), Vec2::new(22.0, 15.0)),
                2,
                Color32::from_rgb(249, 202, 77),
            );
            paint.line_segment(
                [center + Vec2::new(0.0, 4.0), center + Vec2::new(0.0, -9.0)],
                Stroke::new(3.0, Color32::from_rgb(52, 113, 188)),
            );
            paint.line_segment(
                [
                    center + Vec2::new(0.0, -9.0),
                    center + Vec2::new(-5.0, -3.0),
                ],
                Stroke::new(3.0, Color32::from_rgb(52, 113, 188)),
            );
            paint.line_segment(
                [center + Vec2::new(0.0, -9.0), center + Vec2::new(5.0, -3.0)],
                Stroke::new(3.0, Color32::from_rgb(52, 113, 188)),
            );
        }
        LunaToolbarIcon::Search => {
            paint.circle_filled(
                center + Vec2::new(-3.0, -3.0),
                8.0,
                Color32::from_rgb(116, 185, 216),
            );
            paint.circle_stroke(
                center + Vec2::new(-3.0, -3.0),
                8.0,
                Stroke::new(2.0, Color32::from_rgb(45, 92, 145)),
            );
            paint.line_segment(
                [center + Vec2::new(3.0, 3.0), center + Vec2::new(12.0, 12.0)],
                Stroke::new(4.0, Color32::from_rgb(172, 125, 74)),
            );
        }
        LunaToolbarIcon::Folders => {
            paint.rect_filled(
                Rect::from_min_max(
                    center + Vec2::new(-10.0, -9.0),
                    center + Vec2::new(1.0, -2.0),
                ),
                1,
                Color32::from_rgb(221, 151, 40),
            );
            paint.rect_filled(
                Rect::from_center_size(center + Vec2::new(0.0, 2.0), Vec2::new(23.0, 16.0)),
                2,
                Color32::from_rgb(249, 199, 69),
            );
            paint.line_segment(
                [
                    center + Vec2::new(-8.0, -1.0),
                    center + Vec2::new(9.0, -1.0),
                ],
                Stroke::new(1.0, Color32::from_rgb(255, 231, 139)),
            );
        }
    }
    response.clicked()
}

impl LunaPanel {
    fn id(self) -> egui::Id {
        let name = match self {
            Self::Search => "search",
            Self::FolderTree => "folders",
            Self::Advanced => "advanced",
        };
        egui::Id::unique(("ricefield-xp-luna", name))
    }
}

pub fn luna_panel_open(ctx: &egui::Context, panel: LunaPanel) -> bool {
    ctx.data_mut(|data| data.get_persisted::<bool>(panel.id()).unwrap_or(false))
}

pub fn luna_toggle_panel(ctx: &egui::Context, panel: LunaPanel) {
    let id = panel.id();
    ctx.data_mut(|data| {
        let open = data.get_persisted::<bool>(id).unwrap_or(false);
        data.insert_persisted(id, !open);
    });
}

/// Task-pane hyperlink with original vector artwork, no platform icon assets.
/// egui's regular Button keeps keyboard focus and activation behavior.
pub fn luna_link(ui: &mut egui::Ui, label: &str, enabled: bool, icon: LunaLinkIcon) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        let painter = ui.painter();
        match icon {
            LunaLinkIcon::Folder => {
                painter.rect_filled(
                    Rect::from_min_max(
                        rect.min + Vec2::new(1.0, 3.0),
                        rect.min + Vec2::new(10.0, 8.0),
                    ),
                    0,
                    Color32::from_rgb(223, 158, 30),
                );
                painter.rect_filled(
                    Rect::from_min_max(
                        rect.min + Vec2::new(1.0, 6.0),
                        rect.max - Vec2::new(1.0, 2.0),
                    ),
                    1,
                    Color32::from_rgb(247, 194, 56),
                );
                painter.line_segment(
                    [
                        Pos2::new(rect.left() + 2.0, rect.top() + 8.0),
                        Pos2::new(rect.right() - 2.0, rect.top() + 8.0),
                    ],
                    Stroke::new(1.0, Color32::from_rgb(255, 238, 143)),
                );
            }
            LunaLinkIcon::File => {
                painter.rect_filled(
                    rect.shrink2(Vec2::new(3.0, 1.5)),
                    0,
                    Color32::from_rgb(245, 249, 255),
                );
                painter.rect_stroke(
                    rect.shrink2(Vec2::new(3.0, 1.5)),
                    0,
                    Stroke::new(1.0, Color32::from_rgb(65, 122, 188)),
                    egui::StrokeKind::Inside,
                );
                for y in [6.0, 9.0, 12.0] {
                    painter.line_segment(
                        [rect.min + Vec2::new(5.0, y), rect.min + Vec2::new(11.0, y)],
                        Stroke::new(1.0, Color32::from_rgb(110, 155, 200)),
                    );
                }
            }
            LunaLinkIcon::Computer => {
                painter.rect_filled(
                    Rect::from_min_max(
                        rect.min + Vec2::new(1.0, 2.0),
                        rect.min + Vec2::new(15.0, 12.0),
                    ),
                    1,
                    Color32::from_rgb(64, 99, 155),
                );
                painter.rect_filled(
                    Rect::from_min_max(
                        rect.min + Vec2::new(3.0, 4.0),
                        rect.min + Vec2::new(13.0, 10.0),
                    ),
                    0,
                    Color32::from_rgb(139, 213, 236),
                );
                painter.line_segment(
                    [
                        rect.min + Vec2::new(4.0, 14.0),
                        rect.min + Vec2::new(12.0, 14.0),
                    ],
                    Stroke::new(2.0, Color32::from_rgb(88, 101, 125)),
                );
            }
            LunaLinkIcon::Bookmark => {
                painter.circle_filled(rect.center(), 6.0, Color32::from_rgb(247, 204, 76));
                painter.circle_stroke(
                    rect.center(),
                    6.0,
                    Stroke::new(1.0, Color32::from_rgb(173, 133, 43)),
                );
                painter.circle_filled(rect.center(), 2.0, Color32::from_rgb(255, 244, 172));
            }
            LunaLinkIcon::Drive => {
                painter.rect_filled(
                    Rect::from_min_max(
                        rect.min + Vec2::new(1.0, 5.0),
                        rect.max - Vec2::new(1.0, 2.0),
                    ),
                    2,
                    Color32::from_rgb(176, 188, 205),
                );
                painter.line_segment(
                    [
                        rect.min + Vec2::new(3.0, 10.0),
                        rect.min + Vec2::new(13.0, 10.0),
                    ],
                    Stroke::new(1.0, Color32::from_rgb(83, 104, 137)),
                );
            }
        }
        ui.add_enabled(
            enabled,
            egui::Button::new(
                egui::RichText::new(label)
                    .color(if enabled {
                        Color32::from_rgb(24, 68, 155)
                    } else {
                        Color32::from_rgb(130, 143, 167)
                    })
                    .size(12.0),
            )
            .frame(false),
        )
        .clicked()
    })
    .inner
}

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
    if theme.shell_kind() == ShellKind::Luna {
        // XP's Explorer uses individual task panes rather than a flat tree of
        // headings. Keep the expanded state in egui's persistent UI memory so
        // collapsing a pane never mutates file-manager or theme state.
        let id = ui.make_persistent_id(("luna-task-pane", theme.id.as_str(), index));
        let expanded = ui.data_mut(|data| data.get_persisted::<bool>(id).unwrap_or(true));
        let heading = luna_section_title(section);
        let display_title = if ui.available_width() < 185.0 {
            match section {
                SidebarSection::Tasks => "Tasks",
                SidebarSection::Places => "Places",
                SidebarSection::Favorites => "Details",
                SidebarSection::Volumes => "Drives",
            }
        } else {
            heading
        };
        // Avoid missing-glyph boxes: egui's embedded font does not contain
        // every triangular disclosure arrow.
        let header = egui::Button::new(
            egui::RichText::new(display_title)
                .strong()
                .size(12.2)
                .color(Color32::from_rgb(24, 62, 145)),
        )
        .fill(Color32::from_rgb(245, 249, 255))
        .stroke(egui::Stroke::NONE)
        .corner_radius(egui::CornerRadius::same(7));
        let response = ui
            .add_sized([ui.available_width(), 29.0], header)
            .on_hover_text(format!("Toggle {heading}"));
        let center = Pos2::new(response.rect.right() - 14.0, response.rect.center().y);
        ui.painter()
            .circle_filled(center, 8.0, Color32::from_rgb(56, 105, 191));
        ui.painter().line_segment(
            [
                center + Vec2::new(-3.0, -1.0),
                center + Vec2::new(0.0, if expanded { 2.0 } else { -4.0 }),
            ],
            Stroke::new(1.7, Color32::WHITE),
        );
        ui.painter().line_segment(
            [
                center + Vec2::new(0.0, if expanded { 2.0 } else { -4.0 }),
                center + Vec2::new(3.0, -1.0),
            ],
            Stroke::new(1.7, Color32::WHITE),
        );
        if response.clicked() {
            ui.data_mut(|data| data.insert_persisted(id, !expanded));
        }
        if expanded {
            let body_width = (ui.available_width() - 16.0).max(0.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(245, 249, 255))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(172, 195, 230)))
                .inner_margin(egui::Margin::symmetric(7, 9))
                .show(ui, |ui| {
                    ui.set_min_width(body_width);
                    contents(ui);
                });
        }
        return;
    }
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

fn luna_section_title(section: SidebarSection) -> &'static str {
    match section {
        SidebarSection::Tasks => "File and Folder Tasks",
        SidebarSection::Places => "Other Places",
        SidebarSection::Favorites => "Details",
        SidebarSection::Volumes => "Devices and Drives",
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
    let available = context.content_rect().width();
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

#[cfg(test)]
mod luna_tests {
    use super::*;

    #[test]
    fn luna_task_panes_have_distinct_labels() {
        assert_eq!(
            luna_section_title(SidebarSection::Tasks),
            "File and Folder Tasks"
        );
        assert_eq!(luna_section_title(SidebarSection::Places), "Other Places");
        assert_eq!(luna_section_title(SidebarSection::Favorites), "Details");
        assert_eq!(
            luna_section_title(SidebarSection::Volumes),
            "Devices and Drives"
        );
    }

    #[test]
    fn luna_search_folders_and_advanced_tools_have_independent_state() {
        let context = egui::Context::default();
        for panel in [
            LunaPanel::Search,
            LunaPanel::FolderTree,
            LunaPanel::Advanced,
        ] {
            assert!(!luna_panel_open(&context, panel));
        }
        luna_toggle_panel(&context, LunaPanel::FolderTree);
        assert!(luna_panel_open(&context, LunaPanel::FolderTree));
        assert!(!luna_panel_open(&context, LunaPanel::Search));
        assert!(!luna_panel_open(&context, LunaPanel::Advanced));
        luna_toggle_panel(&context, LunaPanel::FolderTree);
        assert!(!luna_panel_open(&context, LunaPanel::FolderTree));
    }
}
