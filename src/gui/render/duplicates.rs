//! 重复文件检测视图

use crate::application::duplicate_service::DuplicateReport;
use crate::gui::app::Snapshot;
use crate::gui::theme::ThemeColors;
use crate::gui::widgets;

/// 渲染重复文件检测视图
pub(crate) fn render_duplicates(
    ui: &mut egui::Ui,
    state: &Snapshot,
    theme: &ThemeColors,
    cached_duplicates: &Option<DuplicateReport>,
) {
    if state.node.is_none() {
        widgets::render_empty_state(ui, "尚未扫描", "在上方输入路径，或点击「📁 选择」目录后开始");
        return;
    }
    let Some(report) = cached_duplicates.as_ref() else {
        widgets::render_empty_state(ui, "正在分析...", "重复文件检测正在后台计算");
        return;
    };

    egui::ScrollArea::both().show(ui, |ui| {
        if report.groups.is_empty() {
            ui.colored_label(theme.success, "未发现重复文件");
            return;
        }
        ui.heading(
            egui::RichText::new(format!(
                "{} 组重复 · {} 文件 · 浪费 {}",
                report.groups.len(),
                report.total_duplicate_files,
                report.total_wasted
            ))
            .color(theme.danger)
            .strong(),
        );
        ui.separator();
        for (i, g) in report.groups.iter().enumerate() {
            ui.collapsing(
                format!("组 {}：{} × {} (浪费 {})", i + 1, g.paths.len(), g.size, g.wasted),
                |ui| {
                    for p in &g.paths {
                        ui.label(p.display().to_string());
                    }
                },
            );
        }
    });
}