//! Treemap 可视化
//!
//! 实现 Squarified Treemap 算法，使用 egui Painter 自绘。
//! 参考：Bruls, Huijsing, van Wijk - "Squarified Treemaps" (2000)
//!
//! 优化策略：
//! 1. 最大渲染节点数限制（MAX_NODES = 5000）
//! 2. 层次细节（LOD）：深度超过 6 层或单元格过小则合并显示

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Ui, Vec2};

use crate::domain::file_node::FileNode;
use crate::domain::value_objects::FileCategory;

use super::theme::ThemeColors;

/// 最大渲染节点数（防止性能问题）
const MAX_NODES: usize = 5000;

/// Treemap 视图
pub struct TreemapView;

impl TreemapView {
    /// 在给定 UI 中绘制 Treemap
    pub fn show(ui: &mut Ui, root: &FileNode, theme: &ThemeColors) {
        let available = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(available, Sense::click());

        if root.size.0 == 0 || rect.width() < 10.0 || rect.height() < 10.0 {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "无数据或区域过小",
                egui::FontId::proportional(14.0),
                theme.text_secondary,
            );
            return;
        }

        let mut painter = ui.painter().clone();

        // 统计节点数，超过限制时显示警告
        let node_count = count_nodes(root);
        if node_count > MAX_NODES {
            // 采样渲染：只渲染最大的子节点
            squarify_with_limit(&mut painter, rect, root, 0, &mut 0);
            let warning = format!("节点过多（{}），已优化渲染", node_count);
            painter.text(
                rect.min + Vec2::new(8.0, rect.height() - 24.0),
                egui::Align2::LEFT_BOTTOM,
                warning,
                egui::FontId::proportional(12.0),
                theme.warn,
            );
        } else {
            squarify(&mut painter, rect, root, 0);
        }

        // 图例移到右下角，半透明背景，避免遮挡主要数据
        let legend_size = Vec2::new(140.0, 180.0);
        let legend_rect = Rect::from_min_size(
            Pos2::new(rect.max.x - legend_size.x - 8.0, rect.max.y - legend_size.y - 8.0),
            legend_size,
        );
        draw_legend(&mut painter, legend_rect, theme);

        // hover 提示：响应整个 rect 的 hover 位置
        if let Some(pos) = response.hover_pos() {
            if let Some(node) = find_node_at(root, rect, pos) {
                let tooltip = format!("{}\n{}", node.name, node.size);
                response.on_hover_text(tooltip);
            }
        }
    }
}

/// Squarified Treemap 核心算法：在 `rect` 内布局 `node` 的子项
fn squarify(painter: &mut egui::Painter, rect: Rect, node: &FileNode, depth: usize) {
    if node.size.0 == 0 || rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }

    // 叶节点或达到最大深度：绘制单元格
    if node.is_file() || node.children.is_empty() || depth >= 6 {
        draw_cell(painter, rect, node, depth);
        return;
    }

    let children: Vec<&FileNode> = node.children.iter().filter(|c| c.size.0 > 0).collect();

    if children.is_empty() {
        draw_cell(painter, rect, node, depth);
        return;
    }

    let total: u64 = children.iter().map(|c| c.size.0).sum();
    if total == 0 {
        return;
    }

    // 归一化面积为像素面积，使用更好的计算顺序减少精度损失
    let area = (rect.width() * rect.height()) as f64;
    let mut items: Vec<(f64, &FileNode)> = children
        .iter()
        .map(|c| {
            // 先计算比例，再乘以总面积，避免大数除法的精度损失
            let ratio = c.size.0 as f64 / total as f64;
            let item_area = ratio * area;
            // 确保最小面积为 1.0 像素，避免小文件完全不可见
            (item_area.max(1.0), *c)
        })
        .collect();
    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    layout_items(painter, rect, &items, depth);
}

/// 在矩形内依次布局 items，按 squarified 算法分行
fn layout_items(painter: &mut egui::Painter, mut rect: Rect, items: &[(f64, &FileNode)], depth: usize) {
    let mut idx = 0;
    while idx < items.len() && rect.width() > 1.0 && rect.height() > 1.0 {
        let short_side = rect.width().min(rect.height()) as f64;
        let mut row: Vec<(f64, &FileNode)> = vec![items[idx]];
        let mut sum: f64 = items[idx].0;
        let mut best = worst_ratio(&row, short_side, sum);
        let mut consumed = 1;

        for j in (idx + 1)..items.len() {
            let mut trial = row.clone();
            trial.push(items[j]);
            let trial_sum = sum + items[j].0;
            let r = worst_ratio(&trial, short_side, trial_sum);
            if r > best {
                break;
            }
            row = trial;
            sum = trial_sum;
            best = r;
            consumed += 1;
        }

        // 放置这一行
        let (row_rect, rest) = place_row(&rect, &row, sum);
        draw_row(painter, row_rect, &row, sum, depth);
        rect = rest;
        idx += consumed;
    }
}

/// 计算行最差宽高比
fn worst_ratio(row: &[(f64, &FileNode)], short_side: f64, sum: f64) -> f64 {
    if sum <= 0.0 || short_side <= 0.0 {
        return f64::MAX;
    }
    let s2 = short_side * short_side;
    let sum2 = sum * sum;
    let mut worst: f64 = 0.0;
    for (a, _) in row {
        if *a <= 0.0 {
            continue;
        }
        let r1 = s2 * a / sum2;
        let r2 = sum2 / (s2 * a);
        worst = worst.max(r1.max(r2));
    }
    worst
}

/// 将行放置在矩形短边方向，返回行占用的矩形与剩余矩形
fn place_row(rect: &Rect, _row: &[(f64, &FileNode)], sum: f64) -> (Rect, Rect) {
    let width = rect.width();
    let height = rect.height();

    if width >= height {
        // 沿左侧切一条竖向带
        let row_w = if height > 0.0 {
            (sum / height as f64).min(width as f64)
        } else {
            0.0
        };
        let row_rect = Rect::from_min_size(rect.min, Vec2::new(row_w as f32, height));
        let rest = Rect::from_min_size(
            Pos2::new(rect.min.x + row_w as f32, rect.min.y),
            Vec2::new((width - row_w as f32).max(0.0), height),
        );
        (row_rect, rest)
    } else {
        // 沿顶部切一条横向带
        let row_h = if width > 0.0 {
            (sum / width as f64).min(height as f64)
        } else {
            0.0
        };
        let row_rect = Rect::from_min_size(rect.min, Vec2::new(width, row_h as f32));
        let rest = Rect::from_min_size(
            Pos2::new(rect.min.x, rect.min.y + row_h as f32),
            Vec2::new(width, (height - row_h as f32).max(0.0)),
        );
        (row_rect, rest)
    }
}

/// 在行矩形内按比例切分各 cell 并绘制
fn draw_row(painter: &mut egui::Painter, row_rect: Rect, row: &[(f64, &FileNode)], sum: f64, depth: usize) {
    if sum <= 0.0 || row.is_empty() {
        return;
    }
    let width = row_rect.width();
    let height = row_rect.height();
    let horizontal = width >= height; // 沿长边排列 cells

    let mut offset = 0.0_f64;
    for (area, child) in row {
        let length = if sum > 0.0 {
            area / sum * (if horizontal { width as f64 } else { height as f64 })
        } else {
            0.0
        };
        let cell = if horizontal {
            Rect::from_min_size(
                Pos2::new(row_rect.min.x + offset as f32, row_rect.min.y),
                Vec2::new(length as f32, height),
            )
        } else {
            Rect::from_min_size(
                Pos2::new(row_rect.min.x, row_rect.min.y + offset as f32),
                Vec2::new(width, length as f32),
            )
        };
        squarify(painter, cell, child, depth + 1);
        offset += length;
    }
}

/// 绘制单元格
fn draw_cell(painter: &mut egui::Painter, rect: Rect, node: &FileNode, depth: usize) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }

    let [r, g, b] = if node.is_dir() {
        darken(FileCategory::Other.base_color(), depth)
    } else {
        node.category.base_color()
    };

    let color = Color32::from_rgb(r, g, b);
    let border = Color32::from_black_alpha(100);

    painter.rect_filled(rect, 0.0, color);
    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, border));

    // 文字标签（仅当区域足够大）
    if rect.width() > 50.0 && rect.height() > 18.0 {
        let label = format!("{}\n{}", node.name, node.size);
        let text_color = if (r as u16 + g as u16 + b as u16) > 384 {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        let font_size = rect.height().min(16.0_f32).max(9.0_f32);
        painter.text(
            rect.min + Vec2::new(3.0, 2.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(font_size),
            text_color,
        );
    }
}

/// 按深度变暗颜色
fn darken([r, g, b]: [u8; 3], depth: usize) -> [u8; 3] {
    let factor = 0.85_f64.powi(depth as i32);
    [
        (r as f64 * factor) as u8,
        (g as f64 * factor) as u8,
        (b as f64 * factor) as u8,
    ]
}

/// 绘制图例
fn draw_legend(painter: &mut egui::Painter, rect: Rect, theme: &ThemeColors) {
    // 使用主题背景色（带透明度），避免完全遮挡数据
    let bg = theme.bg_primary;
    painter.rect_filled(rect, 4.0, Color32::from_rgba_premultiplied(bg.r(), bg.g(), bg.b(), 200));
    painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0, theme.border_light));

    let categories = [
        FileCategory::Video,
        FileCategory::Audio,
        FileCategory::Image,
        FileCategory::Document,
        FileCategory::Archive,
        FileCategory::Executable,
        FileCategory::Source,
        FileCategory::Database,
        FileCategory::System,
        FileCategory::Other,
    ];

    painter.text(
        rect.min + Vec2::new(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        "图例",
        egui::FontId::proportional(12.0),
        theme.text_primary,
    );

    for (i, cat) in categories.iter().enumerate() {
        let y = rect.min.y + 24.0 + i as f32 * 15.0;
        let [r, g, b] = cat.base_color();
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(rect.min.x + 8.0, y), Vec2::new(12.0, 12.0)),
            2.0,
            Color32::from_rgb(r, g, b),
        );
        painter.text(
            Pos2::new(rect.min.x + 26.0, y + 6.0),
            egui::Align2::LEFT_CENTER,
            cat.label(),
            egui::FontId::proportional(11.0),
            theme.text_secondary,
        );
    }
}

/// 在 Treemap 中查找鼠标位置对应的叶节点（用于 hover 提示）
fn find_node_at<'a>(root: &'a FileNode, rect: Rect, pos: Pos2) -> Option<&'a FileNode> {
    if !rect.contains(pos) || root.size.0 == 0 {
        return None;
    }
    find_node_inner(root, rect, pos, 0)
}

fn find_node_inner<'a>(node: &'a FileNode, rect: Rect, pos: Pos2, depth: usize) -> Option<&'a FileNode> {
    if !rect.contains(pos) {
        return None;
    }
    // 叶节点或达到最大深度
    if node.is_file() || node.children.is_empty() || depth >= 6 {
        return Some(node);
    }

    let children: Vec<&FileNode> = node.children.iter().filter(|c| c.size.0 > 0).collect();
    if children.is_empty() {
        return Some(node);
    }

    let total: u64 = children.iter().map(|c| c.size.0).sum();
    if total == 0 {
        return Some(node);
    }

    // 复用 squarify 的布局逻辑查找命中区域
    let area = (rect.width() * rect.height()) as f64;
    let mut items: Vec<(f64, &FileNode)> = children
        .iter()
        .map(|c| {
            // 先计算比例，再乘以总面积，避免大数除法的精度损失
            let ratio = c.size.0 as f64 / total as f64;
            let item_area = ratio * area;
            // 确保最小面积为 1.0 像素，避免小文件完全不可见
            (item_area.max(1.0), *c)
        })
        .collect();
    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut cur_rect = rect;
    let mut idx = 0;
    while idx < items.len() && cur_rect.width() > 1.0 && cur_rect.height() > 1.0 {
        let short_side = cur_rect.width().min(cur_rect.height()) as f64;
        let mut row: Vec<(f64, &FileNode)> = vec![items[idx]];
        let mut sum: f64 = items[idx].0;
        let mut best = worst_ratio(&row, short_side, sum);
        let mut consumed = 1;

        for j in (idx + 1)..items.len() {
            let mut trial = row.clone();
            trial.push(items[j]);
            let trial_sum = sum + items[j].0;
            let r = worst_ratio(&trial, short_side, trial_sum);
            if r > best {
                break;
            }
            row = trial;
            sum = trial_sum;
            best = r;
            consumed += 1;
        }

        let (row_rect, rest) = place_row(&cur_rect, &row, sum);
        // 在这一行内查找命中的 cell
        if let Some(found) = find_in_row(row_rect, &row, sum, pos, depth) {
            return Some(found);
        }
        cur_rect = rest;
        idx += consumed;
    }
    Some(node)
}

/// 在一行内查找命中的 cell
fn find_in_row<'a>(
    row_rect: Rect,
    row: &[(f64, &'a FileNode)],
    sum: f64,
    pos: Pos2,
    depth: usize,
) -> Option<&'a FileNode> {
    if sum <= 0.0 || row.is_empty() {
        return None;
    }
    let width = row_rect.width();
    let height = row_rect.height();
    let horizontal = width >= height;
    let mut offset = 0.0_f64;

    for (area, child) in row {
        let length = if sum > 0.0 {
            area / sum * (if horizontal { width as f64 } else { height as f64 })
        } else {
            0.0
        };
        let cell = if horizontal {
            Rect::from_min_size(
                Pos2::new(row_rect.min.x + offset as f32, row_rect.min.y),
                Vec2::new(length as f32, height),
            )
        } else {
            Rect::from_min_size(
                Pos2::new(row_rect.min.x, row_rect.min.y + offset as f32),
                Vec2::new(width, length as f32),
            )
        };
        if cell.contains(pos) {
            return find_node_inner(child, cell, pos, depth + 1);
        }
        offset += length;
    }
    None
}

/// 统计节点总数
///
/// 利用 `FileNode::aggregate` 预填充的 `file_count`/`dir_count` 缓存，
/// 避免每帧递归遍历整棵树。
fn count_nodes(node: &FileNode) -> usize {
    1usize + node.file_count as usize + node.dir_count as usize
}

/// 带节点数限制的 Squarified Treemap 算法
fn squarify_with_limit(painter: &mut egui::Painter, rect: Rect, node: &FileNode, depth: usize, count: &mut usize) {
    if node.size.0 == 0 || rect.width() < 2.0 || rect.height() < 2.0 || *count >= MAX_NODES {
        return;
    }

    if node.is_file() || node.children.is_empty() || depth >= 4 {
        *count += 1;
        draw_cell(painter, rect, node, depth);
        return;
    }

    let children: Vec<&FileNode> = node.children.iter().filter(|c| c.size.0 > 0).collect();

    if children.is_empty() {
        *count += 1;
        draw_cell(painter, rect, node, depth);
        return;
    }

    let total: u64 = children.iter().map(|c| c.size.0).sum();
    if total == 0 {
        return;
    }

    let area = (rect.width() * rect.height()) as f64;
    let mut items: Vec<(f64, &FileNode)> = children
        .iter()
        .map(|c| {
            // 先计算比例，再乘以总面积，避免大数除法的精度损失
            let ratio = c.size.0 as f64 / total as f64;
            let item_area = ratio * area;
            // 确保最小面积为 1.0 像素，避免小文件完全不可见
            (item_area.max(1.0), *c)
        })
        .collect();
    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let max_items = std::cmp::min(items.len(), (MAX_NODES - *count).max(1));
    layout_items_with_limit(painter, rect, &items[..max_items], depth, count);
}

fn layout_items_with_limit(
    painter: &mut egui::Painter,
    mut rect: Rect,
    items: &[(f64, &FileNode)],
    depth: usize,
    count: &mut usize,
) {
    let mut idx = 0;
    while idx < items.len() && rect.width() > 1.0 && rect.height() > 1.0 && *count < MAX_NODES {
        let short_side = rect.width().min(rect.height()) as f64;
        let mut row: Vec<(f64, &FileNode)> = vec![items[idx]];
        let mut sum: f64 = items[idx].0;
        let mut best = worst_ratio(&row, short_side, sum);
        let mut consumed = 1;

        for j in (idx + 1)..items.len() {
            let mut trial = row.clone();
            trial.push(items[j]);
            let trial_sum = sum + items[j].0;
            let r = worst_ratio(&trial, short_side, trial_sum);
            if r > best {
                break;
            }
            row = trial;
            sum = trial_sum;
            best = r;
            consumed += 1;
        }

        let (row_rect, rest) = place_row(&rect, &row, sum);
        draw_row_with_limit(painter, row_rect, &row, sum, depth, count);
        rect = rest;
        idx += consumed;
    }
}

fn draw_row_with_limit(
    painter: &mut egui::Painter,
    row_rect: Rect,
    row: &[(f64, &FileNode)],
    sum: f64,
    depth: usize,
    count: &mut usize,
) {
    if sum <= 0.0 || row.is_empty() {
        return;
    }
    let width = row_rect.width();
    let height = row_rect.height();
    let horizontal = width >= height;
    let mut offset = 0.0;

    for (size, child) in row {
        let length = if horizontal {
            (*size / sum as f64) * width as f64
        } else {
            (*size / sum as f64) * height as f64
        };

        let cell = if horizontal {
            Rect::from_min_size(
                Pos2::new(row_rect.min.x + offset as f32, row_rect.min.y),
                Vec2::new(length as f32, height),
            )
        } else {
            Rect::from_min_size(
                Pos2::new(row_rect.min.x, row_rect.min.y + offset as f32),
                Vec2::new(width, length as f32),
            )
        };
        squarify_with_limit(painter, cell, child, depth + 1, count);
        offset += length;
    }
}
