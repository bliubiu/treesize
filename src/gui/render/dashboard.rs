//! 仪表盘视图渲染

use egui::{Color32, Vec2};

use crate::application::classify_service::ClassifyReport;
use crate::application::duplicate_service::DuplicateReport;
use crate::application::TopNEntry;
use crate::domain::value_objects::FileCategory;
use crate::domain::FileNode;
use crate::gui::app::Snapshot;
use crate::gui::theme::ThemeColors;
use crate::gui::widgets;

/// 签名条 / 图例分段着色。
///
/// 目录段使用专用色板（按位置循环取色），避免原本「全部灰青 + 奇偶变暗 15%」导致的相邻目录段难以区分；
/// 文件段沿用分类色。条与图例使用同一函数、同一 `index`，保证两者配色一致。
fn signature_segment_color(is_dir: bool, category: FileCategory, index: usize) -> [u8; 3] {
    if is_dir {
        const DIR_PALETTE: [[u8; 3]; 6] = [
            [0, 151, 167],   // 青（accent_dim）
            [63, 81, 181],   // 靛蓝
            [121, 134, 203], // 柔蓝
            [38, 166, 154],  // 青绿
            [79, 195, 247],  // 天蓝
            [149, 117, 205], // 紫
        ];
        DIR_PALETTE[index % DIR_PALETTE.len()]
    } else {
        category.base_color()
    }
}

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
        widgets::render_empty_state(ui, theme, "尚未扫描", "在上方输入路径，或点击「选择」目录后开始");
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

            widgets::stat_card_sized(ui, "总占用", &total_size.to_string(), theme.accent, theme, Some(200.0));
            widgets::stat_card(ui, "文件数", &total_files.to_string(), theme.success, theme);
            widgets::stat_card(ui, "目录数", &total_dirs.to_string(), theme.warn, theme);
            widgets::stat_card(ui, "耗时", &format!("{} ms", elapsed), theme.text_secondary, theme);
        });

        ui.add_space(8.0);

        // ── 卷级对账（仅卷根扫描有数据）──
        if let Some(r) = &state.reconciliation {
            render_volume_reconciliation(ui, r, theme);
            ui.add_space(8.0);
        }

        // ── 签名元素：Top 8 堆叠条 ──
        render_signature_bar(ui, node, theme, dashboard_hover);

        ui.add_space(8.0);

        // ── 快速洞察 ──
        render_quick_insights(
            ui,
            theme,
            cached_top_files,
            cached_top_dirs,
            cached_duplicates,
            cached_classify,
        );
    });
}

/// 卷级对账卡片：把「卷已用空间」与「本次扫描计入」摆在一起，解释差额来源。
///
/// 只在 `Snapshot.reconciliation` 有值时渲染（扫描根为卷根）。
/// 数字一律不改，只做展示与解释；不可读目录明细用折叠面板，避免上百条刷屏。
fn render_volume_reconciliation(
    ui: &mut egui::Ui,
    r: &crate::infrastructure::VolumeReconciliation,
    theme: &ThemeColors,
) {
    let frame = egui::Frame::none()
        .fill(theme.bg_card)
        .rounding(egui::Rounding::same(4.0))
        .stroke(egui::Stroke::new(1.0_f32, theme.border))
        .inner_margin(egui::Margin::symmetric(10.0, 8.0));

    frame.show(ui, |ui| {
        ui.heading(
            egui::RichText::new(format!("卷级对账 · {}", r.volume_root))
                .color(theme.text_primary)
                .strong(),
        );
        ui.add_space(4.0);

        let size = |b: u64| crate::domain::value_objects::ByteSize(b).to_string();
        egui::Grid::new("volume_reconcile_grid")
            .num_columns(2)
            .spacing([16.0, 3.0])
            .show(ui, |ui| {
                let rows = [
                    ("卷总容量", size(r.total_bytes)),
                    ("卷已用空间", size(r.used_bytes)),
                    ("卷空闲空间", size(r.free_bytes)),
                    (
                        "本次计入",
                        format!("{}（{}）", size(r.accounted_bytes), r.basis_label()),
                    ),
                ];
                for (label, value) in rows {
                    ui.label(egui::RichText::new(label).color(theme.text_secondary));
                    ui.label(egui::RichText::new(value).color(theme.text_primary));
                    ui.end_row();
                }

                // 少算用警示黄、多算用危险红、完全一致用成功绿
                let gap_color = if r.gap_bytes > 0 {
                    theme.warn
                } else if r.gap_bytes < 0 {
                    theme.danger
                } else {
                    theme.success
                };
                ui.label(egui::RichText::new(r.gap_label()).color(theme.text_secondary));
                ui.label(
                    egui::RichText::new(format!(
                        "{}（覆盖率 {:.1}%）",
                        size(r.gap_abs()),
                        r.coverage_percent()
                    ))
                    .color(gap_color)
                    .strong(),
                );
                ui.end_row();
            });

        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "差额不等于扫描出错：卷已用空间还含文件系统元数据、卷影副本/还原点、权限拒绝或独占打开的系统文件；硬链接重复计数则会使计入偏大。",
            )
            .color(theme.text_secondary)
            .small(),
        );

        if r.total_unreadable > 0 {
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("不可读目录：{} 个（点击展开）", r.total_unreadable))
                    .color(theme.warn),
            )
            .id_source("volume_unreadable_dirs")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .id_source("volume_unreadable_scroll")
                    .show(ui, |ui| {
                        for entry in &r.unreadable {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(entry.path.as_str())
                                        .color(theme.text_primary)
                                        .small(),
                                );
                                ui.label(
                                    egui::RichText::new(entry.reason.as_str())
                                        .color(theme.text_secondary)
                                        .small(),
                                );
                            });
                        }
                        if r.total_unreadable > r.unreadable.len() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "... 另有 {} 个未列出",
                                    r.total_unreadable - r.unreadable.len()
                                ))
                                .color(theme.text_secondary)
                                .small(),
                            );
                        }
                    });
            });
        }
    });
}

/// 签名元素：水平堆叠条，按大小比例展示 Top 8 子项
fn render_signature_bar(ui: &mut egui::Ui, node: &FileNode, theme: &ThemeColors, dashboard_hover: &mut Option<usize>) {
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
    children.sort_by_key(|a| std::cmp::Reverse(a.size));
    children.truncate(8);

    let total: u64 = children.iter().map(|c| c.size.0).sum();
    if total == 0 {
        widgets::render_empty_state(ui, theme, "暂无数据", "当前目录下没有可统计的文件");
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
        let [r, g, b] = signature_segment_color(child.is_dir(), child.category, i);
        let color = Color32::from_rgb(r, g, b);
        painter.rect_filled(seg_rect, 0.0, color);

        if let Some(pos) = response.hover_pos() {
            if seg_rect.contains(pos) {
                hover_idx = Some(i);
                painter.rect_stroke(seg_rect, 0.0, egui::Stroke::new(2.0_f32, Color32::WHITE));
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
                let [r, g, b] = signature_segment_color(child.is_dir(), child.category, i);
                let color = Color32::from_rgb(r, g, b);
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
    ui.heading(egui::RichText::new("快速洞察").color(theme.text_primary).strong());
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

                    // 重复文件（仅「重复文件」页触发计算，Dashboard 不自动算）
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
                    } else {
                        // 未分析（用户尚未切换到「重复文件」页）：给出引导，而非整行消失
                        ui.label(egui::RichText::new("重复文件").color(theme.text_secondary));
                        ui.label(egui::RichText::new("未分析（切换到「重复文件」页）").color(theme.text_dim));
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
