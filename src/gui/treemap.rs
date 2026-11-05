//! Treemap 可视化
//!
//! 实现 Squarified Treemap 算法，使用 egui Painter 自绘。
//! 参考：Bruls, Huijsing, van Wijk - "Squarified Treemaps" (2000)
//!
//! 优化策略：
//! 1. 最大渲染节点数限制（MAX_NODES = 5000）
//! 2. 层次细节（LOD）：深度超过 6 层或单元格过小则合并显示
//! 3. 布局结果按 (代次, 视口尺寸) 缓存，避免每帧重排整棵树；
//!    渲染与 hover 命中检测都复用同一份缓存单元格，消除重复布局逻辑

use std::cmp::Ordering;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Ui, Vec2};

use super::theme::ThemeColors;
use crate::domain::file_node::FileNode;
use crate::domain::value_objects::FileCategory;

/// 最大渲染节点数（防止性能问题）
const MAX_NODES: usize = 5000;

/// 单个缓存单元格（布局结果）。
///
/// 布局时对 `FileNode` 取一次名字/大小/颜色，之后渲染与命中检测都只依赖本结构，
/// 不再触碰文件树，从而把每帧的「递归 + 排序 + 分配」降到纯遍历。
struct TreemapCell {
    rect: Rect,
    name: String,
    size_text: String,
    color: Color32,
    text_color: Color32,
    show_label: bool,
    font_size: f32,
}

/// Treemap 布局缓存：当数据代次（`generation`）或视口尺寸变化时整体失效重排。
pub struct TreemapCache {
    generation: u64,
    viewport: Vec2,
    cells: Vec<TreemapCell>,
}

impl Default for TreemapCache {
    fn default() -> Self {
        Self {
            generation: 0,
            viewport: Vec2::new(0.0, 0.0),
            cells: Vec::new(),
        }
    }
}

impl TreemapCache {
    /// 判断给定代次与视口是否需要重排，并在失效时更新缓存键。
    ///
    /// 失效条件：数据代次变化、视口尺寸变化超过 0.5px（抗窗口抖动）、
    /// 或缓存尚未填充。命中时**不**更新视口基准，避免亚像素抖动逐帧累积成漂移。
    /// 返回 `true` 时调用方须清空并重新填充 `cells`。
    fn needs_relayout(&mut self, generation: u64, viewport: Vec2) -> bool {
        let changed = self.generation != generation
            || (self.viewport.x - viewport.x).abs() > 0.5
            || (self.viewport.y - viewport.y).abs() > 0.5
            || self.cells.is_empty();
        if changed {
            self.generation = generation;
            self.viewport = viewport;
        }
        changed
    }
}

/// Treemap 视图
pub struct TreemapView;

impl TreemapView {
    /// 在给定 UI 中绘制 Treemap
    ///
    /// `cache` 跨帧保留；`generation` 与数据代次一致方可命中缓存，避免基于旧树渲染。
    pub fn show(ui: &mut Ui, root: &FileNode, theme: &ThemeColors, cache: &mut TreemapCache, generation: u64) {
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

        let viewport = rect.size();
        let node_count = count_nodes(root);

        // 命中判定：代次或视口尺寸变化（阈值 0.5px 抗抖动）或尚无缓存才重排
        if cache.needs_relayout(generation, viewport) {
            cache.cells.clear();
            if node_count > MAX_NODES {
                squarify_with_limit(&mut cache.cells, rect, root, 0);
            } else {
                squarify(&mut cache.cells, rect, root, 0);
            }
        }

        // ── 渲染缓存单元格（纯遍历，无递归/排序/分配）──
        let mut painter = ui.painter().clone();
        let border = Color32::from_black_alpha(100);
        for cell in &cache.cells {
            painter.rect_filled(cell.rect, 0.0, cell.color);
            painter.rect_stroke(cell.rect, 0.0, egui::Stroke::new(1.0_f32, border));
            if cell.show_label {
                let label = format!("{}\n{}", cell.name, cell.size_text);
                painter.text(
                    cell.rect.min + Vec2::new(3.0, 2.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional(cell.font_size),
                    cell.text_color,
                );
            }
        }

        // 图例移到右下角，半透明背景，避免遮挡主要数据
        let legend_size = Vec2::new(140.0, 180.0);
        let legend_rect = Rect::from_min_size(
            Pos2::new(rect.max.x - legend_size.x - 8.0, rect.max.y - legend_size.y - 8.0),
            legend_size,
        );
        draw_legend(&mut painter, legend_rect, theme);

        // 节点过多提示
        if node_count > MAX_NODES {
            let warning = format!("节点过多（{}），已优化渲染", node_count);
            painter.text(
                rect.min + Vec2::new(8.0, rect.height() - 24.0),
                egui::Align2::LEFT_BOTTOM,
                warning,
                egui::FontId::proportional(12.0),
                theme.warn,
            );
        }

        // ── hover 命中检测：基于缓存单元格线性查找，不再重跑布局算法 ──
        if let Some(pos) = response.hover_pos() {
            if let Some(cell) = cache.cells.iter().find(|c| c.rect.contains(pos)) {
                let tooltip = format!("{}\n{}", cell.name, cell.size_text);
                response.on_hover_text(tooltip);
            }
        }
    }
}

/// Squarified Treemap 核心算法：在 `rect` 内布局 `node` 的子项，结果追加到 `cells`
fn squarify(cells: &mut Vec<TreemapCell>, rect: Rect, node: &FileNode, depth: usize) {
    if node.size.0 == 0 || rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }

    // 叶节点或达到最大深度：绘制单元格
    if node.is_file() || node.children.is_empty() || depth >= 6 {
        draw_cell(cells, rect, node, depth);
        return;
    }

    let children: Vec<&FileNode> = node.children.iter().filter(|c| c.size.0 > 0).collect();

    if children.is_empty() {
        draw_cell(cells, rect, node, depth);
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
    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));

    layout_items(cells, rect, &items, depth);
}

/// 在矩形内依次布局 items，按 squarified 算法分行
fn layout_items(cells: &mut Vec<TreemapCell>, mut rect: Rect, items: &[(f64, &FileNode)], depth: usize) {
    let mut idx = 0;
    while idx < items.len() && rect.width() > 1.0 && rect.height() > 1.0 {
        let short_side = rect.width().min(rect.height()) as f64;
        let mut row: Vec<(f64, &FileNode)> = vec![items[idx]];
        let mut sum: f64 = items[idx].0;
        let mut best = worst_ratio(&row, short_side, sum);
        let mut consumed = 1;

        for item in items.iter().skip(idx + 1) {
            let mut trial = row.clone();
            trial.push(*item);
            let trial_sum = sum + item.0;
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
        draw_row(cells, row_rect, &row, sum, depth);
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

/// 在行矩形内按比例切分各 cell 并递归布局
fn draw_row(cells: &mut Vec<TreemapCell>, row_rect: Rect, row: &[(f64, &FileNode)], sum: f64, depth: usize) {
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
        squarify(cells, cell, child, depth + 1);
        offset += length;
    }
}

/// 绘制单元格：取一次名字/大小/颜色写入缓存
fn draw_cell(cells: &mut Vec<TreemapCell>, rect: Rect, node: &FileNode, depth: usize) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }

    let [r, g, b] = if node.is_dir() {
        darken(FileCategory::Other.base_color(), depth)
    } else {
        node.category.base_color()
    };

    let color = Color32::from_rgb(r, g, b);

    // 文字标签（仅当区域足够大）
    let show_label = rect.width() > 50.0 && rect.height() > 18.0;
    let text_color = if (r as u16 + g as u16 + b as u16) > 384 {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    let font_size = rect.height().clamp(9.0_f32, 16.0_f32);

    cells.push(TreemapCell {
        rect,
        name: node.name.clone(),
        size_text: node.size.to_string(),
        color,
        text_color,
        show_label,
        font_size,
    });
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
    painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, theme.border_light));

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

/// 统计节点总数
///
/// 利用 `FileNode::aggregate` 预填充的 `file_count`/`dir_count` 缓存，
/// 避免每帧递归遍历整棵树。
fn count_nodes(node: &FileNode) -> usize { 1usize + node.file_count as usize + node.dir_count as usize }

/// 带节点数限制的 Squarified Treemap 算法
///
/// 当节点总数超过 `MAX_NODES` 时，提前到 depth >= 4 截断，只渲染最大的子节点，
/// 避免单次绘制单元格过多导致卡顿。
fn squarify_with_limit(cells: &mut Vec<TreemapCell>, rect: Rect, node: &FileNode, depth: usize) {
    if node.size.0 == 0 || rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }

    if node.is_file() || node.children.is_empty() || depth >= 4 {
        draw_cell(cells, rect, node, depth);
        return;
    }

    let children: Vec<&FileNode> = node.children.iter().filter(|c| c.size.0 > 0).collect();

    if children.is_empty() {
        draw_cell(cells, rect, node, depth);
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
    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));

    layout_items(cells, rect, &items, depth);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::file_node::FileNode;

    /// 构造样例树：根目录含两个文件与一个子目录，回溯累计大小与计数。
    fn sample_tree() -> FileNode {
        let mut root = FileNode::new_dir(PathBuf::from("/test"), None);
        root.children
            .push(FileNode::new_file(PathBuf::from("/test/a.txt"), 100, None));
        root.children
            .push(FileNode::new_file(PathBuf::from("/test/b.mp4"), 300, None));
        let mut sub = FileNode::new_dir(PathBuf::from("/test/sub"), None);
        sub.children
            .push(FileNode::new_file(PathBuf::from("/test/sub/c.bin"), 200, None));
        root.children.push(sub);
        root.aggregate();
        root
    }

    /// 用真实布局结果填充缓存，避免手写 TreemapCell 造成测试与实现脱节。
    fn filled_cache(generation: u64, viewport: Vec2) -> TreemapCache {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), viewport);
        let mut cache = TreemapCache::default();
        assert!(cache.needs_relayout(generation, viewport), "首次应触发重排");
        squarify(&mut cache.cells, rect, &sample_tree(), 0);
        assert!(!cache.cells.is_empty(), "样例树应产生布局单元格");
        cache
    }

    #[test]
    fn first_layout_sets_cache_keys() {
        let mut cache = TreemapCache::default();
        let viewport = Vec2::new(800.0, 600.0);
        assert!(cache.needs_relayout(7, viewport), "空缓存必须重排");
        assert_eq!(cache.generation, 7);
        assert_eq!(cache.viewport, viewport);
    }

    #[test]
    fn same_generation_and_viewport_hits_cache() {
        let viewport = Vec2::new(800.0, 600.0);
        let mut cache = filled_cache(3, viewport);
        assert!(!cache.needs_relayout(3, viewport), "键未变化不应重排");
    }

    #[test]
    fn generation_change_invalidates_cache() {
        let viewport = Vec2::new(800.0, 600.0);
        let mut cache = filled_cache(3, viewport);
        assert!(cache.needs_relayout(4, viewport), "新代次必须重排");
        assert_eq!(cache.generation, 4);
        assert!(!cache.needs_relayout(4, viewport), "重排后同代次应命中缓存");
    }

    #[test]
    fn viewport_change_beyond_threshold_invalidates_on_both_axes() {
        let viewport = Vec2::new(800.0, 600.0);
        let mut cache = filled_cache(1, viewport);
        assert!(cache.needs_relayout(1, Vec2::new(801.0, 600.0)), "宽度变化应重排");
        assert_eq!(cache.viewport.x, 801.0);

        assert!(cache.needs_relayout(1, Vec2::new(801.0, 602.0)), "高度变化应重排");
        assert_eq!(cache.viewport.y, 602.0);
    }

    #[test]
    fn sub_pixel_jitter_is_ignored_and_does_not_move_baseline() {
        let viewport = Vec2::new(800.0, 600.0);
        let mut cache = filled_cache(1, viewport);
        assert!(!cache.needs_relayout(1, Vec2::new(800.4, 600.4)), "0.4px 抖动应忽略");
        assert_eq!(cache.viewport, viewport, "未失效时基准保持不变");

        // 基准未被逐帧推进，因此累积超过阈值后仍会正确触发一次重排
        assert!(cache.needs_relayout(1, Vec2::new(800.8, 600.8)), "累积抖动应触发重排");
        assert_eq!(cache.viewport, Vec2::new(800.8, 600.8));
    }

    #[test]
    fn emptied_cells_force_relayout() {
        let viewport = Vec2::new(800.0, 600.0);
        let mut cache = filled_cache(2, viewport);
        cache.cells.clear();
        assert!(cache.needs_relayout(2, viewport), "缓存被清空应重排");
    }

    #[test]
    fn layout_cells_stay_inside_viewport() {
        let viewport = Vec2::new(800.0, 600.0);
        let cache = filled_cache(1, viewport);
        for cell in &cache.cells {
            assert!(
                cell.rect.min.x >= -0.01 && cell.rect.min.y >= -0.01,
                "单元格不应越出左上边界"
            );
            assert!(
                cell.rect.max.x <= viewport.x + 0.01 && cell.rect.max.y <= viewport.y + 0.01,
                "单元格不应越出右下边界"
            );
        }
    }

    #[test]
    fn hover_hit_test_reuses_cached_cells() {
        let viewport = Vec2::new(800.0, 600.0);
        let cache = filled_cache(1, viewport);
        // show() 中的命中检测就是在这份缓存上做 contains 查找
        let target = &cache.cells[0];
        let pos = target.rect.center();
        let hit = cache.cells.iter().find(|c| c.rect.contains(pos));
        assert!(hit.is_some(), "布局单元格中心应能被命中检测找到");
        assert_eq!(hit.unwrap().name, target.name, "命中结果应与缓存单元格一致");
    }
}
