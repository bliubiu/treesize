//! GUI 通用 UI 组件（无 app 状态依赖）

use egui::{Color32, Vec2};

use crate::gui::theme::ThemeColors;

/// 渲染 Tab 按钮：文字 + 计数徽标
pub fn render_tab_button(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    badge: Option<u64>,
    theme: &ThemeColors,
    on_click: impl FnOnce(&mut egui::Ui),
) {
    let bg = if selected {
        theme.tab_active_bg
    } else {
        theme.tab_inactive_bg
    };
    let text_color = if selected {
        theme.tab_text_active
    } else {
        theme.tab_text_inactive
    };

    let button = egui::Button::new(egui::RichText::new(label).color(text_color).strong())
        .fill(bg)
        .rounding(egui::Rounding::same(4.0))
        .min_size(Vec2::new(0.0, 28.0));

    let resp = ui.horizontal(|ui| {
        if ui.add(button).clicked() {
            on_click(ui);
            ui.close_menu();
        }
        if let Some(n) = badge {
            let badge_w = 28.0;
            let badge_rect = ui.allocate_exact_size(Vec2::new(badge_w, 16.0), egui::Sense::hover()).0;
            let painter = ui.painter();
            painter.rect_filled(badge_rect, 8.0, theme.bg_hover);
            painter.text(
                badge_rect.center(),
                egui::Align2::CENTER_CENTER,
                &n.to_string(),
                egui::FontId::proportional(10.0),
                theme.text_primary,
            );
        }
    });

    // 选中态：底部强调色横线
    if selected {
        let rect = resp.response.rect;
        let line_y = rect.max.y - 1.0;
        ui.painter().line_segment(
            [
                egui::pos2(rect.min.x + 2.0, line_y),
                egui::pos2(rect.max.x - 2.0, line_y),
            ],
            egui::Stroke::new(2.0, theme.accent),
        );
    }
}

/// 渲染统计卡片
///
/// `min_width` 为 `Some(w)` 时设置固定最小宽度，`None` 使用默认值 120px
pub fn stat_card(ui: &mut egui::Ui, label: &str, value: &str, accent: Color32, theme: &ThemeColors) {
    stat_card_sized(ui, label, value, accent, theme, None)
}

/// 同 `stat_card` 但支持指定最小宽度
pub fn stat_card_sized(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    accent: Color32,
    theme: &ThemeColors,
    min_width: Option<f32>,
) {
    let w = min_width.unwrap_or(120.0);
    let frame = egui::Frame::none()
        .fill(theme.bg_card)
        .rounding(egui::Rounding::same(4.0))
        .stroke(egui::Stroke::new(1.0, theme.border))
        .inner_margin(egui::Margin::symmetric(12.0, 8.0));

    frame.show(ui, |ui: &mut egui::Ui| {
        ui.set_min_size(Vec2::new(w, 48.0));
        ui.vertical(|ui: &mut egui::Ui| {
            ui.label(egui::RichText::new(label).color(theme.text_secondary).small());
            ui.label(egui::RichText::new(value).color(accent).strong());
        });
    });
}

/// 渲染洞察行
pub fn insight_row(ui: &mut egui::Ui, label: &str, node: Option<&crate::domain::FileNode>, theme: &ThemeColors) {
    ui.label(egui::RichText::new(label).color(theme.text_secondary));
    match node {
        Some(n) => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(n.name.clone()).color(theme.text_primary));
                ui.label(
                    egui::RichText::new(format!("{}", n.size))
                        .color(theme.accent)
                        .family(egui::FontFamily::Monospace),
                );
            });
        },
        None => {
            ui.label(egui::RichText::new("—").color(theme.text_dim));
        },
    }
    ui.end_row();
}

/// 在 ui 中绘制小圆点（状态指示灯）
pub fn painter_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 4.0, color);
}

/// 在 ui 中绘制指定颜色的小圆点
pub fn painter_dot_color(ui: &mut egui::Ui, color: Color32) {
    painter_dot(ui, color);
}

/// 统一空状态（带绘制图标）
pub fn render_empty_state(ui: &mut egui::Ui, title: &str, hint: &str) {
    let center_x = ui.min_rect().center().x;

    // 绘制简单图标：文件夹轮廓 + 搜索标识
    let icon_y = ui.cursor().min.y + 40.0;
    let icon_size = 48.0;
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(center_x, icon_y + icon_size / 2.0),
        egui::Vec2::new(icon_size, icon_size),
    );
    let painter = ui.painter();

    // 文件夹主体（矩形 + 顶部折角）
    let body_color = Color32::from_gray(90);
    let body_rect = egui::Rect::from_min_size(
        egui::pos2(icon_rect.min.x + 4.0, icon_rect.min.y + 10.0),
        egui::Vec2::new(icon_size - 8.0, icon_size - 14.0),
    );
    painter.rect_filled(body_rect, 3.0, body_color);

    // 文件夹标签（小矩形在上方）
    let tab_rect = egui::Rect::from_min_size(
        egui::pos2(icon_rect.min.x + 2.0, icon_rect.min.y + 2.0),
        egui::Vec2::new(18.0, 10.0),
    );
    painter.rect_filled(tab_rect, 2.0, body_color);

    // 问号标识在文件夹中间
    let q_color = Color32::from_gray(160);
    painter.text(
        body_rect.center(),
        egui::Align2::CENTER_CENTER,
        "?",
        egui::FontId::proportional(20.0),
        q_color,
    );

    // 文字部分
    ui.add_space(icon_size + 60.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(title).color(Color32::from_gray(180)).strong());
        ui.add_space(6.0);
        ui.label(egui::RichText::new(hint).color(Color32::from_gray(140)));
    });
}

/// 截断文件名（按字符数）
pub fn truncate_name(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        name.to_string()
    } else if max_chars <= 3 {
        name.chars().take(max_chars).collect()
    } else {
        let prefix: String = name.chars().take(max_chars - 1).collect();
        format!("{}…", prefix)
    }
}

/// 格式化千位分隔数字（如 142530 → "142,530"）
pub fn _format_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// 表格中的占比条 cell（自定义填充色）
pub fn bar_cell_color(ui: &mut egui::Ui, percent: f64, max_percent: f64, fill: Color32, theme: &ThemeColors) {
    let w = 120.0;
    let h = 10.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, theme.bar_track);
    let ratio = (percent / max_percent).clamp(0.0, 1.0) as f32;
    let fill_rect = egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * ratio, rect.height()));
    painter.rect_filled(fill_rect, 2.0, fill);
}
