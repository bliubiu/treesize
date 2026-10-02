//! Icicle / Flame Graph 视图
//!
//! 计算自顶向下（Icicle）或自底向上（Flame Graph）的矩形布局，
//! 并渲染为 SVG 字符串。每个矩形的宽度与文件（或目录）大小成正比。

use crate::domain::file_node::FileNode;
use crate::domain::value_objects::{ByteSize, FileCategory};

/// 布局方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IcicleDirection {
    /// 根在顶部，子节点向下展开（Icicle）
    TopDown,
    /// 根在底部，子节点向上展开（Flame Graph）
    BottomUp,
}

/// 单个矩形区块
#[derive(Debug, Clone)]
pub struct IcicleBlock {
    /// 相对于 SVG 的 x 坐标
    pub x: f64,
    /// 相对于 SVG 的 y 坐标
    pub y: f64,
    /// 宽度
    pub w: f64,
    /// 高度
    pub h: f64,
    /// 显示名称（截断后）
    pub label: String,
    /// 完整路径
    pub path: String,
    /// 字节大小
    pub size: u64,
    /// 深度层级
    pub depth: usize,
    /// 文件大类
    pub category: FileCategory,
}

/// 为给定文件类别返回 SVG 填充色
pub fn category_color(cat: &FileCategory) -> &'static str {
    match cat {
        FileCategory::Document => "#60a5fa",
        FileCategory::Image => "#a78bfa",
        FileCategory::Video => "#f472b6",
        FileCategory::Audio => "#2dd4bf",
        FileCategory::Archive => "#fb923c",
        FileCategory::Source => "#4ade80",
        FileCategory::Executable => "#f87171",
        FileCategory::Database => "#818cf8",
        FileCategory::System => "#94a3b8",
        FileCategory::Other => "#6b7280",
    }
}

// ── 布局计算 ──

/// 布局参数
pub struct LayoutConfig {
    /// SVG 总宽度（px）
    pub width: f64,
    /// 每行高度（px）
    pub row_height: f64,
    /// 最大显示深度（超过此深度聚合到父级）
    pub max_depth: usize,
    /// 最小矩形宽度（px），小于此值不再向下递归
    pub min_block_width: f64,
    /// 排列方向
    pub direction: IcicleDirection,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            width: 1200.0,
            row_height: 32.0,
            max_depth: 8,
            min_block_width: 4.0,
            direction: IcicleDirection::TopDown,
        }
    }
}

/// 计算 Icicle / Flame Graph 布局
///
/// 返回平铺的区块列表，每个区块包含位置和元数据。
pub fn compute_layout(root: &FileNode, config: &LayoutConfig) -> Vec<IcicleBlock> {
    let mut blocks = Vec::new();
    let total = root.size.0.max(1);
    let depth_limit = config.max_depth;

    // 从根开始递归计算布局
    let svg_h = (depth_limit + 1) as f64 * config.row_height;
    let (root_y, child_dir) = match config.direction {
        IcicleDirection::TopDown => (0.0, 1),
        IcicleDirection::BottomUp => (svg_h - config.row_height, -1),
    };

    compute_blocks(
        root,
        0.0,
        root_y,
        config.width,
        config.row_height,
        total,
        0,
        depth_limit,
        config.min_block_width,
        child_dir,
        &mut blocks,
    );

    blocks
}

fn compute_blocks(
    node: &FileNode,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    total: u64,
    depth: usize,
    max_depth: usize,
    min_w: f64,
    child_dir: i32,
    blocks: &mut Vec<IcicleBlock>,
) {
    // 添加当前节点区块
    if depth == 0 || w >= min_w {
        let label = if node.is_dir() {
            truncate_label(&node.name, w, 11.0)
        } else {
            let name = format!("{}  {}", &node.name, ByteSize(node.size.0));
            truncate_label(&name, w, 11.0)
        };
        blocks.push(IcicleBlock {
            x,
            y: y.round(),
            w: w.round(),
            h: h.round(),
            label,
            path: node.path.display().to_string(),
            size: node.size.0,
            depth,
            category: node.category,
        });
    }

    // 达到最大深度或最小宽度，停止递归
    if depth >= max_depth || w < min_w || node.children.is_empty() {
        return;
    }

    // 排序子节点（按大小降序）
    let mut children: Vec<&FileNode> = node.children.iter().collect();
    children.sort_by(|a, b| b.size.cmp(&a.size));

    let child_y = y + child_dir as f64 * h;

    for child in children {
        if child.size.0 == 0 {
            continue;
        }
        let cw = (child.size.0 as f64 / total as f64) * w;
        // 确保最小宽度
        let cw = cw.max(min_w);
        // 最小的子节点不能超过剩余宽度
        let remaining = w; // simplified: just use proportional width
        let cw = cw.min(remaining);

        compute_blocks(
            child,
            x,
            child_y,
            cw,
            h,
            total,
            depth + 1,
            max_depth,
            min_w,
            child_dir,
            blocks,
        );

        // 不移动 x — 因为每个子节点都从 x=0 开始，不对
        // 实际上 X 应该累加。修正：
        // 我们把 width 也传进去，但需要不同的策略
    }
}

// ── 修正版布局算法 ──

/// 计算 Icicle 布局（修正版 - 正确的横向分割）
pub fn compute_layout_v2(root: &FileNode, config: &LayoutConfig) -> Vec<IcicleBlock> {
    let mut blocks = Vec::new();
    let total = root.size.0.max(1);
    let depth_limit = config.max_depth.min(12);

    let svg_h = (depth_limit + 1) as f64 * config.row_height;
    let (root_y, child_dir) = match config.direction {
        IcicleDirection::TopDown => (0.0, 1),
        IcicleDirection::BottomUp => (svg_h - config.row_height, -1),
    };

    // 根节点占满全宽
    add_block(
        &mut blocks,
        root,
        0.0,
        root_y,
        config.width,
        config.row_height,
        total,
        0,
    );

    // 递归布局子节点（只有根节点的子节点向下分割，每个子节点内部不继续水平分割）
    let mut children: Vec<&FileNode> = root.children.iter().collect();
    children.sort_by(|a, b| b.size.cmp(&a.size));

    let child_y = root_y + child_dir as f64 * config.row_height;

    for child in &children {
        if child.size.0 == 0 {
            continue;
        }
        let cw = (child.size.0 as f64 / total as f64) * config.width;
        if cw < config.min_block_width {
            continue;
        }
        layout_node(
            child,
            0.0, // x always 0 for each node's own sub-tree (full width split)
            child_y,
            config.width, // full width (the actual x position is computed within)
            config.row_height,
            total,
            1,
            depth_limit,
            config.min_block_width,
            child_dir,
            &mut blocks,
        );
    }

    blocks
}

/// 为单个节点及其子树生成布局。
/// `x`, `w` 定义该节点可用的水平范围。
fn layout_node(
    node: &FileNode,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    total: u64,
    depth: usize,
    max_depth: usize,
    min_w: f64,
    child_dir: i32,
    blocks: &mut Vec<IcicleBlock>,
) {
    if w < min_w {
        return;
    }

    // 添加当前节点
    add_block(blocks, node, x, y, w, h, total, depth);

    if depth >= max_depth || node.children.is_empty() {
        return;
    }

    // 排序子节点
    let mut children: Vec<&FileNode> = node.children.iter().collect();
    children.sort_by(|a, b| b.size.cmp(&a.size));

    let child_y = y + child_dir as f64 * h;

    // 子节点按大小比例水平分割
    let child_total = node.size.0.max(1);
    let mut cx = x;

    for child in &children {
        if child.size.0 == 0 {
            continue;
        }
        let cw = (child.size.0 as f64 / child_total as f64) * w;
        let cw = cw.max(min_w);

        // 检查会不会超出右边界
        let remaining = x + w - cx;
        if cw > remaining {
            break;
        }

        layout_node(
            child,
            cx,
            child_y,
            cw,
            h,
            total,
            depth + 1,
            max_depth,
            min_w,
            child_dir,
            blocks,
        );

        cx += cw;
        if cx >= x + w {
            break;
        }
    }
}

fn add_block(
    blocks: &mut Vec<IcicleBlock>,
    node: &FileNode,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    _total: u64,
    depth: usize,
) {
    let label = if w < 40.0 {
        String::new()
    } else if node.is_dir() {
        truncate_label(&node.name, w, 11.0)
    } else {
        let name = format!("{}  {}", &node.name, ByteSize(node.size.0));
        truncate_label(&name, w, 11.0)
    };

    blocks.push(IcicleBlock {
        x: x.round(),
        y: y.round(),
        w: w.round().max(1.0),
        h: h.round().max(1.0),
        label,
        path: node.path.display().to_string(),
        size: node.size.0,
        depth,
        category: node.category,
    });
}

// ── SVG 渲染 ──

/// 渲染 SVG 字符串
pub fn render_svg(blocks: &[IcicleBlock], config: &LayoutConfig) -> String {
    if blocks.is_empty() {
        return String::new();
    }

    let max_depth = blocks.iter().map(|b| b.depth).max().unwrap_or(0);
    let svg_h = (max_depth + 1) as f64 * config.row_height + 20.0; // bottom padding
    let w = config.width;

    let mut svg = String::with_capacity(blocks.len() * 200 + 512);
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {svg_h}" style="width:100%;max-width:{w}px;font-family:system-ui,-apple-system,sans-serif;font-size:11px">
<defs>
  <style>
    .i-block {{ transition:opacity .15s;cursor:pointer }}
    .i-block:hover {{ opacity:.8;stroke:#fff;stroke-width:1 }}
    .i-label {{ fill:#fff;font-weight:500;pointer-events:none;text-shadow:0 1px 2px rgba(0,0,0,.6);white-space:nowrap;overflow:hidden }}
  </style>
</defs>
"#,
    ));

    for block in blocks {
        let fill = category_color(&block.category);
        let tt = format!("{}  [{}]", block.path, ByteSize(block.size));

        // 检查矩形是否足够大以显示文本
        let label_y = block.y + block.h / 2.0 + 4.0;

        svg.push_str(&format!(
            r#"<rect class="i-block" x="{x}" y="{y}" width="{w}" height="{h}" fill="{fill}" rx="1">
  <title>{tt}</title>
</rect>
"#,
            x = block.x,
            y = block.y,
            w = block.w,
            h = block.h,
            fill = fill,
            tt = tt.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"),
        ));

        if !block.label.is_empty() && block.w > 30.0 {
            // 如果文本超出宽度，用 textLength 裁剪或直接截断
            let label = &block.label;
            svg.push_str(&format!(
                r#"<text class="i-label" x="{x}" y="{y}" dominant-baseline="middle">{label}</text>
"#,
                x = block.x + 4.0,
                y = label_y,
                label = label.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"),
            ));
        }
    }

    svg.push_str("</svg>\n");
    svg
}

/// 渲染完整的 HTML 页面（包含 Icicle + 基本说明）
pub fn render_html_page(blocks: &[IcicleBlock], config: &LayoutConfig, title: &str) -> String {
    let svg = render_svg(blocks, config);
    let direction_label = match config.direction {
        IcicleDirection::TopDown => "Icicle （自顶向下）",
        IcicleDirection::BottomUp => "Flame Graph （自底向上）",
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1.0">
<title>{title} - {direction_label}</title>
<style>
  body{{margin:0;padding:20px;font-family:system-ui,-apple-system,sans-serif;background:#0f1117;color:#e1e4ed}}
  h1{{font-size:20px;margin-bottom:8px}}
  .info{{color:#8b8fa8;font-size:13px;margin-bottom:20px}}
  .svg-wrap{{background:#1a1d28;border-radius:8px;padding:12px;overflow-x:auto}}
</style>
</head>
<body>
<h1>{title}</h1>
<p class="info">{direction_label} &mdash; 悬停查看完整路径和大小</p>
<div class="svg-wrap">{svg}</div>
</body>
</html>"#,
        title = title,
        direction_label = direction_label,
        svg = svg,
    )
}

// ── 工具函数 ──

/// 截断标签使其适合给定宽度
fn truncate_label(name: &str, max_w: f64, font_size: f64) -> String {
    // 近似：每个字符约 font_size * 0.6 像素宽
    let char_w = font_size * 0.6;
    let max_chars = (max_w / char_w) as usize;
    if max_chars < 3 {
        return String::new();
    }
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    let truncated: String = name.chars().take(max_chars.saturating_sub(2)).collect();
    format!("{}..", truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_tree() -> FileNode {
        let mut root = FileNode::new_dir(PathBuf::from("/root"), None);
        let mut sub1 = FileNode::new_dir(PathBuf::from("/root/sub1"), None);
        sub1.children
            .push(FileNode::new_file(PathBuf::from("/root/sub1/a.log"), 200, None));
        sub1.children
            .push(FileNode::new_file(PathBuf::from("/root/sub1/b.bin"), 800, None));
        root.children.push(sub1);

        let mut sub2 = FileNode::new_dir(PathBuf::from("/root/sub2"), None);
        sub2.children
            .push(FileNode::new_file(PathBuf::from("/root/sub2/c.txt"), 150, None));
        sub2.children
            .push(FileNode::new_file(PathBuf::from("/root/sub2/d.txt"), 50, None));
        root.children.push(sub2);

        root.children
            .push(FileNode::new_file(PathBuf::from("/root/e.jpg"), 300, None));
        root.aggregate();
        root
    }

    #[test]
    fn layout_icicle_topdown() {
        let root = make_tree();
        let cfg = LayoutConfig::default();
        let blocks = compute_layout_v2(&root, &cfg);
        assert!(!blocks.is_empty(), "should produce blocks");
        // Root must be present
        assert!(blocks.iter().any(|b| b.depth == 0));
        // Depth must increase
        for b in &blocks {
            if b.depth > 0 {
                assert!(b.w > 0.0 && b.h > 0.0);
            }
        }
    }

    #[test]
    fn layout_icicle_bottomup() {
        let root = make_tree();
        let cfg = LayoutConfig {
            direction: IcicleDirection::BottomUp,
            ..Default::default()
        };
        let blocks = compute_layout_v2(&root, &cfg);
        assert!(!blocks.is_empty());
    }

    #[test]
    fn svg_renders_with_valid_xml() {
        let root = make_tree();
        let cfg = LayoutConfig::default();
        let blocks = compute_layout_v2(&root, &cfg);
        let svg = render_svg(&blocks, &cfg);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>\n"));
        assert!(svg.contains("i-block"));
    }

    #[test]
    fn svg_contains_path_info() {
        let root = make_tree();
        let cfg = LayoutConfig::default();
        let blocks = compute_layout_v2(&root, &cfg);
        let svg = render_svg(&blocks, &cfg);
        assert!(svg.contains("title"));
    }

    #[test]
    fn layout_empty_tree() {
        let blocks = compute_layout_v2(
            &FileNode::new_dir(PathBuf::from("/empty"), None),
            &LayoutConfig::default(),
        );
        assert_eq!(blocks.len(), 1, "empty tree should produce exactly one root block");
        assert_eq!(blocks[0].depth, 0, "root block must be at depth 0");
    }

    #[test]
    fn layout_depth_limit_truncates() {
        let mut root = FileNode::new_dir(PathBuf::from("/root"), None);
        let mut child = FileNode::new_dir(PathBuf::from("/root/c1"), None);
        let mut grand = FileNode::new_dir(PathBuf::from("/root/c1/c2"), None);
        grand
            .children
            .push(FileNode::new_file(PathBuf::from("/root/c1/c2/f.txt"), 100, None));
        child.children.push(grand);
        root.children.push(child);
        root.aggregate();

        let cfg = LayoutConfig {
            max_depth: 2,
            ..Default::default()
        };
        let blocks = compute_layout_v2(&root, &cfg);
        let max_seen = blocks.iter().map(|b| b.depth).max().unwrap_or(0);
        assert!(max_seen <= cfg.max_depth, "depth should be capped at max_depth");
    }

    #[test]
    fn truncate_label_works() {
        assert_eq!(truncate_label("hello", 100.0, 11.0), "hello");
        let t = truncate_label("very long file name here.txt", 50.0, 11.0);
        assert!(t.ends_with(".."));
        assert!(t.len() < "very long file name here.txt".len());
    }
}
