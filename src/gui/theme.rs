use egui::{Color32, Context, Visuals};

#[derive(Clone, Copy, Debug)]
pub struct ThemeColors {
    pub bg_primary: Color32,
    pub bg_surface: Color32,
    pub bg_card: Color32,
    pub bg_hover: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_dim: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_dim: Color32,
    pub border: Color32,
    pub border_light: Color32,
    pub success: Color32,
    pub danger: Color32,
    pub warn: Color32,
    pub tab_active_bg: Color32,
    pub tab_inactive_bg: Color32,
    pub tab_text_active: Color32,
    pub tab_text_inactive: Color32,
    pub bar_track: Color32,
    pub bar_fill: Color32,
}

fn dark_theme() -> ThemeColors {
    ThemeColors {
        bg_primary: Color32::from_rgb(15, 20, 25),
        bg_surface: Color32::from_rgb(26, 32, 40),
        bg_card: Color32::from_rgb(34, 42, 52),
        bg_hover: Color32::from_rgb(44, 54, 66),
        accent: Color32::from_rgb(79, 195, 247),
        accent_hover: Color32::from_rgb(129, 212, 250),
        accent_dim: Color32::from_rgb(41, 182, 246),
        text_primary: Color32::from_rgb(232, 238, 242),
        text_secondary: Color32::from_rgb(150, 162, 175),
        text_dim: Color32::from_rgb(96, 108, 120),
        border: Color32::from_rgb(42, 52, 64),
        border_light: Color32::from_rgb(58, 70, 84),
        success: Color32::from_rgb(102, 187, 106),
        danger: Color32::from_rgb(239, 83, 80),
        warn: Color32::from_rgb(255, 183, 77),
        tab_active_bg: Color32::from_rgb(34, 42, 52),
        tab_inactive_bg: Color32::from_rgb(26, 32, 40),
        tab_text_active: Color32::from_rgb(79, 195, 247),
        tab_text_inactive: Color32::from_rgb(150, 162, 175),
        bar_track: Color32::from_rgb(42, 52, 64),
        bar_fill: Color32::from_rgb(79, 195, 247),
    }
}

fn light_theme() -> ThemeColors {
    ThemeColors {
        bg_primary: Color32::from_rgb(245, 247, 250),
        bg_surface: Color32::from_rgb(255, 255, 255),
        bg_card: Color32::from_rgb(237, 241, 245),
        bg_hover: Color32::from_rgb(225, 230, 237),
        accent: Color32::from_rgb(2, 119, 189),
        accent_hover: Color32::from_rgb(3, 155, 229),
        accent_dim: Color32::from_rgb(79, 195, 247),
        text_primary: Color32::from_rgb(26, 35, 50),
        text_secondary: Color32::from_rgb(96, 110, 130),
        text_dim: Color32::from_rgb(150, 162, 175),
        border: Color32::from_rgb(220, 226, 234),
        border_light: Color32::from_rgb(235, 240, 245),
        success: Color32::from_rgb(67, 160, 71),
        danger: Color32::from_rgb(229, 57, 53),
        warn: Color32::from_rgb(245, 124, 0),
        tab_active_bg: Color32::from_rgb(237, 241, 245),
        tab_inactive_bg: Color32::from_rgb(255, 255, 255),
        tab_text_active: Color32::from_rgb(2, 119, 189),
        tab_text_inactive: Color32::from_rgb(96, 110, 130),
        bar_track: Color32::from_rgb(220, 226, 234),
        bar_fill: Color32::from_rgb(2, 119, 189),
    }
}

pub fn apply_theme(ctx: &Context, dark: bool) -> ThemeColors {
    let colors = if dark { dark_theme() } else { light_theme() };

    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    visuals.panel_fill = colors.bg_primary;
    visuals.window_fill = colors.bg_surface;
    visuals.window_stroke = egui::Stroke::new(1.0, colors.border);
    visuals.widgets.noninteractive.bg_fill = colors.bg_surface;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, colors.text_secondary);
    visuals.widgets.inactive.bg_fill = colors.bg_card;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, colors.text_primary);
    visuals.widgets.hovered.bg_fill = colors.bg_hover;
    visuals.widgets.active.bg_fill = colors.accent;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(2.0, colors.accent);
    visuals.selection.stroke = egui::Stroke::new(1.0, colors.accent);
    visuals.hyperlink_color = colors.accent;

    ctx.set_visuals(visuals);
    colors
}