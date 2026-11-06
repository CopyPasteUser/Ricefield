//! Original Finder-inspired controls for the RiceField macOS shell.
//!
//! This is a presentation-only layer. Navigation and filesystem commands
//! remain owned by Explorer, including confirmations and remote write guards.

use crate::theme::Theme;
use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinderAction {
    Back,
    Forward,
    Up,
    Grid,
    List,
    QuickLook,
    NewFolder,
    Copy,
    Cut,
    Paste,
    Rename,
    Trash,
    Info,
    Pin,
    Split,
    Sidebar,
    Hidden,
    Network,
    Connect,
    Refresh,
    Tools,
    Transfers,
}

impl FinderAction {
    fn label(self) -> &'static str {
        match self {
            Self::Back => "Back",
            Self::Forward => "Forward",
            Self::Up => "Parent folder",
            Self::Grid => "Icon view",
            Self::List => "List view",
            Self::QuickLook => "Quick Look",
            Self::NewFolder => "New folder",
            Self::Copy => "Copy",
            Self::Cut => "Cut",
            Self::Paste => "Paste",
            Self::Rename => "Rename",
            Self::Trash => "Move to Trash",
            Self::Info => "Get Info",
            Self::Pin => "Add to Favorites",
            Self::Split => "Split view",
            Self::Sidebar => "Show sidebar",
            Self::Hidden => "Show hidden files",
            Self::Network => "Browse network",
            Self::Connect => "Connect to server",
            Self::Refresh => "Refresh",
            Self::Tools => "Show advanced tools",
            Self::Transfers => "Transfers & history",
        }
    }
}

#[derive(Clone, Copy)]
pub struct FinderContext {
    pub local: bool,
    pub selection: usize,
    pub back: bool,
    pub forward: bool,
    pub grid: bool,
    pub split: bool,
    pub sidebar: bool,
    pub hidden: bool,
    pub pinned: bool,
    pub tools: bool,
    pub volume_busy: bool,
}

impl FinderContext {
    pub fn enabled(self, action: FinderAction) -> bool {
        match action {
            FinderAction::Back => self.back,
            FinderAction::Forward => self.forward,
            FinderAction::NewFolder
            | FinderAction::Pin
            | FinderAction::Paste
            | FinderAction::Split => self.local,
            FinderAction::Copy | FinderAction::Cut | FinderAction::Rename | FinderAction::Trash => {
                self.local && self.selection > 0
            }
            FinderAction::QuickLook | FinderAction::Info => self.local && self.selection == 1,
            FinderAction::Connect => !self.volume_busy,
            _ => true,
        }
    }

    fn active(self, action: FinderAction) -> bool {
        match action {
            FinderAction::Grid => self.grid,
            FinderAction::List => !self.grid,
            FinderAction::Pin => self.pinned,
            FinderAction::Split => self.split,
            FinderAction::Sidebar => self.sidebar,
            FinderAction::Hidden => self.hidden,
            FinderAction::Tools => self.tools,
            _ => false,
        }
    }

    fn label(self, action: FinderAction) -> &'static str {
        match action {
            FinderAction::Pin if self.pinned => "Remove from Favorites",
            FinderAction::Sidebar if self.sidebar => "Hide sidebar",
            FinderAction::Hidden if self.hidden => "Hide hidden files",
            FinderAction::Tools if self.tools => "Hide advanced tools",
            _ => action.label(),
        }
    }
}

#[derive(Clone, Copy)]
enum ToolbarGlyph {
    Grid,
    List,
    Eye,
    NewFolder,
}

/// The primary Finder toolbar: native window buttons are deliberately absent.
pub fn toolbar(
    ui: &mut egui::Ui,
    theme: &Theme,
    title: &str,
    state: FinderContext,
) -> Option<FinderAction> {
    let mut chosen = None;
    let compact = ui.available_width() < 660.0;
    egui::Frame::new()
        .fill(theme.palette.chrome())
        .inner_margin(egui::Margin::symmetric(7, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                if nav(ui, theme, "<", state.back, "Back (Alt+Left)") {
                    chosen = Some(FinderAction::Back);
                }
                if nav(ui, theme, ">", state.forward, "Forward (Alt+Right)") {
                    chosen = Some(FinderAction::Forward);
                }
                ui.add_space(7.0);
                ui.label(
                    egui::RichText::new(short_title(title, if compact { 20 } else { 52 }))
                        .size(16.0)
                        .strong()
                        .color(theme.palette.text()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    more_menu(ui, state, &mut chosen);
                    if !compact {
                        if glyph_button(
                            ui,
                            theme,
                            ToolbarGlyph::NewFolder,
                            state.local,
                            false,
                            "New folder (Ctrl+N)",
                        ) {
                            chosen = Some(FinderAction::NewFolder);
                        }
                        if glyph_button(
                            ui,
                            theme,
                            ToolbarGlyph::Eye,
                            state.enabled(FinderAction::QuickLook),
                            false,
                            "Quick Look (Space)",
                        ) {
                            chosen = Some(FinderAction::QuickLook);
                        }
                        ui.separator();
                        if glyph_button(
                            ui,
                            theme,
                            ToolbarGlyph::List,
                            true,
                            !state.grid,
                            "List view",
                        ) {
                            chosen = Some(FinderAction::List);
                        }
                        if glyph_button(
                            ui,
                            theme,
                            ToolbarGlyph::Grid,
                            true,
                            state.grid,
                            "Icon view",
                        ) {
                            chosen = Some(FinderAction::Grid);
                        }
                    }
                });
            });
            if compact {
                ui.horizontal(|ui| {
                    if glyph_button(ui, theme, ToolbarGlyph::Grid, true, state.grid, "Icon view") {
                        chosen = Some(FinderAction::Grid);
                    }
                    if glyph_button(
                        ui,
                        theme,
                        ToolbarGlyph::List,
                        true,
                        !state.grid,
                        "List view",
                    ) {
                        chosen = Some(FinderAction::List);
                    }
                    ui.separator();
                    if glyph_button(
                        ui,
                        theme,
                        ToolbarGlyph::Eye,
                        state.enabled(FinderAction::QuickLook),
                        false,
                        "Quick Look (Space)",
                    ) {
                        chosen = Some(FinderAction::QuickLook);
                    }
                    if glyph_button(
                        ui,
                        theme,
                        ToolbarGlyph::NewFolder,
                        state.local,
                        false,
                        "New folder (Ctrl+N)",
                    ) {
                        chosen = Some(FinderAction::NewFolder);
                    }
                });
            }
        });
    chosen
}

/// Optional second strip for TOML custom layouts; all actions are gated by
/// the same remote and selection rules as the primary Finder toolbar.
pub fn secondary_commands(ui: &mut egui::Ui, state: FinderContext) -> Option<FinderAction> {
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for action in [
            FinderAction::NewFolder,
            FinderAction::Copy,
            FinderAction::Paste,
            FinderAction::Rename,
            FinderAction::Info,
            FinderAction::Refresh,
        ] {
            if ui
                .add_enabled(
                    state.enabled(action),
                    egui::Button::new(state.label(action)),
                )
                .clicked()
            {
                chosen = Some(action);
            }
        }
    });
    chosen
}

fn short_title(title: &str, max: usize) -> String {
    if title.chars().count() > max {
        format!(
            "{}…",
            title
                .chars()
                .take(max.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        title.to_owned()
    }
}

fn nav(ui: &mut egui::Ui, theme: &Theme, glyph: &str, enabled: bool, tooltip: &str) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(glyph).size(16.0).strong())
            .min_size(Vec2::new(30.0, 28.0))
            .fill(Color32::from_rgb(249, 250, 252))
            .stroke(Stroke::new(1.0, theme.palette.border()))
            .corner_radius(7),
    )
    .on_hover_text(tooltip)
    .clicked()
}

fn glyph_button(
    ui: &mut egui::Ui,
    theme: &Theme,
    glyph: ToolbarGlyph,
    enabled: bool,
    selected: bool,
    tooltip: &str,
) -> bool {
    let (rect, response) = ui
        .add_enabled_ui(enabled, |ui| {
            ui.allocate_exact_size(Vec2::new(31.0, 28.0), Sense::click())
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, tooltip));
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(
        rect.shrink(0.5),
        7,
        if selected {
            theme.palette.selection()
        } else if response.hovered() || response.has_focus() {
            Color32::WHITE
        } else {
            Color32::from_rgb(248, 249, 251)
        },
    );
    painter.rect_stroke(
        rect.shrink(0.5),
        7,
        Stroke::new(
            1.0,
            if selected {
                theme.palette.accent()
            } else {
                theme.palette.border()
            },
        ),
        egui::StrokeKind::Inside,
    );
    let center = rect.center();
    let ink = if enabled {
        theme.palette.text()
    } else {
        theme.palette.muted().gamma_multiply(0.35)
    };
    paint_toolbar_icon(&painter, center, glyph, ink, theme.palette.folder());
    let clicked = response.clicked()
        || (enabled
            && response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }));
    response.on_hover_text(tooltip);
    clicked
}

fn paint_toolbar_icon(p: &Painter, c: Pos2, glyph: ToolbarGlyph, ink: Color32, folder: Color32) {
    match glyph {
        ToolbarGlyph::Grid => {
            for x in [-6.0, 3.0] {
                for y in [-6.0, 3.0] {
                    p.rect_filled(
                        Rect::from_min_size(c + Vec2::new(x, y), Vec2::splat(6.0)),
                        1.5,
                        ink,
                    );
                }
            }
        }
        ToolbarGlyph::List => {
            for y in [-6.0, 0.0, 6.0] {
                p.circle_filled(c + Vec2::new(-7.0, y), 1.4, ink);
                p.line_segment(
                    [c + Vec2::new(-3.0, y), c + Vec2::new(8.0, y)],
                    Stroke::new(1.6, ink),
                );
            }
        }
        ToolbarGlyph::Eye => {
            p.circle_stroke(c, 6.0, Stroke::new(1.8, ink));
            p.circle_filled(c, 2.7, ink);
            p.line_segment(
                [c + Vec2::new(-10.0, 0.0), c + Vec2::new(-6.0, -4.0)],
                Stroke::new(1.3, ink),
            );
            p.line_segment(
                [c + Vec2::new(6.0, -4.0), c + Vec2::new(10.0, 0.0)],
                Stroke::new(1.3, ink),
            );
        }
        ToolbarGlyph::NewFolder => {
            p.rect_filled(
                Rect::from_min_max(c + Vec2::new(-9.0, -5.0), c + Vec2::new(0.0, -1.0)),
                1,
                folder,
            );
            p.rect_filled(
                Rect::from_min_max(c + Vec2::new(-9.0, -2.0), c + Vec2::new(7.0, 7.0)),
                2,
                folder,
            );
            p.line_segment(
                [c + Vec2::new(8.0, -9.0), c + Vec2::new(8.0, -1.0)],
                Stroke::new(2.0, ink),
            );
            p.line_segment(
                [c + Vec2::new(4.0, -5.0), c + Vec2::new(12.0, -5.0)],
                Stroke::new(2.0, ink),
            );
        }
    }
}

fn more_menu(ui: &mut egui::Ui, state: FinderContext, chosen: &mut Option<FinderAction>) {
    ui.menu_button("More", |ui| {
        for group in [
            &[
                FinderAction::Up,
                FinderAction::Refresh,
                FinderAction::NewFolder,
                FinderAction::Pin,
            ][..],
            &[
                FinderAction::Copy,
                FinderAction::Cut,
                FinderAction::Paste,
                FinderAction::Rename,
                FinderAction::Trash,
                FinderAction::Info,
            ][..],
            &[
                FinderAction::QuickLook,
                FinderAction::Split,
                FinderAction::Sidebar,
                FinderAction::Hidden,
                FinderAction::Tools,
            ][..],
            &[
                FinderAction::Network,
                FinderAction::Connect,
                FinderAction::Transfers,
            ][..],
        ] {
            for &action in group {
                let button = egui::Button::new(state.label(action)).selected(state.active(action));
                if ui.add_enabled(state.enabled(action), button).clicked() {
                    *chosen = Some(action);
                    ui.close();
                }
            }
            ui.separator();
        }
    });
}

#[derive(Clone, Copy)]
pub enum FinderNavIcon {
    Home,
    Folder,
    Downloads,
    Pictures,
    Music,
    Computer,
    Network,
    Favorite,
}

pub fn sidebar_link(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    active: bool,
    icon: FinderNavIcon,
) -> bool {
    let response = sidebar_response(ui, theme, label, active, icon);
    sidebar_activated(ui, &response)
}

pub fn sidebar_activated(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.clicked()
        || (response.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)))
}

pub fn sidebar_response(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    active: bool,
    icon: FinderNavIcon,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(80.0), 28.0),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let p = ui.painter().with_clip_rect(rect);
    if active || response.hovered() || response.has_focus() {
        p.rect_filled(
            rect.shrink(0.5),
            6,
            if active {
                theme.palette.selection()
            } else {
                Color32::from_rgb(249, 250, 253)
            },
        );
    }
    let ink = if active {
        theme.palette.accent()
    } else {
        Color32::from_rgb(75, 133, 195)
    };
    paint_nav_icon(&p, rect.left_center() + Vec2::new(16.0, 0.0), icon, ink);
    p.text(
        rect.left_center() + Vec2::new(34.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        theme.palette.text(),
    );
    response.on_hover_text(label)
}

fn paint_nav_icon(p: &Painter, c: Pos2, icon: FinderNavIcon, ink: Color32) {
    let at = |x: f32, y: f32| c + Vec2::new(x, y);
    match icon {
        FinderNavIcon::Folder | FinderNavIcon::Favorite | FinderNavIcon::Downloads => {
            p.rect_filled(Rect::from_min_max(at(-9.0, -5.0), at(0.0, -1.0)), 2, ink);
            p.rect_filled(Rect::from_min_max(at(-9.0, -2.0), at(9.0, 7.0)), 3, ink);
            if matches!(icon, FinderNavIcon::Downloads) {
                p.line_segment(
                    [at(0.0, -5.0), at(0.0, 3.0)],
                    Stroke::new(1.7, Color32::WHITE),
                );
                p.line_segment(
                    [at(-3.0, 0.0), at(0.0, 3.0)],
                    Stroke::new(1.7, Color32::WHITE),
                );
                p.line_segment(
                    [at(3.0, 0.0), at(0.0, 3.0)],
                    Stroke::new(1.7, Color32::WHITE),
                );
            }
        }
        FinderNavIcon::Home => {
            p.add(egui::Shape::convex_polygon(
                vec![at(-9.0, -1.0), at(0.0, -9.0), at(9.0, -1.0)],
                ink,
                Stroke::NONE,
            ));
            p.rect_filled(Rect::from_min_max(at(-7.0, -1.0), at(7.0, 8.0)), 1, ink);
            p.rect_filled(
                Rect::from_min_max(at(-2.0, 3.0), at(2.0, 8.0)),
                0,
                Color32::WHITE,
            );
        }
        FinderNavIcon::Pictures => {
            p.rect_filled(Rect::from_min_max(at(-9.0, -7.0), at(9.0, 7.0)), 2, ink);
            p.circle_filled(at(4.0, -3.0), 2.2, Color32::WHITE);
            p.add(egui::Shape::convex_polygon(
                vec![
                    at(-7.0, 5.0),
                    at(-2.0, -2.0),
                    at(1.0, 2.0),
                    at(4.0, -1.0),
                    at(8.0, 5.0),
                ],
                Color32::WHITE,
                Stroke::NONE,
            ));
        }
        FinderNavIcon::Music => {
            p.circle_filled(at(-5.0, 6.0), 3.2, ink);
            p.circle_filled(at(4.0, 4.0), 3.2, ink);
            p.line_segment([at(-2.0, 5.0), at(-2.0, -8.0)], Stroke::new(2.0, ink));
            p.line_segment([at(7.0, 3.0), at(7.0, -8.0)], Stroke::new(2.0, ink));
            p.line_segment([at(-2.0, -8.0), at(7.0, -8.0)], Stroke::new(2.0, ink));
        }
        FinderNavIcon::Computer => {
            p.rect_stroke(
                Rect::from_min_max(at(-9.0, -7.0), at(9.0, 5.0)),
                2,
                Stroke::new(2.0, ink),
                egui::StrokeKind::Inside,
            );
            p.line_segment([at(-5.0, 8.0), at(5.0, 8.0)], Stroke::new(2.0, ink));
        }
        FinderNavIcon::Network => {
            p.circle_stroke(c, 8.0, Stroke::new(1.6, ink));
            p.line_segment([at(-8.0, 0.0), at(8.0, 0.0)], Stroke::new(1.5, ink));
            p.line_segment([at(0.0, -8.0), at(0.0, 8.0)], Stroke::new(1.5, ink));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> FinderContext {
        FinderContext {
            local: true,
            selection: 0,
            back: false,
            forward: false,
            grid: false,
            split: false,
            sidebar: true,
            hidden: false,
            pinned: false,
            tools: false,
            volume_busy: false,
        }
    }

    #[test]
    fn selection_and_remote_location_gate_mutations() {
        let mut state = context();
        assert!(!state.enabled(FinderAction::Copy));
        assert!(state.enabled(FinderAction::NewFolder));
        state.selection = 1;
        assert!(state.enabled(FinderAction::QuickLook));
        assert!(state.enabled(FinderAction::Info));
        state.selection = 2;
        assert!(!state.enabled(FinderAction::QuickLook));
        assert!(state.enabled(FinderAction::Rename));
        state.local = false;
        for action in [
            FinderAction::Copy,
            FinderAction::Trash,
            FinderAction::NewFolder,
            FinderAction::Paste,
            FinderAction::Info,
            FinderAction::Split,
        ] {
            assert!(!state.enabled(action));
        }
        assert!(state.enabled(FinderAction::Network));
    }

    #[test]
    fn context_sensitive_menu_labels_are_true() {
        let mut state = context();
        assert_eq!(state.label(FinderAction::Pin), "Add to Favorites");
        state.pinned = true;
        assert_eq!(state.label(FinderAction::Pin), "Remove from Favorites");
        assert_eq!(state.label(FinderAction::Sidebar), "Hide sidebar");
        assert_eq!(short_title("Pictures", 6), "Pictu…");
    }
}
