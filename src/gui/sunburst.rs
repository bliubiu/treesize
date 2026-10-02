//! Sunburst 环形图可视化
//!
//! 使用同心环段表示目录层次结构。内层为根节点，向外逐层展开。
//! 每个环段的角度大小与节点大小成正比，环段颜色按文件类别区分。
//!
//! 优化策略：
//! 1. 最大环段数限制（MAX_SEGMENTS = 5000）
//! 2. 最小角度过滤，过小的环段跳过以避免过度细分
//! 3. 最大深度限制（MAX_DEPTH = 10）

use std::f32::consts::TAU;

use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Sense, Shape, Ui, Vec2};
use eframe::epaint;

use crate::domain::file_node::FileNode;
use crate::domain::value_objects::{ByteSize, FileCategory};

use super::theme::ThemeColors;

// ─── 常量 ──────────────────────────────────────────────────────────────────

/// 最大渲染环段数（防止性能问题）
const MAX_SEGMENTS: usize = 5000;

/// 最大展示深度（超出此深度的节点聚合并跳过细分）
const MAX_DEPTH: usize = 10;

/// 最小角度（弧度），过小的环段跳过
const MIN_ANGLE: f32 = 0.002; // ≈ 0.11°

/// 中心圆半径占最大半径的比例
const CENTER_RATIO: f32 = 0.10;

/// 环间间隙比例（占环厚的百分比）
const RING_GAP_RATIO: f32 = 0.03;

// ─── 环段信息（用于悬停检测） ─────────────────────────────────────────────
//
// 直接存储渲染和悬停所需数据，避免使用裸指针

struct SegmentInfo {
    inner_r: f32,
    outer_r: f32,
    start_angle: f32,
    end_angle: f32,
    depth: usize,
    /// 节点名称（用于悬停提示）
    name: String,
    /// 完整路径（用于悬停提示）
    path: String,
    /// 节点大小
    size: ByteSize,
    /// 是否为目录（影响颜色计算）
    is_dir: bool,
    /// 文件类别（影响颜色计算）
    category: FileCategory,
}

// ─── Sunburst 视图 ─────────────────────────────────────────────────────────

pub struct SunburstView;

impl SunburstView {
    /// 在给定 UI 中绘制 Sunburst 环形图
    pub fn show(ui: &mut Ui, root: &FileNode, theme: &ThemeColors) {
        let available = ui.available_size();
        let side = available.x.min(available.y);
        let (rect, response) = ui.allocate_exact_size(Vec2::new(side, side), Sense::click());

        if root.size.0 == 0 || side < 20.0 {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "无数据或区域过小",
                egui::FontId::proportional(14.0),
                theme.text_secondary,
            );
            return;
        }

        let center = rect.center();
        let max_radius = side * 0.46; // 留边距
        let center_r = (max_radius * CENTER_RATIO).max(4.0);
        let ring_thickness = (max_radius - center_r) / MAX_DEPTH as f32;

        // ── 收集环段信息 ──
        let mut segments: Vec<SegmentInfo> = Vec::new();
        collect_segments(
            &mut segments,
            root,
            center_r,
            ring_thickness,
            0.0,
            TAU,
            0,
            root.size.0 as f32,
        );

        // ── 绘制环段（Mesh） ──
        let mut painter = ui.painter().clone();
        let mut mesh = Mesh::default();

        for seg in &segments {
            let [r, g, b] = if seg.is_dir {
                darken(FileCategory::Other.base_color(), seg.depth)
            } else {
                seg.category.base_color()
            };
            add_ring_segment(
                &mut mesh,
                center,
                seg.inner_r,
                seg.outer_r,
                seg.start_angle,
                seg.end_angle,
                Color32::from_rgb(r, g, b),
            );
        }
        painter.add(Shape::Mesh(mesh));

        // ── 绘制中心圆（根节点）：使用主题强调色 ──
        painter.circle_filled(center, center_r, theme.accent_dim);
        painter.circle_stroke(center, center_r, egui::Stroke::new(1.0, theme.accent));

        // 中心标签（根目录名，最多显示 8 字）
        let root_label = if root.name.len() > 8 {
            format!("{}…", &root.name[..8])
        } else {
            root.name.clone()
        };
        if center_r > 14.0 {
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                root_label,
                egui::FontId::proportional((center_r * 0.6).min(14.0)),
                Color32::WHITE,
            );
        }

        // ── 图例 ──
        let legend_size = Vec2::new(140.0, 180.0);
        let legend_rect = Rect::from_min_size(
            Pos2::new(rect.max.x - legend_size.x - 8.0, rect.min.y + 8.0),
            legend_size,
        );
        draw_legend(&mut painter, legend_rect, theme);

        // ── 悬停提示 ──
        if let Some(pos) = response.hover_pos() {
            if let Some(seg) = find_segment(&segments, center, pos) {
                let tooltip = format!("{}\n大小：{}\n路径：{}", seg.name, seg.size, seg.path);
                response.on_hover_text(tooltip);
            }
        }
    }
}

// ─── 布局收集 ─────────────────────────────────────────────────────────────

/// 递归收集环段信息
fn collect_segments(
    segments: &mut Vec<SegmentInfo>,
    node: &FileNode,
    center_r: f32,
    ring_thickness: f32,
    start_angle: f32,
    angle_span: f32,
    depth: usize,
    total_size: f32,
) {
    if segments.len() >= MAX_SEGMENTS {
        return;
    }

    let inner_r = center_r + depth as f32 * ring_thickness;
    let gap = ring_thickness * RING_GAP_RATIO;
    let outer_r = inner_r + ring_thickness - gap;

    segments.push(SegmentInfo {
        inner_r,
        outer_r,
        start_angle,
        end_angle: start_angle + angle_span,
        depth,
        name: node.name.clone(),
        path: node.path.display().to_string(),
        size: node.size,
        is_dir: node.is_dir(),
        category: node.category,
    });

    // 叶节点或达到最大深度：不再细分
    if !node.is_dir() || node.children.is_empty() || depth >= MAX_DEPTH - 1 {
        return;
    }

    // 按大小降序排序子节点
    let mut children: Vec<&FileNode> = node.children.iter().filter(|c| c.size.0 > 0).collect();
    children.sort_by(|a, b| b.size.0.cmp(&a.size.0));

    // 如果所有子节点都很小且总大小不匹配，用 node.size
    let parent_total = total_size.max(1.0);

    let mut current_angle = start_angle;
    for child in children {
        let child_span = angle_span * (child.size.0 as f32 / parent_total);
        if child_span >= MIN_ANGLE {
            collect_segments(
                segments,
                child,
                center_r,
                ring_thickness,
                current_angle,
                child_span,
                depth + 1,
                child.size.0 as f32,
            );
            current_angle += child_span;
        }
        // 跳过过小的环段（角度 < MIN_ANGLE 的不显示，角度损耗可忽略）
    }
}

// ─── 网格渲染 ─────────────────────────────────────────────────────────────

/// 向 Mesh 中添加一个环形段
fn add_ring_segment(
    mesh: &mut Mesh,
    center: Pos2,
    inner_r: f32,
    outer_r: f32,
    start_angle: f32,
    end_angle: f32,
    color: Color32,
) {
    let span = end_angle - start_angle;
    let n = ((span * 60.0 / TAU).ceil() as u32).max(4).min(120);
    let step = span / n as f32;

    let base = mesh.vertices.len() as u32;

    for i in 0..=n {
        let a = start_angle + step * i as f32;
        let (s, c) = a.sin_cos();

        let inner_pt = Pos2::new(center.x + inner_r * c, center.y + inner_r * s);
        let outer_pt = Pos2::new(center.x + outer_r * c, center.y + outer_r * s);

        mesh.vertices.push(epaint::Vertex {
            pos: inner_pt,
            color,
            uv: epaint::WHITE_UV,
        });
        mesh.vertices.push(epaint::Vertex {
            pos: outer_pt,
            color,
            uv: epaint::WHITE_UV,
        });
    }

    // 三角化：每个四边形拆成两个三角形
    for i in 0..n {
        let b = base + i * 2;
        mesh.indices.extend_from_slice(&[b, b + 1, b + 2, b + 2, b + 1, b + 3]);
    }
}

// ─── 颜色工具 ─────────────────────────────────────────────────────────────

/// 按深度变暗颜色
fn darken([r, g, b]: [u8; 3], depth: usize) -> [u8; 3] {
    let factor = 0.85_f64.powi(depth as i32);
    [
        (r as f64 * factor) as u8,
        (g as f64 * factor) as u8,
        (b as f64 * factor) as u8,
    ]
}

// ─── 图例 ─────────────────────────────────────────────────────────────────

/// 绘制颜色分类图例
fn draw_legend(painter: &mut egui::Painter, rect: Rect, theme: &ThemeColors) {
    painter.rect_filled(rect, 4.0, Color32::from_rgba_premultiplied(15, 20, 25, 220));
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

// ─── 悬停命中检测 ─────────────────────────────────────────────────────────

/// 在环段列表中查找鼠标位置对应的环段
fn find_segment<'a>(segments: &'a [SegmentInfo], center: Pos2, pos: Pos2) -> Option<&'a SegmentInfo> {
    let rel = pos - center;
    let dist = rel.length();
    let angle = rel.y.atan2(rel.x);
    // 将角度归一化到 [0, TAU)
    let angle = if angle < 0.0 { angle + TAU } else { angle };

    for seg in segments {
        if dist >= seg.inner_r && dist <= seg.outer_r {
            // 处理角度范围跨越 0 的情况（例如 [350°, 10°] 跨越 0°）
            let in_range = if seg.start_angle <= seg.end_angle {
                angle >= seg.start_angle && angle < seg.end_angle
            } else {
                angle >= seg.start_angle || angle < seg.end_angle
            };
            if in_range {
                return Some(seg);
            }
        }
    }
    None
}
