//! 文件收集面板

use egui::Color32;

use crate::domain::drag_collector::CollectedItem;
use crate::domain::value_objects::ByteSize;
use crate::gui::theme::ThemeColors;
use crate::gui::widgets;

/// 收集器操作委托（避免传递整个 app 状态）
pub struct CollectDelegates<'a> {
    pub collect_path: &'a mut dyn FnMut(std::path::PathBuf),
    pub delete_marked: &'a mut dyn FnMut(),
}

/// 渲染文件收集面板
pub(crate) fn _render_collect(
    ui: &mut egui::Ui,
    theme: &ThemeColors,
    items: &mut Vec<CollectedItem>,
    delegates: &mut CollectDelegates<'_>,
) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        // ── 文件/目录选择按钮 ──
        ui.horizontal(|ui| {
            if ui.button("添加目录").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    (delegates.collect_path)(path);
                }
            }
            if ui.button("添加文件").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    (delegates.collect_path)(path);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !items.is_empty() {
                    let marked = items.iter().filter(|i| i.marked_for_deletion).count();
                    if marked > 0 {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(format!("删除已标记（{}）", marked)).color(Color32::WHITE),
                                )
                                .fill(theme.danger),
                            )
                            .clicked()
                        {
                            (delegates.delete_marked)();
                        }
                    }
                    if ui.button("清空全部").clicked() {
                        items.clear();
                    }
                }
            });
        });

        ui.add_space(8.0);

        if items.is_empty() {
            ui.add_space(20.0);
            widgets::render_empty_state(
                ui,
                "暂无收集文件",
                "点击上方「添加目录」或「添加文件」按钮选择要管理的项目",
            );
            return;
        }

        // 统计卡片
        let total_size: u64 = items.iter().map(|i| i.size).sum();
        let marked = items.iter().filter(|i| i.marked_for_deletion).count();
        ui.horizontal(|ui| {
            widgets::stat_card(ui, "已收集", &items.len().to_string(), theme.accent, theme);
            widgets::stat_card(
                ui,
                "总大小",
                &ByteSize(total_size).to_string(),
                theme.text_primary,
                theme,
            );
            widgets::stat_card(
                ui,
                "待删除",
                &marked.to_string(),
                if marked > 0 { theme.danger } else { theme.text_secondary },
                theme,
            );
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // 收集项列表
        let mut remove_idx = None;
        let mut toggle_mark_idx = None;
        egui::Grid::new("collect_grid")
            .striped(true)
            .min_col_width(80.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("名称").strong().color(theme.text_primary));
                ui.label(egui::RichText::new("大小").strong().color(theme.text_primary));
                ui.label(egui::RichText::new("类型").strong().color(theme.text_primary));
                ui.label(egui::RichText::new("路径").strong().color(theme.text_primary));
                ui.label(egui::RichText::new("操作").strong().color(theme.text_primary));
                ui.end_row();

                // 快照避免 borrow 冲突
                let snapshot: Vec<_> = items
                    .iter()
                    .map(|item| {
                        (
                            item.name.clone(),
                            item.size,
                            item.is_dir,
                            item.path.clone(),
                            item.marked_for_deletion,
                        )
                    })
                    .collect();

                for (i, (name, size, is_dir, path, marked)) in snapshot.iter().enumerate() {
                    let icon = if *is_dir { "□" } else { "○" };
                    ui.label(format!("{} {}", icon, name));
                    ui.label(ByteSize(*size).to_string());
                    let type_text = if *is_dir { "目录" } else { "文件" };
                    ui.label(type_text);
                    ui.label(
                        egui::RichText::new(path.display().to_string())
                            .color(theme.text_secondary)
                            .small(),
                    );

                    ui.horizontal(|ui| {
                        let mark_label = if *marked { "已标记" } else { "标记删除" };
                        if ui.button(mark_label).clicked() {
                            toggle_mark_idx = Some(i);
                        }
                        if ui.button("移除").clicked() {
                            remove_idx = Some(i);
                        }
                    });

                    ui.end_row();
                }
            });

        // 处理收集项操作
        if let Some(idx) = remove_idx {
            if idx < items.len() {
                items.remove(idx);
            }
        }
        if let Some(idx) = toggle_mark_idx {
            if let Some(item) = items.get_mut(idx) {
                item.marked_for_deletion = !item.marked_for_deletion;
            }
        }
    });
}
