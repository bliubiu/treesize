//! 仪表盘视图渲染

use egui::{Color32, Vec2};

use crate::application::classify_service::ClassifyReport;
use crate::application::duplicate_service::DuplicateReport;
use crate::application::TopNEntry;
use crate::domain::FileNode;
use crate::gui::app::Snapshot;
use crate::gui::theme::ThemeColors;
use crate::gui::widgets;

/// 渲染仪表盘主面板
pub(crate) fn render_dashboard(
    ui: &mut egui::Ui,
    state: &Snapshot,
    theme: &ThemeColors,
    dashboard_hover: &mut Option<usize>,
    cached_top_files: &Option<Vec<TopNEntry>>,
    cached_top_dirs: &Option<Vec<TopNEntry>>,
    cached_duplicates: &Option<DuplicateReport>,
    cached_classify: &Option<ClassifyReport>,
) {
    if state.node.is_none() {
        widgets::render_empty_state(ui, "尚未扫描", "在上方输入路径，或点击「📁 选择」目录后开始");
        return;
    }
    let node = state.node.as_ref().unwrap();
    let stats = state.stats.as_ref();

    egui::ScrollArea::vertical().show(ui, |ui| {
        // ── 顶部统计卡片 ──
        ui.horizontal(|ui| {
            let total_size = node.size;
            let total_files = stats.map(|s| s.total_files).unwrap_or(node.file_count);
            let total_dirs = stats.map(|s| s.total_dirs).unwrap_or(node.dir_count);
            let elapsed = stats.map(|s| s.elapsed_ms).unwrap_or(0);

            widgets::stat_card(ui, "总占用", &total_size.to_string(), theme.accent, theme);
            widgets::stat_card(ui, "文件数", &total_files.to_string(), theme.success, theme);
            widgets::stat_card(ui, "目录数", &total_dirs.to_string(), theme.warn, theme);
            widgets::stat_card(ui, "耗时", &format!("{} ms", elapsed), theme.text_secondary, theme);
        });

        ui.add_space(8.0);

        // ── 签名元素：Top 8 堆叠条 ──
        render_signature_bar(ui, node, theme, dashboard_hover);

        ui.add_space(8.0);

        // ── 快速洞察 ──
        render_quick_insights(ui, theme, cached_top_files, cached_top_dirs, cached_duplicates, cached_classify);
    });
}

/// 签名元素：水平堆叠条，按大小比例展示 Top 8 子项
fn render_signature_bar(
    ui: &mut egui::Ui,
    node: &FileNode,
    theme: &ThemeColors,
    dashboard_hover: &mut Option<usize>,
) {
    ui.heading(
        egui::RichText::new("空间分布 · Top 8")
            .color(theme.text_primary)
            .strong(),
    );
    ui.label(
        egui::RichText::new("鼠标悬停色块查看详情")
            .color(theme.text_secondary)
            .small(),
    );
    ui.add_space(4.0);

    // 取 Top 8 子项（目录与文件混合）
    let mut children: Vec<&FileNode> = node.children.iter().filter(|c| c.size.0 > 0).collect();
    children.sort_by(|a, b| b.size.cmp(&a.size));
    children.truncate(8);

    let total: u64 = children.iter().map(|c| c.size.0).sum();
    if total == 0 {
        widgets::render_empty_state(ui, "暂无数据", "当前目录下没有可统计的文件");
        return;
    }

    let available = ui.available_width().min(800.0);
    let bar_height = 36.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(available, bar_height), egui::Sense::hover());

    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, theme.bar_track);

    let mut x = rect.min.x;
    let mut hover_idx: Option<usize> = None;
    for (i, child) in children.iter().enumerate() {
        let ratio = child.size.0 as f32 / total as f32;
        let w = (ratio * rect.width()).max(2.0);
        let seg_rect = egui::Rect::from_min_size(egui::pos2(x, rect.min.y), Vec2::new(w, bar_height));
        let [r, g, b] = if child.is_dir() {
            crate::domain::value_objects::FileCategory::Other.base_color()
        } else {
            child.category.base_color()
        };
        let color = if i % 2 == 0 {
            Color32::from_rgb(r, g, b)
        } else {
            Color32::from_rgb(
                (r as f32 * 0.85) as u8,
                (g as f32 * 0.85) as u8,
                (b as f32 * 0.85) as u8,
            )
        };
        painter.rect_filled(seg_rect, 0.0, color);

        if let Some(pos) = response.hover_pos() {
            if seg_rect.contains(pos) {
                hover_idx = Some(i);
                painter.rect_stroke(seg_rect, 0.0, egui::Stroke::new(2.0, Color32::WHITE));
            }
        }

        if w > 40.0 {
            let label = widgets::truncate_name(&child.name, (w / 7.0) as usize);
            let text_color = if (r as u16 + g as u16 + b as u16) > 384 {
                Color32::BLACK
            } else {
                Color32::WHITE
            };
            painter.text(
                seg_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(11.0),
                text_color,
            );
        }

        x += w;
    }

    *dashboard_hover = hover_idx;

    if let Some(i) = hover_idx {
        if let Some(child) = children.get(i) {
            let percent = (child.size.0 as f64 / node.size.0 as f64) * 100.0;
            let tooltip = format!("{}\n{}  ({:.2}%)", child.name, child.size, percent);
            response.on_hover_text(tooltip);
        }
    }

    ui.add_space(4.0);
    egui::Grid::new("sig_legend")
        .num_columns(2)
        .spacing([16.0, 3.0])
        .show(ui, |ui| {
            for (i, child) in children.iter().enumerate() {
                let [r, g, b] = if child.is_dir() {
                    crate::domain::value_objects::FileCategory::Other.base_color()
                } else {
                    child.category.base_color()
                };
                let color = if i % 2 == 0 {
                    Color32::from_rgb(r, g, b)
                } else {
                    Color32::from_rgb(
                        (r as f32 * 0.85) as u8,
                        (g as f32 * 0.85) as u8,
                        (b as f32 * 0.85) as u8,
                    )
                };
                let percent = (child.size.0 as f64 / node.size.0 as f64) * 100.0;
                ui.horizontal(|ui| {
                    widgets::painter_dot_color(ui, color);
                    ui.label(&child.name);
                    ui.label(
                        egui::RichText::new(format!("{} · {:.1}%", child.size, percent))
                            .color(theme.text_secondary)
                            .small(),
                    );
                });
                ui.end_row();
            }
        });
}

/// 快速洞察：最大文件、最大目录、重复文件、可清理
///
/// 最大文件/目录使用后台缓存的 TopN 报告，避免在主线程递归遍历整个文件树。
fn render_quick_insights(
    ui: &mut egui::Ui,
    theme: &ThemeColors,
    cached_top_files: &Option<Vec<TopNEntry>>,
    cached_top_dirs: &Option<Vec<TopNEntry>>,
    cached_duplicates: &Option<DuplicateReport>,
    cached_classify: &Option<ClassifyReport>,
) {
    ui.heading(
        egui::RichText::new("快速洞察")
            .color(theme.text_primary)
            .strong(),
    );
    ui.add_space(4.0);

    egui::Frame::none()
        .fill(theme.bg_card)
        .rounding(egui::Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .show(ui, |ui| {
            egui::Grid::new("insights")
                .num_columns(2)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    // 最大文件（从 TopN 缓存取第一条）
                    if let Some(files) = cached_top_files {
                        if let Some(top) = files.first() {
                            ui.label(egui::RichText::new("最大文件").color(theme.text_secondary));
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&top.name).color(theme.text_primary));
                                ui.label(
                                    egui::RichText::new(format!("{}", top.size))
                                        .color(theme.accent)
                                        .family(egui::FontFamily::Monospace),
                                );
                            });
                            ui.end_row();
                        }
                    } else {
                        ui.label(egui::RichText::new("最大文件").color(theme.text_secondary));
                        ui.label(egui::RichText::new("计算中…").color(theme.text_dim));
                        ui.end_row();
                    }

                    // 最大目录（从 TopN 缓存取第一条）
                    if let Some(dirs) = cached_top_dirs {
                        if let Some(top) = dirs.first() {
                            ui.label(egui::RichText::new("最大目录").color(theme.text_secondary));
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&top.name).color(theme.text_primary));
                                ui.label(
                                    egui::RichText::new(format!("{}", top.size))
                                        .color(theme.accent)
                                        .family(egui::FontFamily::Monospace),
                                );
                            });
                            ui.end_row();
                        }
                    } else {
                        ui.label(egui::RichText::new("最大目录").color(theme.text_secondary));
                        ui.label(egui::RichText::new("计算中…").color(theme.text_dim));
                        ui.end_row();
                    }

                    // 重复文件
                    if let Some(report) = cached_duplicates {
                        if report.groups.is_empty() {
                            ui.label(egui::RichText::new("重复文件").color(theme.text_secondary));
                            ui.label(egui::RichText::new("未发现重复").color(theme.success));
                        } else {
                            ui.label(egui::RichText::new("重复文件").color(theme.text_secondary));
                            ui.horizontal(|ui| {
                                ui.colored_label(
                                    theme.danger,
                                    format!("{} 组 · 浪费 {}", report.groups.len(), report.total_wasted),
                                );
                            });
                        }
                        ui.end_row();
                    }

                    // 分类 Top
                    if let Some(report) = cached_classify {
                        if let Some(top_cat) = report.by_category.iter().max_by_key(|c| c.total_size.0) {
                            ui.label(egui::RichText::new("主要类型").color(theme.text_secondary));
                            ui.label(format!("{} · {}", top_cat.label, top_cat.total_size));
                            ui.end_row();
                        }
                    }
                });
        });
}