//! 浪费文件检测视图

use egui::Color32;

use crate::application::waste_service::WasteReport;
use crate::gui::app::Snapshot;
use crate::gui::theme::ThemeColors;
use crate::gui::widgets;

/// 渲染浪费文件检测视图
pub(crate) fn render_waste(
    ui: &mut egui::Ui,
    state: &Snapshot,
    theme: &ThemeColors,
    cached_waste: &Option<WasteReport>,
) {
    if state.node.is_none() {
        widgets::render_empty_state(ui, "尚未扫描", "在上方输入路径，或点击「📁 选择」目录后开始");
        return;
    }
    let Some(report) = cached_waste.as_ref() else {
        widgets::render_empty_state(ui, "正在分析...", "浪费检测正在后台计算");
        return;
    };

    egui::ScrollArea::vertical().show(ui, |ui| {
        if report.items.is_empty() {
            ui.colored_label(theme.success, "未发现空间浪费");
            return;
        }

        // 顶部统计卡片
        ui.horizontal(|ui| {
            widgets::stat_card(ui, "浪费项", &report.items.len().to_string(), theme.warn, theme);
            widgets::stat_card(ui, "总浪费空间", &report.total_wasted.to_string(), theme.danger, theme);
            widgets::stat_card(ui, "可自动清理", &report.auto_cleanable.to_string(), theme.success, theme);
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // 按类型分组统计
        ui.heading(egui::RichText::new("按类型统计").color(theme.text_primary).strong());
        ui.add_space(4.0);

        egui::Grid::new("waste_type_grid")
            .striped(true)
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("类型").strong().color(theme.text_primary));
                ui.label(egui::RichText::new("数量").strong().color(theme.text_primary));
                ui.label(egui::RichText::new("大小").strong().color(theme.text_primary));
                ui.label(egui::RichText::new("说明").strong().color(theme.text_primary));
                ui.end_row();

                for summary in &report.by_type {
                    let [r, g, b] = summary.waste_type.color();
                    let color = Color32::from_rgb(r, g, b);

                    ui.colored_label(color, summary.waste_type.label());
                    ui.label(summary.count.to_string());
                    ui.label(summary.total_size.to_string());
                    ui.label(
                        egui::RichText::new(summary.waste_type.description())
                            .color(theme.text_secondary)
                            .small(),
                    );
                    ui.end_row();
                }
            });

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        // 详细浪费项列表
        ui.heading(egui::RichText::new("详细列表").color(theme.text_primary).strong());
        ui.add_space(4.0);

        for (i, item) in report.items.iter().enumerate() {
            let [r, g, b] = item.waste_type.color();
            let color = Color32::from_rgb(r, g, b);

            ui.horizontal(|ui| {
                ui.colored_label(color, "●");
                ui.label(egui::RichText::new(item.waste_type.label()).strong().color(color));
                ui.label(item.size.to_string());
                ui.label(
                    egui::RichText::new(item.path.display().to_string())
                        .color(theme.text_secondary)
                        .small(),
                );
                if let Some(ref note) = item.note {
                    ui.label(egui::RichText::new(note).color(theme.text_dim).small());
                }
            });

            if i >= 99 {
                ui.label(
                    egui::RichText::new(format!("... 还有 {} 项", report.items.len() - 100))
                        .color(theme.text_dim),
                );
                break;
            }
        }
    });
}