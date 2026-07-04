//! 文件分类统计视图

use egui::Color32;

use crate::application::classify_service::ClassifyReport;
use crate::gui::app::Snapshot;
use crate::gui::theme::ThemeColors;
use crate::gui::widgets;

/// 渲染文件分类统计视图
pub(crate) fn render_classify(
    ui: &mut egui::Ui,
    state: &Snapshot,
    theme: &ThemeColors,
    cached_classify: &Option<ClassifyReport>,
) {
    if state.node.is_none() {
        widgets::render_empty_state(ui, "尚未扫描", "在上方输入路径，或点击「📁 选择」目录后开始");
        return;
    }
    let Some(report) = cached_classify.as_ref() else {
        widgets::render_empty_state(ui, "正在分析...", "分类统计正在后台计算");
        return;
    };

    let max_cat_percent = report
        .by_category
        .iter()
        .map(|c| c.percent)
        .fold(0.0_f64, f64::max)
        .max(0.01);

    egui::ScrollArea::both().show(ui, |ui| {
        ui.heading(
            egui::RichText::new(format!("总文件 {} 个 · 总大小 {}", report.total_files, report.total_size))
                .color(theme.text_primary)
                .strong(),
        );
        ui.add_space(8.0);

        ui.heading(egui::RichText::new("按大类").color(theme.text_primary).strong());
        ui.separator();
        egui::Grid::new("by_cat").striped(true).show(ui, |ui| {
            ui.label("类别");
            ui.label("大小");
            ui.label("占比");
            ui.label("可视化");
            ui.label("文件数");
            ui.end_row();
            for c in &report.by_category {
                let [r, g, b] = c.category.base_color();
                ui.colored_label(Color32::from_rgb(r, g, b), format!("{} {}", c.label, "■"));
                ui.label(
                    egui::RichText::new(c.total_size.to_string())
                        .family(egui::FontFamily::Monospace),
                );
                ui.label(
                    egui::RichText::new(format!("{:.2}%", c.percent))
                        .family(egui::FontFamily::Monospace),
                );
                widgets::bar_cell_color(
                    ui,
                    c.percent,
                    max_cat_percent,
                    Color32::from_rgb(r, g, b),
                    theme,
                );
                ui.label(
                    egui::RichText::new(c.file_count.to_string())
                        .family(egui::FontFamily::Monospace),
                );
                ui.end_row();
            }
        });

        ui.add_space(10.0);
        ui.heading(egui::RichText::new("按扩展名（前 30）").color(theme.text_primary).strong());
        ui.separator();
        egui::Grid::new("by_ext").striped(true).show(ui, |ui| {
            ui.label("扩展名");
            ui.label("大小");
            ui.label("占比");
            ui.label("文件数");
            ui.end_row();
            for e in report.by_extension.iter().take(30) {
                ui.label(&e.extension);
                ui.label(
                    egui::RichText::new(e.total_size.to_string())
                        .family(egui::FontFamily::Monospace),
                );
                ui.label(
                    egui::RichText::new(format!("{:.2}%", e.percent))
                        .family(egui::FontFamily::Monospace),
                );
                ui.label(
                    egui::RichText::new(e.file_count.to_string())
                        .family(egui::FontFamily::Monospace),
                );
                ui.end_row();
            }
        });
    });
}