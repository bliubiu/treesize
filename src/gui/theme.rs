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
    /// 进度条底色
    pub progress_track: Color32,
    /// 进度条填充色
    pub progress_fill: Color32,
    /// 分组标签色
    pub group_label: Color32,
}

fn dark_theme() -> ThemeColors {
    ThemeColors {
        // ── 背景层级（自深至浅）──
        bg_primary: Color32::from_rgb(10, 14, 20), // #0A0E14
        bg_surface: Color32::from_rgb(20, 26, 35), // #141A23
        bg_card: Color32::from_rgb(28, 36, 51),    // #1C2433
        bg_hover: Color32::from_rgb(38, 48, 67),   // #263043

        // ── 强调色：青蓝（数据流/冷存储）──
        accent: Color32::from_rgb(0, 188, 212),        // #00BCD4
        accent_hover: Color32::from_rgb(77, 208, 225), // #4DD0E1
        accent_dim: Color32::from_rgb(0, 151, 167),    // #0097A7

        // ── 文字层级 ──
        text_primary: Color32::from_rgb(236, 239, 241),   // #ECEFF1
        text_secondary: Color32::from_rgb(144, 164, 174), // #90A4AE
        text_dim: Color32::from_rgb(84, 110, 122),        // #546E7A

        // ── 边框 ──
        border: Color32::from_rgb(38, 50, 56),       // #263238
        border_light: Color32::from_rgb(55, 71, 79), // #37474F

        // ── 语义色 ──
        success: Color32::from_rgb(102, 187, 106), // #66BB6A
        danger: Color32::from_rgb(239, 83, 80),    // #EF5350
        warn: Color32::from_rgb(255, 183, 77),     // #FFB74D

        // ── Tab ──
        tab_active_bg: Color32::from_rgb(28, 36, 51),
        tab_inactive_bg: Color32::from_rgb(20, 26, 35),
        tab_text_active: Color32::from_rgb(0, 188, 212),
        tab_text_inactive: Color32::from_rgb(144, 164, 174),

        // ── 进度条/柱状条 ──
        bar_track: Color32::from_rgb(38, 50, 56),
        bar_fill: Color32::from_rgb(0, 188, 212),
        progress_track: Color32::from_rgb(28, 36, 51),
        progress_fill: Color32::from_rgb(0, 188, 212),

        // ── 杂项 ──
        group_label: Color32::from_rgb(84, 110, 122),
    }
}

fn light_theme() -> ThemeColors {
    ThemeColors {
        bg_primary: Color32::from_rgb(245, 247, 250), // #F5F7FA
        bg_surface: Color32::from_rgb(255, 255, 255), // #FFFFFF
        bg_card: Color32::from_rgb(236, 239, 241),    // #ECEFF1
        bg_hover: Color32::from_rgb(224, 228, 232),   // #E0E4E8

        accent: Color32::from_rgb(0, 131, 143),       // #00838F
        accent_hover: Color32::from_rgb(0, 172, 193), // #00ACC1
        accent_dim: Color32::from_rgb(77, 208, 225),  // #4DD0E1

        text_primary: Color32::from_rgb(38, 50, 56),     // #263238
        text_secondary: Color32::from_rgb(96, 125, 139), // #607D8B
        text_dim: Color32::from_rgb(144, 164, 174),      // #90A4AE

        border: Color32::from_rgb(207, 216, 220),       // #CFD8DC
        border_light: Color32::from_rgb(221, 230, 235), // #DDE6EB

        success: Color32::from_rgb(67, 160, 71), // #43A047
        danger: Color32::from_rgb(229, 57, 53),  // #E53935
        warn: Color32::from_rgb(245, 124, 0),    // #F57C00

        tab_active_bg: Color32::from_rgb(236, 239, 241),
        tab_inactive_bg: Color32::from_rgb(255, 255, 255),
        tab_text_active: Color32::from_rgb(0, 131, 143),
        tab_text_inactive: Color32::from_rgb(96, 125, 139),

        bar_track: Color32::from_rgb(207, 216, 220),
        bar_fill: Color32::from_rgb(0, 131, 143),
        progress_track: Color32::from_rgb(236, 239, 241),
        progress_fill: Color32::from_rgb(0, 131, 143),

        group_label: Color32::from_rgb(144, 164, 174),
    }
}

pub fn apply_theme(ctx: &Context, dark: bool) -> ThemeColors {
    let colors = if dark { dark_theme() } else { light_theme() };

    let mut visuals = if dark { Visuals::dark() } else { Visuals::light() };

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
