//! Presentation-only widgets shared by every rice.
//!
//! Widgets return semantic events; filesystem actions remain exclusively in
//! the Explorer controller. Themes may vary their layout without acquiring
//! write permissions or diverging in keyboard behavior.

use crate::theme::{ShellKind, SidebarSection, SidebarSectionMode, Theme};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

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
    let width = match icon {
        LunaToolbarIcon::Back => 80.0,
        LunaToolbarIcon::Forward => 100.0,
        LunaToolbarIcon::Up => 67.0,
        LunaToolbarIcon::Search => 93.0,
        LunaToolbarIcon::Folders => 97.0,
    };
    let (_, response) = ui
        .add_enabled_ui(enabled, |ui| {
            ui.allocate_exact_size(Vec2::new(width, 35.0), Sense::click())
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let pressed = response.clicked()
        || (enabled
            && response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }));
    let center = Pos2::new(response.rect.left() + 17.0, response.rect.center().y);
    let alpha = if enabled { 1.0 } else { 0.4 };
    let paint = ui.painter();
    if response.hovered() || response.has_focus() {
        paint.rect_filled(response.rect, 3, Color32::from_rgb(218, 228, 243));
    }
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
    paint.text(
        Pos2::new(response.rect.left() + 37.0, response.rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        Color32::from_rgb(38, 45, 56).gamma_multiply(alpha),
    );
    pressed
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

#[derive(Clone, Copy)]
pub enum AeroNavIcon {
    Favorite,
    Library,
    Network,
}

/// Explorer tree rows are full-width, left-aligned and keyboard-activatable.
/// Their original vector markers avoid OS-owned assets and font glyph gaps.
pub fn aero_sidebar_link(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    icon: AeroNavIcon,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(80.0), 26.0),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let painter = ui.painter().with_clip_rect(rect);
    if selected || response.hovered() || response.has_focus() {
        painter.rect_filled(
            rect.shrink(1.0),
            2,
            if selected {
                Color32::from_rgb(196, 229, 249)
            } else {
                Color32::from_rgb(227, 241, 251)
            },
        );
        painter.rect_stroke(
            rect.shrink(1.0),
            2,
            Stroke::new(1.0, Color32::from_rgb(142, 189, 215)),
            egui::StrokeKind::Inside,
        );
    }
    let point = |x: f32, y: f32| Pos2::new(rect.left() + x, rect.top() + y);
    match icon {
        AeroNavIcon::Favorite => {
            painter.circle_filled(point(14.0, 13.0), 6.5, Color32::from_rgb(228, 171, 49));
            painter.circle_filled(point(12.5, 11.0), 2.5, Color32::from_rgb(255, 239, 155));
        }
        AeroNavIcon::Library => {
            for (x, color) in [
                (8.0, Color32::from_rgb(73, 135, 197)),
                (12.0, Color32::from_rgb(99, 162, 100)),
                (16.0, Color32::from_rgb(221, 174, 72)),
            ] {
                painter.rect_filled(
                    Rect::from_min_max(point(x, 7.0), point(x + 4.0, 20.0)),
                    1,
                    color,
                );
                painter.line_segment(
                    [point(x + 1.0, 10.0), point(x + 3.0, 10.0)],
                    Stroke::new(1.0, Color32::WHITE),
                );
            }
        }
        AeroNavIcon::Network => {
            painter.line_segment(
                [point(11.0, 9.0), point(19.0, 14.0)],
                Stroke::new(2.0, Color32::from_rgb(78, 139, 180)),
            );
            for (x, y) in [(10.0, 9.0), (20.0, 14.0), (10.0, 19.0)] {
                painter.circle_filled(point(x, y), 3.4, Color32::from_rgb(86, 158, 198));
                painter.circle_stroke(point(x, y), 3.4, Stroke::new(1.0, Color32::WHITE));
            }
        }
    }
    painter.text(
        point(29.0, 13.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        Color32::from_rgb(36, 55, 72),
    );
    let activated = response.clicked()
        || (response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }));
    response.on_hover_text(label);
    activated
}

#[derive(Clone, Copy)]
pub enum Win10NavIcon {
    QuickAccess,
    Folder,
    Computer,
    Network,
}

/// Compact, left-aligned navigation used by the Windows 10 Explorer shell.
/// Icons and selection markers are drawn in egui, not borrowed from Windows.
pub fn win10_sidebar_link(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    selected: bool,
    icon: Win10NavIcon,
) -> bool {
    let response = win10_sidebar_response(ui, theme, label, selected, icon);
    win10_sidebar_activated(ui, &response)
}

pub fn win10_sidebar_activated(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.clicked()
        || (response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }))
}

/// Return the full response when a row also needs a device context menu.
pub fn win10_sidebar_response(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    selected: bool,
    icon: Win10NavIcon,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(85.0), 28.0),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let painter = ui.painter().with_clip_rect(rect);
    if selected || response.hovered() || response.has_focus() {
        painter.rect_filled(
            rect,
            0,
            if selected {
                theme.palette.selection()
            } else {
                Color32::from_rgb(236, 242, 247)
            },
        );
        if selected {
            painter.rect_filled(
                Rect::from_min_max(rect.left_top(), rect.left_bottom() + Vec2::new(3.0, 0.0)),
                0,
                theme.palette.accent(),
            );
        }
    }
    let center = Pos2::new(rect.left() + 15.0, rect.center().y);
    let blue = theme.palette.accent();
    match icon {
        Win10NavIcon::QuickAccess => {
            painter.circle_filled(center, 6.0, Color32::from_rgb(215, 167, 55));
            painter.line_segment(
                [center + Vec2::new(0.0, -4.0), center + Vec2::new(0.0, 4.0)],
                Stroke::new(1.8, Color32::WHITE),
            );
        }
        Win10NavIcon::Folder => {
            painter.rect_filled(
                Rect::from_center_size(center + Vec2::new(-3.0, -4.0), Vec2::new(13.0, 6.0)),
                1,
                Color32::from_rgb(225, 174, 72),
            );
            painter.rect_filled(
                Rect::from_center_size(center + Vec2::new(0.0, 2.0), Vec2::new(20.0, 12.0)),
                1,
                Color32::from_rgb(241, 197, 89),
            );
        }
        Win10NavIcon::Computer => {
            painter.rect_stroke(
                Rect::from_center_size(center + Vec2::new(0.0, -2.0), Vec2::new(20.0, 13.0)),
                1,
                Stroke::new(2.0, blue),
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [center + Vec2::new(-6.0, 8.0), center + Vec2::new(6.0, 8.0)],
                Stroke::new(2.2, blue),
            );
        }
        Win10NavIcon::Network => {
            for (x, y) in [(-6.0, -6.0), (6.0, -2.0), (0.0, 7.0)] {
                painter.circle_filled(center + Vec2::new(x, y), 3.8, blue);
            }
            for (a, b) in [((-6.0, -6.0), (6.0, -2.0)), ((6.0, -2.0), (0.0, 7.0))] {
                painter.line_segment(
                    [center + Vec2::new(a.0, a.1), center + Vec2::new(b.0, b.1)],
                    Stroke::new(1.5, blue),
                );
            }
        }
    }
    painter.text(
        Pos2::new(rect.left() + 31.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        theme.palette.text(),
    );
    response.on_hover_text(label)
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
    let title = if theme.shell_kind() == ShellKind::Chromeos {
        match section {
            SidebarSection::Tasks => "Tools",
            SidebarSection::Places => "My files",
            SidebarSection::Favorites => "Shortcuts",
            SidebarSection::Volumes => "Storage & network",
        }
    } else if theme.shell_kind() == ShellKind::Finder {
        match section {
            SidebarSection::Tasks => "Tools",
            SidebarSection::Places => "Places",
            SidebarSection::Favorites => "Favorites",
            SidebarSection::Volumes => "Locations",
        }
    } else if theme.shell_kind() == ShellKind::Aqua {
        match section {
            SidebarSection::Tasks => "CREATIVE TOOLS",
            SidebarSection::Places => "MY SPACES",
            SidebarSection::Favorites => "FAVORITE PLACES",
            SidebarSection::Volumes => "CONNECTED WORLDS",
        }
    } else if theme.shell_kind() == ShellKind::Ribbon {
        match section {
            SidebarSection::Tasks => "File tasks",
            SidebarSection::Places => "This PC",
            SidebarSection::Favorites => "Quick access",
            SidebarSection::Volumes => "Network & drives",
        }
    } else if theme.shell_kind() == ShellKind::Aero {
        match section {
            SidebarSection::Tasks => "File tasks",
            SidebarSection::Places => "Libraries",
            SidebarSection::Favorites => "Favorites",
            SidebarSection::Volumes => "Computer & Network",
        }
    } else {
        match section {
            SidebarSection::Tasks => "FILE TASKS",
            SidebarSection::Places => "PLACES",
            SidebarSection::Favorites => "FAVORITES",
            SidebarSection::Volumes => "VOLUMES",
        }
    };
    if theme.sidebar_section_mode() == SidebarSectionMode::Collapsible {
        egui::CollapsingHeader::new(title)
            .id_salt((theme.id.as_str(), index))
            .default_open(true)
            .show(ui, contents);
    } else {
        ui.label(
            egui::RichText::new(title)
                .size(
                    if matches!(
                        theme.shell_kind(),
                        ShellKind::Aero
                            | ShellKind::Ribbon
                            | ShellKind::Aqua
                            | ShellKind::Finder
                            | ShellKind::Chromeos
                    ) {
                        12.5
                    } else {
                        10.5
                    },
                )
                .strong()
                .color(
                    if matches!(
                        theme.shell_kind(),
                        ShellKind::Aero
                            | ShellKind::Ribbon
                            | ShellKind::Aqua
                            | ShellKind::Finder
                            | ShellKind::Chromeos
                    ) {
                        theme.palette.text()
                    } else {
                        theme.palette.muted()
                    },
                ),
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
