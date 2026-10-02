//! 扫描历史趋势与快照对比渲染

use egui::{Color32, Pos2, Rect, Vec2};

use crate::application::models::{SnapshotDiff, TrendReport};
use crate::application::SnapshotDiffService;
use crate::domain::value_objects::ByteSize;
use crate::gui::theme::ThemeColors;
use crate::gui::widgets;
use crate::infrastructure::history_storage::HistoryStorage;

/// 渲染快照对比结果（只读，不修改 app 状态）
pub(crate) fn render_diff_result(ui: &mut egui::Ui, diff: &SnapshotDiff, theme: &ThemeColors) {
    // 总体变化卡片
    ui.horizontal(|ui| {
        let size_color = if diff.size_delta > 0 {
            theme.danger
        } else if diff.size_delta < 0 {
            theme.success
        } else {
            theme.text_secondary
        };

        widgets::stat_card(
            ui,
            "大小变化",
            &format!(
                "{} ({:+.1}%)",
                if diff.size_delta > 0 {
                    format!("+{}", ByteSize(diff.size_delta as u64))
                } else {
                    format!("-{}", ByteSize((-diff.size_delta) as u64))
                },
                diff.size_delta_pct
            ),
            size_color,
            theme,
        );

        let file_color = if diff.file_delta > 0 {
            theme.warn
        } else if diff.file_delta < 0 {
            theme.success
        } else {
            theme.text_secondary
        };

        widgets::stat_card(ui, "文件数变化", &format!("{:+}", diff.file_delta), file_color, theme);
        widgets::stat_card(
            ui,
            "目录数变化",
            &format!("{:+}", diff.dir_delta),
            theme.text_secondary,
            theme,
        );
    });

    ui.add_space(8.0);

    // 目录变化
    if !diff.dir_diffs.is_empty() {
        ui.label(
            egui::RichText::new("目录大小变化（Top 10）")
                .color(theme.text_primary)
                .strong(),
        );
        ui.add_space(4.0);

        egui::Grid::new("dir_diff_grid")
            .striped(true)
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("目录").strong());
                ui.label(egui::RichText::new("旧大小").strong());
                ui.label(egui::RichText::new("新大小").strong());
                ui.label(egui::RichText::new("变化").strong());
                ui.end_row();

                for dir_diff in diff.dir_diffs.iter().take(10) {
                    ui.label(&dir_diff.name);
                    ui.label(ByteSize(dir_diff.old_size).to_string());
                    ui.label(ByteSize(dir_diff.new_size).to_string());

                    let delta = dir_diff.delta();
                    let color = if delta > 0 {
                        theme.danger
                    } else if delta < 0 {
                        theme.success
                    } else {
                        theme.text_secondary
                    };

                    let delta_text = if delta > 0 {
                        format!("+{}", ByteSize(delta as u64))
                    } else {
                        format!("-{}", ByteSize((-delta) as u64))
                    };

                    ui.label(egui::RichText::new(delta_text).color(color));
                    ui.end_row();
                }
            });

        ui.add_space(8.0);
    }

    // 新增目录
    if !diff.new_dirs.is_empty() {
        ui.label(
            egui::RichText::new(format!("新增目录（{} 个）", diff.new_dirs.len()))
                .color(theme.success)
                .strong(),
        );
        ui.add_space(4.0);

        for dir in diff.new_dirs.iter().take(5) {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("+").color(theme.success));
                ui.label(&dir.name);
                ui.label(egui::RichText::new(ByteSize(dir.new_size).to_string()).color(theme.text_secondary));
            });
        }

        ui.add_space(8.0);
    }

    // 消失目录
    if !diff.removed_dirs.is_empty() {
        ui.label(
            egui::RichText::new(format!("消失目录（{} 个）", diff.removed_dirs.len()))
                .color(theme.danger)
                .strong(),
        );
        ui.add_space(4.0);

        for dir in diff.removed_dirs.iter().take(5) {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("-").color(theme.danger));
                ui.label(&dir.name);
                ui.label(egui::RichText::new(ByteSize(dir.old_size).to_string()).color(theme.text_secondary));
            });
        }

        ui.add_space(8.0);
    }

    // 分类变化
    if !diff.category_diffs.is_empty() {
        ui.label(egui::RichText::new("文件分类变化").color(theme.text_primary).strong());
        ui.add_space(4.0);

        egui::Grid::new("cat_diff_grid")
            .striped(true)
            .min_col_width(100.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("分类").strong());
                ui.label(egui::RichText::new("大小变化").strong());
                ui.label(egui::RichText::new("文件数变化").strong());
                ui.end_row();

                for cat_diff in diff.category_diffs.iter().take(10) {
                    ui.label(&cat_diff.category);

                    let size_delta = cat_diff.size_delta();
                    let size_color = if size_delta > 0 {
                        theme.danger
                    } else if size_delta < 0 {
                        theme.success
                    } else {
                        theme.text_secondary
                    };

                    let size_text = if size_delta > 0 {
                        format!("+{}", ByteSize(size_delta as u64))
                    } else {
                        format!("-{}", ByteSize((-size_delta) as u64))
                    };

                    ui.label(egui::RichText::new(size_text).color(size_color));

                    let count_delta = cat_diff.count_delta();
                    let count_color = if count_delta > 0 {
                        theme.warn
                    } else if count_delta < 0 {
                        theme.success
                    } else {
                        theme.text_secondary
                    };

                    ui.label(egui::RichText::new(format!("{:+}", count_delta)).color(count_color));
                    ui.end_row();
                }
            });
    }
}

/// 趋势折线图（免费函数）
pub(crate) fn trend_line_chart(ui: &mut egui::Ui, trend: &TrendReport, theme: &ThemeColors) {
    let data = &trend.size_trend;
    if data.len() < 2 {
        return;
    }

    let size = ui.available_size();
    let chart_height = (size.y * 0.6).max(160.0).min(400.0);
    let (rect, _response) = ui.allocate_exact_size(Vec2::new(size.x, chart_height), egui::Sense::hover());

    let painter = ui.painter();
    let margin = 60.0;
    let plot_rect = Rect::from_min_size(
        Pos2::new(rect.min.x + margin, rect.min.y + 10.0),
        Vec2::new(rect.width() - margin - 10.0, chart_height - 40.0),
    );

    let min_size = data.iter().map(|p| p.total_size).min().unwrap_or(0);
    let max_size = data.iter().map(|p| p.total_size).max().unwrap_or(1);
    let size_range = (max_size - min_size).max(1) as f64;

    // 背景网格
    painter.rect_stroke(plot_rect, 0.0, egui::Stroke::new(1.0, theme.border_light));

    // Y 轴标签
    for i in 0..=4 {
        let y = plot_rect.max.y - (plot_rect.height() * i as f32 / 4.0);
        let val = min_size + (size_range as u64 * i as u64 / 4);
        painter.text(
            Pos2::new(rect.min.x + 4.0, y),
            egui::Align2::LEFT_CENTER,
            ByteSize(val).to_string(),
            egui::FontId::proportional(10.0),
            theme.text_dim,
        );
        if i > 0 {
            painter.line_segment(
                [Pos2::new(plot_rect.min.x, y), Pos2::new(plot_rect.max.x, y)],
                egui::Stroke::new(1.0, Color32::from_rgba_premultiplied(200, 200, 200, 30)),
            );
        }
    }

    // X 轴标签
    for (i, point) in data.iter().enumerate() {
        if data.len() > 10 && i % (data.len() / 10).max(1) != 0 && i != data.len() - 1 {
            continue;
        }
        let x = plot_rect.min.x + plot_rect.width() * i as f32 / (data.len() - 1).max(1) as f32;
        painter.text(
            Pos2::new(x, plot_rect.max.y + 14.0),
            egui::Align2::CENTER_CENTER,
            point.date.format("%m-%d").to_string(),
            egui::FontId::proportional(9.0),
            theme.text_dim,
        );
    }

    // 折线数据点
    let points: Vec<Pos2> = data
        .iter()
        .map(|p| {
            let x = plot_rect.min.x
                + plot_rect.width() * (p.total_size.saturating_sub(min_size)) as f32 / size_range as f32;
            let ratio = (p.total_size.saturating_sub(min_size)) as f64 / size_range;
            let y = plot_rect.max.y - (ratio as f32 * plot_rect.height());
            Pos2::new(x, y)
        })
        .collect();

    if points.len() >= 2 {
        let fill_color = Color32::from_rgba_premultiplied(79, 195, 247, 40);
        for i in 0..points.len() - 1 {
            let p0 = points[i];
            let p1 = points[i + 1];
            let bottom_y = plot_rect.max.y;
            painter.add(egui::Shape::convex_polygon(
                vec![p0, p1, Pos2::new(p1.x, bottom_y), Pos2::new(p0.x, bottom_y)],
                fill_color,
                egui::Stroke::NONE,
            ));
        }

        for i in 0..points.len() - 1 {
            painter.line_segment([points[i], points[i + 1]], egui::Stroke::new(2.0, theme.accent));
        }

        for (i, &p) in points.iter().enumerate() {
            let fill = if i == points.len() - 1 {
                theme.accent
            } else {
                theme.accent_dim
            };
            painter.circle_filled(p, 3.0, fill);
            painter.circle_stroke(p, 3.0, egui::Stroke::new(1.0, Color32::WHITE));
        }
    }

    painter.text(
        Pos2::new(plot_rect.center().x, rect.min.y + 2.0),
        egui::Align2::CENTER_TOP,
        "空间占用趋势",
        egui::FontId::proportional(12.0),
        theme.text_primary,
    );
}

/// 完整的趋势面板：统计卡片 + 趋势图 + 快照对比
pub(crate) fn render_trend_panel(ui: &mut egui::Ui, trend: &TrendReport, theme: &ThemeColors) {
    if trend.size_trend.is_empty() {
        widgets::render_empty_state(ui, "暂无扫描历史", "还没有该路径的历史记录，完成一次扫描后自动生成");
        return;
    }

    // 统计卡片
    ui.horizontal(|ui| {
        let first = &trend.size_trend[0];
        let last = trend.size_trend.last().unwrap();
        let growth = last.total_size.saturating_sub(first.total_size);
        let pct = if first.total_size > 0 {
            (growth as f64 / first.total_size as f64) * 100.0
        } else {
            0.0
        };
        widgets::stat_card(ui, "扫描次数", &trend.snapshots.len().to_string(), theme.accent, theme);
        widgets::stat_card(
            ui,
            "首次",
            &first.date.format("%Y-%m-%d").to_string(),
            theme.text_secondary,
            theme,
        );
        widgets::stat_card(
            ui,
            "最近",
            &last.date.format("%Y-%m-%d").to_string(),
            theme.text_secondary,
            theme,
        );
        widgets::stat_card(
            ui,
            "增长",
            &format!("{} ({:.1}%)", ByteSize(growth), pct),
            theme.warn,
            theme,
        );
    });

    ui.add_space(8.0);
    trend_line_chart(ui, trend, theme);
    ui.add_space(16.0);
    ui.separator();
    ui.add_space(8.0);
}

/// 快照对比选择面板
pub(crate) fn render_snapshot_diff_panel(
    ui: &mut egui::Ui,
    trend: &TrendReport,
    theme: &ThemeColors,
    diff_old_selected: &mut Option<i64>,
    diff_new_selected: &mut Option<i64>,
    cached_diff: &mut Option<SnapshotDiff>,
    history_storage: &Option<HistoryStorage>,
) {
    ui.heading(egui::RichText::new("快照对比").color(theme.text_primary).strong());
    ui.add_space(4.0);

    if trend.snapshots.len() < 2 {
        ui.label(egui::RichText::new("需要至少 2 次扫描记录才能进行对比").color(theme.text_secondary));
        return;
    }

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("旧快照：").color(theme.text_primary));
        egui::ComboBox::from_id_source("diff_old_combo")
            .selected_text(
                diff_old_selected
                    .and_then(|id| trend.snapshots.iter().find(|s| s.id == Some(id)))
                    .map(|s| s.scanned_at.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "选择...".to_string()),
            )
            .show_ui(ui, |ui: &mut egui::Ui| {
                for snap in &trend.snapshots {
                    if let Some(id) = snap.id {
                        let label = snap.scanned_at.format("%Y-%m-%d %H:%M").to_string();
                        ui.selectable_value(diff_old_selected, Some(id), label);
                    }
                }
            });

        ui.add_space(16.0);

        ui.label(egui::RichText::new("新快照：").color(theme.text_primary));
        egui::ComboBox::from_id_source("diff_new_combo")
            .selected_text(
                diff_new_selected
                    .and_then(|id| trend.snapshots.iter().find(|s| s.id == Some(id)))
                    .map(|s| s.scanned_at.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "选择...".to_string()),
            )
            .show_ui(ui, |ui: &mut egui::Ui| {
                for snap in &trend.snapshots {
                    if let Some(id) = snap.id {
                        let label = snap.scanned_at.format("%Y-%m-%d %H:%M").to_string();
                        ui.selectable_value(diff_new_selected, Some(id), label);
                    }
                }
            });

        ui.add_space(16.0);

        if ui.button("对比").clicked() {
            if let (Some(old_id), Some(new_id)) = (*diff_old_selected, *diff_new_selected) {
                if let Some(ref storage) = history_storage {
                    match storage.get_diff_data(old_id, new_id) {
                        Ok((old, new, old_cats, new_cats, old_dirs, new_dirs)) => {
                            let diff =
                                SnapshotDiffService::compare(&old, &new, &old_cats, &new_cats, &old_dirs, &new_dirs);
                            *cached_diff = Some(diff);
                        },
                        Err(e) => {
                            tracing::error!("快照对比失败：{e}");
                        },
                    }
                }
            }
        }
    });

    ui.add_space(8.0);

    if let Some(ref diff) = *cached_diff {
        render_diff_result(ui, diff, theme);
    }
}
