//! 报表服务
//!
//! 将 `FileNode` 树转换为多种输出格式：树形文本、JSON、CSV、HTML、TopN 列表。

use std::io::Write;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::domain::error::{DomainError, Result};
use crate::domain::file_node::FileNode;

/// 将序列化/CSV 错误包装为领域层 IO 错误
/// 序列化属于基础设施关注点，不应对应领域层的专有错误变体
fn ser_err(e: impl std::fmt::Display) -> DomainError {
    DomainError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}
use crate::domain::scan_engine::ScanStats;
use crate::domain::value_objects::{ByteSize, FileCategory};

/// TopN 报表条目（可序列化）
#[derive(Debug, Serialize)]
pub struct TopNEntry {
    pub path: String,
    pub name: String,
    pub size: ByteSize,
    pub percent: f64,
    /// 文件大类
    pub category: FileCategory,
    /// 规范化扩展名（小写、无点），目录为空
    pub extension: String,
    /// 最后修改时间（格式化 YYYY-MM-DD HH:MM）
    pub modified: Option<String>,
}

/// 报表服务
pub struct ReportService;

impl ReportService {
    /// 渲染树形文本报表到 writer
    ///
    /// `max_depth` 为最大显示深度（0 = 仅根，usize::MAX = 全部）。
    /// `min_percent` 为显示阈值，占比小于此值的节点折叠（0.0 = 显示全部）。
    pub fn render_tree<W: Write>(root: &FileNode, writer: &mut W, max_depth: usize, min_percent: f64) -> Result<()> {
        Self::render_tree_inner(root, writer, 0, max_depth, min_percent, root.size)?;
        Ok(())
    }

    fn render_tree_inner<W: Write>(
        node: &FileNode,
        writer: &mut W,
        depth: usize,
        max_depth: usize,
        min_percent: f64,
        total: ByteSize,
    ) -> Result<()> {
        let indent = "  ".repeat(depth);
        let percent = if total.0 > 0 {
            (node.size.0 as f64 / total.0 as f64) * 100.0
        } else {
            0.0
        };

        // 折叠小节点
        if depth > 0 && min_percent > 0.0 && percent < min_percent {
            return Ok(());
        }

        let kind = if node.is_dir() { "\u{1f4c1}" } else { "\u{1f4c4}" };
        let line = format!(
            "{indent}{kind} {} [{} | {:.2}% | {} \u{4e2a}\u{6587}\u{4ef6}/{} \u{4e2a}\u{76ee}\u{5f55}]\n",
            node.name, node.size, percent, node.file_count, node.dir_count,
        );
        writer.write_all(line.as_bytes()).map_err(DomainError::Io)?;

        if depth < max_depth {
            for child in &node.children {
                Self::render_tree_inner(child, writer, depth + 1, max_depth, min_percent, total)?;
            }
        }
        Ok(())
    }

    /// 序列化为 JSON 写入文件
    pub fn write_json(root: &FileNode, path: &Path) -> Result<()> {
        let file = std::fs::File::create(path).map_err(DomainError::Io)?;
        let writer = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(writer, root).map_err(|e| ser_err(e))?;
        Ok(())
    }

    /// 序列化为 JSON 字符串
    pub fn to_json_string(root: &FileNode) -> Result<String> {
        serde_json::to_string_pretty(root).map_err(|e| ser_err(e))
    }

    /// 导出扁平化 CSV（每个文件/目录一行）
    pub fn write_csv(root: &FileNode, path: &Path) -> Result<()> {
        let file = std::fs::File::create(path).map_err(DomainError::Io)?;
        let mut writer = csv::Writer::from_writer(file);
        Self::write_csv_header(&mut writer)?;
        Self::write_csv_inner(root, &mut writer)?;
        writer.flush().map_err(DomainError::Io)?;
        Ok(())
    }

    /// 将 CSV 写入字节缓冲区（Vec<u8>），避免代码重复
    pub fn write_csv_to_buf(root: &FileNode) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        {
            let mut csv_writer = csv::Writer::from_writer(&mut buf);
            Self::write_csv_header(&mut csv_writer)?;
            Self::write_csv_inner(root, &mut csv_writer)?;
            csv_writer.flush().map_err(DomainError::Io)?;
        }
        Ok(buf)
    }

    fn write_csv_header<W: Write>(writer: &mut csv::Writer<W>) -> Result<()> {
        writer
            .write_record(&[
                "路径",
                "名称",
                "类型",
                "大小(字节)",
                "大小(可读)",
                "扩展名",
                "文件数",
                "目录数",
                "修改时间",
            ])
            .map_err(|e| ser_err(e))
    }

    fn write_csv_inner<W: Write>(node: &FileNode, writer: &mut csv::Writer<W>) -> Result<()> {
        let kind = if node.is_dir() { "目录" } else { "文件" };
        let modified = node
            .modified
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "无效时间戳".to_string())
            })
            .unwrap_or_else(|| "未知".to_string());

        writer
            .write_record(&[
                node.path.display().to_string(),
                node.name.clone(),
                kind.to_string(),
                node.size.0.to_string(),
                node.size.human_readable(),
                node.extension.clone(),
                node.file_count.to_string(),
                node.dir_count.to_string(),
                modified,
            ])
            .map_err(|e| ser_err(e))?;

        for child in &node.children {
            Self::write_csv_inner(child, writer)?;
        }
        Ok(())
    }

    /// 取 TopN 大文件（可序列化形式）
    pub fn top_n_files_report(root: &FileNode, n: usize) -> Vec<TopNEntry> {
        let total = root.size.0;
        let mut files: Vec<&FileNode> = root.iter_all().filter(|n| n.is_file()).collect();
        files.sort_by(|a, b| b.size.cmp(&a.size));
        files.truncate(n);
        files
            .into_iter()
            .map(|node| {
                let percent = if total > 0 {
                    (node.size.0 as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                TopNEntry {
                    path: node.path.display().to_string(),
                    name: node.name.clone(),
                    size: node.size,
                    percent,
                    category: node.category,
                    extension: node.extension.clone(),
                    modified: node.modified.map(|t| -> String {
                        let datetime: chrono::DateTime<chrono::Local> = t.into();
                        datetime.format("%Y-%m-%d %H:%M").to_string()
                    }),
                }
            })
            .collect()
    }

    /// 取 TopN 大目录（按累计大小，可序列化形式）
    pub fn top_n_dirs_report(root: &FileNode, n: usize) -> Vec<TopNEntry> {
        let total = root.size.0;
        let mut dirs: Vec<&FileNode> = root
            .iter_all()
            .filter(|n| n.is_dir() && !n.children.is_empty())
            .collect();
        dirs.sort_by(|a, b| b.size.cmp(&a.size));
        dirs.truncate(n);
        dirs.into_iter()
            .map(|node| {
                let percent = if total > 0 {
                    (node.size.0 as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                TopNEntry {
                    path: node.path.display().to_string(),
                    name: node.name.clone(),
                    size: node.size,
                    percent,
                    category: node.category,
                    extension: node.extension.clone(),
                    modified: node.modified.map(|t| -> String {
                        let datetime: chrono::DateTime<chrono::Local> = t.into();
                        datetime.format("%Y-%m-%d %H:%M").to_string()
                    }),
                }
            })
            .collect()
    }

    /// 生成自包含 HTML 交互式报表
    pub fn write_html<W: Write>(
        root: &FileNode,
        writer: &mut W,
        stats: &ScanStats,
        scan_path: &Path,
        top_n: usize,
        classify: Option<&crate::application::classify_service::ClassifyReport>,
    ) -> Result<()> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let path_str = scan_path.display().to_string();
        let total = root.size.0;

        let top_files = Self::top_n_files_report(root, top_n);
        let top_dirs = Self::top_n_dirs_report(root, top_n);

        let tree_val = serde_json::to_value(root).map_err(|e| ser_err(e))?;
        let files_val = serde_json::to_value(&top_files).map_err(|e| ser_err(e))?;
        let dirs_val = serde_json::to_value(&top_dirs).map_err(|e| ser_err(e))?;
        let classify_val = classify.and_then(|c| serde_json::to_value(c).ok());

        write_html_tags(writer)?;
        write_html_header(writer, &path_str, &now, stats, total)?;
        write_html_body(writer, top_n)?;
        write_html_js(writer, &tree_val, &files_val, &dirs_val, classify_val.as_ref())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HTML report: section writers — each write! call is either a literal
// string or has at most one format argument.  This avoids all conflict
// between Rust {} placeholders and JavaScript { } braces.
// ---------------------------------------------------------------------------

fn write_html_tags<W: Write>(w: &mut W) -> Result<()> {
    // Write CSS as a separate write_all call to avoid brace conflicts with write!()
    let doctype = b"<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"UTF-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1.0\">\n<title>treesize - ";
    w.write_all(doctype).map_err(DomainError::Io)?;
    write!(
        w,
        "\u{78c1}\u{76d8}\u{5360}\u{7528}\u{5206}\u{6790}\u{62a5}\u{544a}</title>\n<style>\n"
    )
    .map_err(DomainError::Io)?;
    // CSS as a raw byte slice — no write!() format processing means {} are literal
    w.write_all(b":root{--bg:#0f1117;--surface:#1a1d28;--surface-2:#242738;--border:#2e3148;--text:#e1e4ed;--text-secondary:#8b8fa8;--accent:#6c8cff;--orange:#fb923c;--green:#4ade80;--purple:#a78bfa;--teal:#2dd4bf;--pink:#f472b6;--red:#f87171;--font-mono:ui-monospace,'SF Mono','Cascadia Code','Fira Code',Consolas,monospace;--font-sans:-apple-system,BlinkMacSystemFont,'Segoe UI','Noto Sans SC',Roboto,sans-serif}").map_err(DomainError::Io)?;
    w.write_all(b"*{margin:0;padding:0;box-sizing:border-box}")
        .map_err(DomainError::Io)?;
    w.write_all(b"body{font-family:var(--font-sans);background:var(--bg);color:var(--text);line-height:1.6}")
        .map_err(DomainError::Io)?;
    w.write_all(b".container{max-width:1200px;margin:0 auto;padding:24px}")
        .map_err(DomainError::Io)?;
    w.write_all(b".header{background:linear-gradient(135deg,var(--surface),var(--surface-2));border:1px solid var(--border);border-radius:12px;padding:32px;margin-bottom:24px}").map_err(DomainError::Io)?;
    w.write_all(b".header h1{font-size:24px;font-weight:700;margin-bottom:8px;background:linear-gradient(90deg,var(--accent),var(--purple));-webkit-background-clip:text;-webkit-text-fill-color:transparent;background-clip:text}").map_err(DomainError::Io)?;
    w.write_all(b".header .meta{color:var(--text-secondary);font-size:14px}")
        .map_err(DomainError::Io)?;
    w.write_all(b".header .meta span{display:inline-block;margin-right:20px}")
        .map_err(DomainError::Io)?;
    w.write_all(
        b".stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:16px;margin-bottom:24px}",
    )
    .map_err(DomainError::Io)?;
    w.write_all(b".stat-card{background:var(--surface);border:1px solid var(--border);border-radius:10px;padding:20px;text-align:center}").map_err(DomainError::Io)?;
    w.write_all(b".stat-card .value{font-size:28px;font-weight:700;font-family:var(--font-mono);color:var(--accent)}")
        .map_err(DomainError::Io)?;
    w.write_all(b".stat-card .label{font-size:13px;color:var(--text-secondary);margin-top:4px}")
        .map_err(DomainError::Io)?;
    w.write_all(b".section{background:var(--surface);border:1px solid var(--border);border-radius:12px;padding:24px;margin-bottom:24px}").map_err(DomainError::Io)?;
    w.write_all(b".section h2{font-size:18px;font-weight:600;margin-bottom:16px;padding-bottom:8px;border-bottom:1px solid var(--border)}").map_err(DomainError::Io)?;
    w.write_all(b".table-wrap{overflow-x:auto}").map_err(DomainError::Io)?;
    w.write_all(b"table{width:100%;border-collapse:collapse;font-size:14px}")
        .map_err(DomainError::Io)?;
    w.write_all(b"th,td{padding:8px 12px;text-align:right;white-space:nowrap}")
        .map_err(DomainError::Io)?;
    w.write_all(b"th{color:var(--text-secondary);font-weight:600;font-size:12px;text-transform:uppercase;letter-spacing:.5px;border-bottom:1px solid var(--border);position:sticky;top:0;background:var(--surface);cursor:pointer;user-select:none}").map_err(DomainError::Io)?;
    w.write_all(b"th:first-child,td:first-child{text-align:left}")
        .map_err(DomainError::Io)?;
    w.write_all(b"td.name-cell{max-width:400px;overflow:hidden;text-overflow:ellipsis;direction:rtl;text-align:left}")
        .map_err(DomainError::Io)?;
    w.write_all(b"tr:hover td{background:var(--surface-2)}")
        .map_err(DomainError::Io)?;
    w.write_all(b".bar{display:inline-block;height:6px;border-radius:3px;background:var(--accent);min-width:2px;vertical-align:middle}").map_err(DomainError::Io)?;
    w.write_all(b".bar-wrap{width:80px;display:inline-block;background:var(--border);border-radius:3px;height:6px}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tree{font-family:var(--font-mono);font-size:13px;line-height:1.8}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tree ul{list-style:none;padding-left:20px}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tree li{position:relative}").map_err(DomainError::Io)?;
    w.write_all(b".tree .toggle{cursor:pointer;display:inline-block;width:16px;text-align:center;color:var(--text-secondary);user-select:none}").map_err(DomainError::Io)?;
    w.write_all(b".tree .toggle:hover{color:var(--accent)}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tree .node-label{cursor:pointer;display:inline-flex;align-items:center;gap:6px;padding:2px 4px;border-radius:4px}").map_err(DomainError::Io)?;
    w.write_all(b".tree .node-label:hover{background:var(--surface-2)}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tree .node-icon{width:16px;text-align:center}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tree .node-icon.folder{color:var(--orange)}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tree .node-icon.file{color:var(--accent)}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tree .node-size{color:var(--text-secondary);margin-left:8px}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tree .node-pct{color:var(--text-secondary);font-size:11px;margin-left:4px}")
        .map_err(DomainError::Io)?;
    w.write_all(
        b".tree .tree-bar{display:inline-block;height:4px;border-radius:2px;margin-left:6px;vertical-align:middle}",
    )
    .map_err(DomainError::Io)?;
    w.write_all(b".collapsed>ul{display:none}").map_err(DomainError::Io)?;
    w.write_all(b".cat-dot{display:inline-block;width:10px;height:10px;border-radius:50%;margin-right:6px;vertical-align:middle}").map_err(DomainError::Io)?;
    w.write_all(b".classify-grid{display:grid;grid-template-columns:1fr 1fr;gap:24px}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tabs{display:flex;gap:4px;margin-bottom:16px;flex-wrap:wrap}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tab{padding:8px 16px;border-radius:8px;cursor:pointer;font-size:14px;color:var(--text-secondary);border:1px solid transparent;background:transparent;transition:all .15s}").map_err(DomainError::Io)?;
    w.write_all(b".tab:hover{color:var(--text);background:var(--surface-2)}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tab.active{color:var(--accent);border-color:var(--accent);background:rgba(108,140,255,.1)}")
        .map_err(DomainError::Io)?;
    w.write_all(b".tab-content{display:none}").map_err(DomainError::Io)?;
    w.write_all(b".tab-content.active{display:block}")
        .map_err(DomainError::Io)?;
    w.write_all(b".footer{text-align:center;color:var(--text-secondary);font-size:12px;padding:24px}")
        .map_err(DomainError::Io)?;
    w.write_all(b"</style>\n</head>\n<body>\n<div class=\"container\">\n")
        .map_err(DomainError::Io)?;
    Ok(())
}

fn write_html_header<W: Write>(w: &mut W, path: &str, now: &str, stats: &ScanStats, total_size: u64) -> Result<()> {
    let hr = ByteSize(total_size).human_readable();
    let secs = stats.elapsed_ms as f64 / 1000.0;
    write!(w, "  <div class=\"header\">\n    <h1>").map_err(DomainError::Io)?;
    write!(w, "\u{1f4ca} \u{78c1}\u{76d8}\u{5360}\u{7528}\u{5206}\u{6790}\u{62a5}\u{544a}</h1>\n    <div class=\"meta\">\n      <span>\u{1f4c2} ").map_err(DomainError::Io)?;
    write_html_text(w, path)?;
    write!(w, "</span>\n      <span>\u{1f550} ").map_err(DomainError::Io)?;
    write_html_text(w, now)?;
    write!(w, "</span>\n      <span>\u{23f1} ").map_err(DomainError::Io)?;
    write!(w, "{:.1}", secs).map_err(DomainError::Io)?;
    write!(w, " \u{79d2}</span>\n    </div>\n  </div>\n").map_err(DomainError::Io)?;

    write!(
        w,
        "  <div class=\"stats\">\n    <div class=\"stat-card\"><div class=\"value\">"
    )
    .map_err(DomainError::Io)?;
    write_html_text(w, &hr)?;
    write!(w, "</div><div class=\"label\">\u{603b}\u{5927}\u{5c0f}</div></div>\n    <div class=\"stat-card\"><div class=\"value\">").map_err(DomainError::Io)?;
    write!(w, "{}", stats.total_files).map_err(DomainError::Io)?;
    write!(w, "</div><div class=\"label\">\u{6587}\u{4ef6}\u{6570}</div></div>\n    <div class=\"stat-card\"><div class=\"value\">").map_err(DomainError::Io)?;
    write!(w, "{}", stats.total_dirs).map_err(DomainError::Io)?;
    write!(w, "</div><div class=\"label\">\u{76ee}\u{5f55}\u{6570}</div></div>\n    <div class=\"stat-card\"><div class=\"value\" style=\"color:var(--orange)\">").map_err(DomainError::Io)?;
    write!(w, "{}", stats.error_count).map_err(DomainError::Io)?;
    write!(
        w,
        "</div><div class=\"label\">\u{626b}\u{63cf}\u{9519}\u{8bef}</div></div>\n  </div>\n"
    )
    .map_err(DomainError::Io)?;
    Ok(())
}

fn write_html_body<W: Write>(w: &mut W, top_n: usize) -> Result<()> {
    w.write_all(b"  <div class=\"tabs\">\n    <div class=\"tab active\" data-tab=\"tree\">\xf0\x9f\x8c\xb3 \xe7\x9b\xae\xe5\xbd\x95\xe6\xa0\x91</div>\n    <div class=\"tab\" data-tab=\"top-files\">\xf0\x9f\x93\x84 \xe5\xa4\xa7\xe6\x96\x87\xe4\xbb\xb6</div>\n    <div class=\"tab\" data-tab=\"top-dirs\">\xf0\x9f\x93\x81 \xe5\xa4\xa7\xe7\x9b\xae\xe5\xbd\x95</div>\n    <div class=\"tab\" data-tab=\"classify\" id=\"classify-tab\" style=\"display:none\">\xf0\x9f\x93\x8a \xe5\x88\x86\xe7\xb1\xbb\xe7\xbb\x9f\xe8\xae\xa1</div>\n  </div>\n").map_err(DomainError::Io)?;

    w.write_all(b"  <div class=\"section tab-content active\" id=\"tab-tree\">\n    <h2>\xf0\x9f\x8c\xb3 \xe7\x9b\xae\xe5\xbd\x95\xe6\xa0\x91</h2>\n    <div class=\"tree\" id=\"tree-root\"></div>\n  </div>\n").map_err(DomainError::Io)?;

    w.write_all(b"  <div class=\"section tab-content\" id=\"tab-top-files\">\n    <h2>\xf0\x9f\x93\x84 Top ")
        .map_err(DomainError::Io)?;
    write!(w, "{}", top_n).map_err(DomainError::Io)?;
    w.write_all(b" \xe5\xa4\xa7\xe6\x96\x87\xe4\xbb\xb6</h2>\n    <div class=\"table-wrap\">\n      <table><thead><tr>\n        <th data-sort=\"name\">\xe6\x96\x87\xe4\xbb\xb6\xe5\x90\x8d</th>\n        <th data-sort=\"size\" data-dir=\"desc\">\xe5\xa4\xa7\xe5\xb0\x8f</th>\n        <th data-sort=\"percent\" data-dir=\"desc\">\xe5\x8d\xa0\xe6\xaf\x94</th>\n        <th data-sort=\"category\">\xe7\xb1\xbb\xe5\x88\xab</th>\n        <th data-sort=\"extension\">\xe6\x89\xa9\xe5\xb1\x95\xe5\x90\x8d</th>\n        <th data-sort=\"modified\">\xe4\xbf\xae\xe6\x94\xb9\xe6\x97\xb6\xe9\x97\xb4</th>\n      </tr></thead>\n      <tbody id=\"top-files-body\"></tbody></table>\n    </div>\n  </div>\n").map_err(DomainError::Io)?;

    w.write_all(b"  <div class=\"section tab-content\" id=\"tab-top-dirs\">\n    <h2>\xf0\x9f\x93\x81 Top ")
        .map_err(DomainError::Io)?;
    write!(w, "{}", top_n).map_err(DomainError::Io)?;
    w.write_all(b" \xe5\xa4\xa7\xe7\x9b\xae\xe5\xbd\x95</h2>\n    <div class=\"table-wrap\">\n      <table><thead><tr>\n        <th data-sort=\"name\">\xe7\x9b\xae\xe5\xbd\x95\xe5\x90\x8d</th>\n        <th data-sort=\"size\" data-dir=\"desc\">\xe5\xa4\xa7\xe5\xb0\x8f</th>\n        <th data-sort=\"percent\" data-dir=\"desc\">\xe5\x8d\xa0\xe6\xaf\x94</th>\n      </tr></thead>\n      <tbody id=\"top-dirs-body\"></tbody></table>\n    </div>\n  </div>\n").map_err(DomainError::Io)?;

    w.write_all(b"  <div class=\"section tab-content\" id=\"tab-classify\">\n    <h2>\xf0\x9f\x93\x8a \xe6\x96\x87\xe4\xbb\xb6\xe5\x88\x86\xe7\xb1\xbb\xe7\xbb\x9f\xe8\xae\xa1</h2>\n    <div id=\"classify-root\"></div>\n  </div>\n").map_err(DomainError::Io)?;

    w.write_all(b"  <div class=\"footer\">Generated by <strong>treesize</strong> \xe2\x80\x94 \xe7\xa3\x81\xe7\x9b\x98\xe5\x8d\xa0\xe7\x94\xa8\xe5\x88\x86\xe6\x9e\x90\xe5\xb7\xa5\xe5\x85\xb7</div>\n</div>\n").map_err(DomainError::Io)?;
    Ok(())
}

/// Escape HTML entities in a plain-text value and write it.
fn write_html_text<W: Write>(w: &mut W, s: &str) -> Result<()> {
    // Only escape &, <, >, ", ' – sufficient for attribute and text content
    let mut start = 0;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let rep = match b {
            b'&' => Some("&amp;"),
            b'<' => Some("&lt;"),
            b'>' => Some("&gt;"),
            b'"' => Some("&quot;"),
            b'\'' => Some("&#39;"),
            _ => None,
        };
        if let Some(e) = rep {
            w.write_all(&bytes[start..i]).map_err(DomainError::Io)?;
            write!(w, "{e}").map_err(DomainError::Io)?;
            start = i + 1;
        }
    }
    w.write_all(&bytes[start..]).map_err(DomainError::Io)?;
    Ok(())
}

fn write_html_js<W: Write>(
    w: &mut W,
    tree: &serde_json::Value,
    top_files: &serde_json::Value,
    top_dirs: &serde_json::Value,
    classify: Option<&serde_json::Value>,
) -> Result<()> {
    // We use serde_json::to_writer for the data (no {} conflicts) and
    // write the rest of the JS via write!() with literal strings only.
    // The JS code uses string concatenation instead of template literals.
    write!(w, "<script>\nvar TREE=").map_err(DomainError::Io)?;
    serde_json::to_writer(&mut *w, tree).map_err(|e| ser_err(e))?;
    write!(w, ";var TOP_FILES=").map_err(DomainError::Io)?;
    serde_json::to_writer(&mut *w, top_files).map_err(|e| ser_err(e))?;
    write!(w, ";var TOP_DIRS=").map_err(DomainError::Io)?;
    serde_json::to_writer(&mut *w, top_dirs).map_err(|e| ser_err(e))?;
    write!(w, ";var CLASSIFY=").map_err(DomainError::Io)?;
    match classify {
        Some(v) => serde_json::to_writer(&mut *w, v).map_err(|e| ser_err(e))?,
        None => write!(w, "null").map_err(DomainError::Io)?,
    }
    write!(w, ";var TS=").map_err(DomainError::Io)?;
    let total = tree.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    write!(w, "{}", total).map_err(DomainError::Io)?;

    // 静态 JS+HTML 尾部从独立文件包含，避免原始字符串分隔符冲突风险
    write!(w, "{}", include_str!("report_footer.html")).map_err(DomainError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn build_tree() -> FileNode {
        let mut root = FileNode::new_dir(PathBuf::from("/root"), None);
        root.children
            .push(FileNode::new_file(PathBuf::from("/root/a.txt"), 100, None));
        root.children
            .push(FileNode::new_file(PathBuf::from("/root/b.txt"), 300, None));
        let mut sub = FileNode::new_dir(PathBuf::from("/root/sub"), None);
        sub.children
            .push(FileNode::new_file(PathBuf::from("/root/sub/c.txt"), 200, None));
        root.children.push(sub);
        root.aggregate();
        root
    }

    #[test]
    fn render_tree_to_string() {
        let root = build_tree();
        let mut buf = Vec::new();
        ReportService::render_tree(&root, &mut buf, 10, 0.0).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("root"));
        assert!(out.contains("a.txt"));
        assert!(out.contains("b.txt"));
        assert!(out.contains("sub"));
    }

    #[test]
    fn top_n_files_works() {
        let root = build_tree();
        let top = ReportService::top_n_files_report(&root, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].size.0, 300);
        assert_eq!(top[1].size.0, 200);
    }

    #[test]
    fn top_n_dirs_works() {
        let root = build_tree();
        let top = ReportService::top_n_dirs_report(&root, 5);
        // root 自身和 sub 都是非空目录
        assert!(top.iter().any(|e| e.name == "root"));
        assert!(top.iter().any(|e| e.name == "sub"));
    }

    #[test]
    fn to_json_string_works() {
        let root = build_tree();
        let json = ReportService::to_json_string(&root).unwrap();
        assert!(json.contains("root"));
        assert!(json.contains("a.txt"));
    }
}
