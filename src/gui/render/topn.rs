//! 大文件（TopN）视图渲染

use egui::Color32;

use crate::application::TopNEntry;
use crate::gui::app::{Snapshot, TopFilesSortBy};
use crate::gui::theme::ThemeColors;
use crate::gui::widgets;

/// 渲染大文件（TopN）视图
///
/// 返回 `Some(TopFilesSortBy)` 表示需要切换排序列，由调用者在外部应用。
pub(crate) fn render_topn(
    ui: &mut egui::Ui,
    state: &Snapshot,
    theme: &ThemeColors,
    search: &mut String,
    top_n: &mut usize,
    sort_by: &mut TopFilesSortBy,
    cached_all_files: &Option<Vec<TopNEntry>>,
) -> Option<TopFilesSortBy> {
    if state.node.is_none() {
        widgets::render_empty_state(ui, "尚未扫描", "在上方输入路径，或点击「选择」目录后开始");
        return None;
    }

    if cached_all_files.is_none() {
        widgets::render_empty_state(ui, "正在准备数据…", "请稍候，正在构建文件列表缓存");
        return None;
    }

    // ── 延迟操作标志（解决 ScrollArea 闭包内的借用冲突） ──
    let mut sort_click: Option<TopFilesSortBy> = None;

    // ── 顶部工具栏 ──
    ui.horizontal(|ui| {
        ui.label("查找");
        ui.add_sized(
            [200.0, 0.0],
            egui::TextEdit::singleline(search).hint_text("搜索文件名或路径..."),
        );
        ui.add_space(12.0);
        ui.label("显示");
        let mut top_n_f = *top_n as f32;
        if ui
            .add(
                egui::Slider::new(&mut top_n_f, 10.0..=10_000.0)
                    .clamp_to_range(true)
                    .text("个"),
            )
            .changed()
        {
            *top_n = top_n_f as usize;
        }
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(format!("排序: {}", sort_by.label()))
                .color(theme.text_secondary)
                .weak(),
        );
    });

    // ── 提取只读引用供 ScrollArea 闭包使用 ──
    let all_files = cached_all_files.as_ref().unwrap();
    let search_raw = &*search;
    let search_lower = search_raw.to_ascii_lowercase();
    let search_empty = search_raw.is_empty();

    // ── 搜索过滤 ──
    let filtered: Vec<&TopNEntry> = if search_empty {
        all_files.iter().collect()
    } else {
        all_files
            .iter()
            .filter(|e| {
                e.name.to_ascii_lowercase().contains(&search_lower)
                    || e.path.to_ascii_lowercase().contains(&search_lower)
            })
            .collect()
    };

    // ── 排序列 ──
    let mut sorted = filtered;
    let current_sort = sort_by.clone();
    match current_sort {
        TopFilesSortBy::SizeDesc => sorted.sort_by(|a, b| b.size.cmp(&a.size)),
        TopFilesSortBy::SizeAsc => sorted.sort_by(|a, b| a.size.cmp(&b.size)),
        TopFilesSortBy::NameAsc => sorted.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())),
        TopFilesSortBy::NameDesc => {
            sorted.sort_by(|a, b| b.name.to_ascii_lowercase().cmp(&a.name.to_ascii_lowercase()))
        },
        TopFilesSortBy::PercentDesc => {
            sorted.sort_by(|a, b| b.percent.partial_cmp(&a.percent).unwrap_or(std::cmp::Ordering::Equal))
        },
        TopFilesSortBy::PercentAsc => {
            sorted.sort_by(|a, b| a.percent.partial_cmp(&b.percent).unwrap_or(std::cmp::Ordering::Equal))
        },
        TopFilesSortBy::CategoryAsc => sorted.sort_by(|a, b| a.category.label().cmp(b.category.label())),
        TopFilesSortBy::CategoryDesc => sorted.sort_by(|a, b| b.category.label().cmp(a.category.label())),
        TopFilesSortBy::ModifiedDesc => sorted.sort_by(|a, b| {
            b.modified
                .as_deref()
                .unwrap_or("")
                .cmp(a.modified.as_deref().unwrap_or(""))
        }),
        TopFilesSortBy::ModifiedAsc => sorted.sort_by(|a, b| {
            a.modified
                .as_deref()
                .unwrap_or("")
                .cmp(b.modified.as_deref().unwrap_or(""))
        }),
    }

    let display_count = (*top_n).min(sorted.len());
    sorted.truncate(display_count);
    let all_files_len = all_files.len();
    let displayed_len = sorted.len();

    // ── 内容区域（ScrollArea + 表头 + 数据行） ──
    egui::ScrollArea::both().show(ui, |ui| {
        // 信息行
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("共 {} 项", all_files_len))
                    .color(theme.text_secondary)
                    .weak(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("已显示 {} 项", displayed_len))
                        .color(theme.text_secondary)
                        .weak(),
                );
            });
        });

        ui.add_space(2.0);
        ui.separator();
        ui.add_space(2.0);

        // ── 表头（可点击排序列） ──
        ui.horizontal(|ui| {
            ui.add_sized([30.0, 22.0], egui::Label::new(egui::RichText::new("#").strong()));

            let name_arrow = file_sort_arrow(sort_by, TopFilesSortBy::NameAsc);
            let name_resp = ui.add_sized(
                [180.0, 22.0],
                egui::Label::new(egui::RichText::new(format!("名称{}", name_arrow)).strong()),
            );
            if name_resp.clicked() {
                sort_click = Some(TopFilesSortBy::NameAsc);
            }

            let size_arrow = file_sort_arrow(sort_by, TopFilesSortBy::SizeDesc);
            let size_resp = ui.add_sized(
                [110.0, 22.0],
                egui::Label::new(egui::RichText::new(format!("大小{}", size_arrow)).strong()),
            );
            if size_resp.clicked() {
                sort_click = Some(TopFilesSortBy::SizeDesc);
            }

            let pct_arrow = file_sort_arrow(sort_by, TopFilesSortBy::PercentDesc);
            let pct_resp = ui.add_sized(
                [70.0, 22.0],
                egui::Label::new(egui::RichText::new(format!("%{}", pct_arrow)).strong()),
            );
            if pct_resp.clicked() {
                sort_click = Some(TopFilesSortBy::PercentDesc);
            }

            let cat_arrow = file_sort_arrow(sort_by, TopFilesSortBy::CategoryAsc);
            let cat_resp = ui.add_sized(
                [80.0, 22.0],
                egui::Label::new(egui::RichText::new(format!("类别{}", cat_arrow)).strong()),
            );
            if cat_resp.clicked() {
                sort_click = Some(TopFilesSortBy::CategoryAsc);
            }

            let mod_arrow = file_sort_arrow(sort_by, TopFilesSortBy::ModifiedDesc);
            let mod_resp = ui.add_sized(
                [140.0, 22.0],
                egui::Label::new(egui::RichText::new(format!("修改时间{}", mod_arrow)).strong()),
            );
            if mod_resp.clicked() {
                sort_click = Some(TopFilesSortBy::ModifiedDesc);
            }

            ui.add(egui::Label::new(egui::RichText::new("路径").strong()).sense(egui::Sense::click()));
        });

        ui.separator();

        // ── 数据行 ──
        egui::Grid::new("top_files_grid")
            .striped(true)
            .min_col_width(0.0)
            .show(ui, |ui| {
                for (i, entry) in sorted.iter().enumerate() {
                    let rank = format!("{}", i + 1);

                    // 排名
                    ui.add_sized(
                        [30.0, 20.0],
                        egui::Label::new(egui::RichText::new(&rank).color(theme.text_secondary).weak()),
                    );

                    // 名称
                    let name_truncated = if entry.name.chars().count() > 40 {
                        let truncated: String = entry.name.chars().take(37).collect();
                        format!("{}…", truncated)
                    } else {
                        entry.name.clone()
                    };
                    ui.add_sized(
                        [180.0, 20.0],
                        egui::Label::new(egui::RichText::new(&name_truncated).color(theme.text_primary)),
                    );

                    // 大小（等宽）
                    ui.add_sized(
                        [110.0, 20.0],
                        egui::Label::new(
                            egui::RichText::new(entry.size.to_string())
                                .family(egui::FontFamily::Monospace)
                                .color(theme.text_primary),
                        ),
                    );

                    // 百分比
                    let pct_text = format!("{:.2}%", entry.percent);
                    ui.add_sized(
                        [70.0, 20.0],
                        egui::Label::new(
                            egui::RichText::new(&pct_text)
                                .family(egui::FontFamily::Monospace)
                                .color(theme.text_secondary),
                        ),
                    );

                    // 类别（带色标）
                    let [r, g, b] = entry.category.base_color();
                    let cat_color = Color32::from_rgb(r, g, b);
                    ui.add_sized(
                        [80.0, 20.0],
                        egui::Label::new(
                            egui::RichText::new(format!("  ■ {}", entry.category.label())).color(cat_color),
                        ),
                    );

                    // 修改时间
                    let mod_str = entry.modified.as_deref().unwrap_or("-");
                    ui.add_sized(
                        [140.0, 20.0],
                        egui::Label::new(egui::RichText::new(mod_str).color(theme.text_secondary)),
                    );

                    // 路径（截断）
                    let path_short = if entry.path.len() > 80 {
                        format!("…{}", &entry.path[entry.path.len().saturating_sub(77)..])
                    } else {
                        entry.path.clone()
                    };
                    ui.add_sized(
                        [ui.available_width().max(50.0), 20.0],
                        egui::Label::new(egui::RichText::new(&path_short).color(theme.text_secondary)),
                    );

                    // 右键菜单
                    let resp = ui.label("");
                    resp.context_menu(|ui| {
                        if ui.button("复制路径").clicked() {
                            ui.output_mut(|o| o.copied_text = entry.path.clone());
                            ui.close_menu();
                        }
                        if ui.button("复制名称").clicked() {
                            ui.output_mut(|o| o.copied_text = entry.name.clone());
                            ui.close_menu();
                        }
                    });

                    ui.end_row();
                }
            });
    });

    sort_click
}

/// 返回排序列的排序箭头（▲/▼），非排序列返回空
fn file_sort_arrow(sort_by: &TopFilesSortBy, base: TopFilesSortBy) -> &'static str {
    if sort_by.column() != base.column() {
        return "";
    }
    if sort_by.is_asc() {
        " ▲"
    } else {
        " ▼"
    }
}

/// 切换排序列：同列切换方向，不同列设为默认降序（字母列默认升序）
#[allow(dead_code)]
pub(crate) fn toggle_file_sort(sort_by: &mut TopFilesSortBy, base: TopFilesSortBy) {
    if sort_by.column() == base.column() {
        *sort_by = sort_by.toggle();
    } else {
        *sort_by = base;
    }
}
