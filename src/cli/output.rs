//! CLI 报表输出
//!
//! 根据用户参数生成对应格式的报表并输出到 stdout 或文件。
//! 主报表（tree/json/csv）输出到 stdout 或文件；
//! 辅助报表（TopN/分类/重复文件）输出到 stderr，避免污染管道。

use std::io::{self, Write};
use std::path::Path;

use super::args::{CliArgs, OutputFormat};
use crate::application::{self, ClassifyService, DuplicateService, ReportService};
use crate::domain::error::{DomainError, Result};
use crate::domain::file_node::FileNode;
use crate::domain::scan_engine::ScanStats;

/// 根据参数输出报表
pub fn emit_reports(args: &CliArgs, root: &FileNode, stats: &ScanStats) -> Result<()> {
    // 主报表
    match args.output {
        OutputFormat::Tree => emit_tree(args, root)?,
        OutputFormat::Json => emit_json(args, root)?,
        OutputFormat::Csv => emit_csv(args, root)?,
        OutputFormat::Html => emit_html(args, root, stats)?,
        OutputFormat::Icicle => emit_icicle_svg(args, root, application::IcicleDirection::TopDown)?,
        OutputFormat::FlameGraph => emit_icicle_svg(args, root, application::IcicleDirection::BottomUp)?,
        OutputFormat::All => {
            emit_tree(args, root)?;
            emit_json(args, root)?;
            emit_csv(args, root)?;
            emit_html(args, root, stats)?;
            emit_icicle_svg(args, root, application::IcicleDirection::TopDown)?;
            emit_icicle_svg(args, root, application::IcicleDirection::BottomUp)?;
        },
    }

    // 辅助报表输出到 stderr
    if args.top > 0 {
        emit_topn(args, root)?;
    }

    if args.classify {
        emit_classify(root)?;
    }

    if args.duplicates {
        emit_duplicates(args, root)?;
    }

    Ok(())
}

fn emit_tree(args: &CliArgs, root: &FileNode) -> Result<()> {
    if let Some(file) = &args.out_file {
        let mut f = std::fs::File::create(file).map_err(DomainError::Io)?;
        ReportService::render_tree(root, &mut f, args.depth, args.min_percent)?;
        eprintln!("树形报表已写入：{}", file.display());
    } else {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        ReportService::render_tree(root, &mut lock, args.depth, args.min_percent)?;
    }
    Ok(())
}

fn emit_json(args: &CliArgs, root: &FileNode) -> Result<()> {
    let path = args.out_file.as_deref().map(|p| {
        if matches!(args.output, OutputFormat::All) {
            with_extension(p, "json")
        } else {
            p.to_path_buf()
        }
    });

    if let Some(file) = path {
        ReportService::write_json(root, &file)?;
        eprintln!("JSON 报表已写入：{}", file.display());
    } else {
        let json = ReportService::to_json_string(root)?;
        println!("{json}");
    }
    Ok(())
}

fn emit_csv(args: &CliArgs, root: &FileNode) -> Result<()> {
    let path = args.out_file.as_deref().map(|p| {
        if matches!(args.output, OutputFormat::All) {
            with_extension(p, "csv")
        } else {
            p.to_path_buf()
        }
    });

    if let Some(file) = path {
        ReportService::write_csv(root, &file)?;
        eprintln!("CSV 报表已写入：{}", file.display());
    } else {
        // CSV 输出到 stdout，复用 ReportService 的 write_csv_to_buf 避免代码重复
        let buf = ReportService::write_csv_to_buf(root)?;
        io::stdout().write_all(&buf).map_err(DomainError::Io)?;
    }
    Ok(())
}

/// TopN 报表输出到 stderr
fn emit_topn(args: &CliArgs, root: &FileNode) -> Result<()> {
    eprintln!("\n========== Top {} 大文件 ==========", args.top);
    let files = ReportService::top_n_files_report(root, args.top);
    for (i, e) in files.iter().enumerate() {
        eprintln!("{:>3}. {:>10} ({:>5.2}%)  {}", i + 1, e.size, e.percent, e.path);
    }

    eprintln!("\n========== Top {} 大目录 ==========", args.top);
    let dirs = ReportService::top_n_dirs_report(root, args.top);
    for (i, e) in dirs.iter().enumerate() {
        eprintln!("{:>3}. {:>10} ({:>5.2}%)  {}", i + 1, e.size, e.percent, e.path);
    }
    Ok(())
}

/// 分类统计输出到 stderr
fn emit_classify(root: &FileNode) -> Result<()> {
    let report = ClassifyService::analyze(root);

    eprintln!("\n========== 文件分类统计 ==========");
    eprintln!("总文件数：{}", report.total_files);
    eprintln!("总大小：{}", report.total_size);
    eprintln!("\n按大类：");
    eprintln!("{:<10} {:>12} {:>10} {:>10}", "类别", "大小", "占比", "文件数");
    eprintln!("{}", "-".repeat(46));
    for c in &report.by_category {
        eprintln!(
            "{:<10} {:>12} {:>9.2}% {:>10}",
            c.label, c.total_size, c.percent, c.file_count
        );
    }

    eprintln!("\n按扩展名（前 20）：");
    eprintln!("{:<12} {:>12} {:>10} {:>10}", "扩展名", "大小", "占比", "文件数");
    eprintln!("{}", "-".repeat(48));
    for e in report.by_extension.iter().take(20) {
        eprintln!(
            "{:<12} {:>12} {:>9.2}% {:>10}",
            e.extension, e.total_size, e.percent, e.file_count
        );
    }
    Ok(())
}

/// 重复文件扫描输出到 stderr
fn emit_duplicates(args: &CliArgs, root: &FileNode) -> Result<()> {
    eprintln!("\n========== 重复文件扫描 ==========");
    eprintln!(
        "最小关注大小：{}",
        crate::domain::value_objects::ByteSize(args.dup_min_size)
    );
    let report = DuplicateService::scan(root, args.dup_min_size);

    if report.groups.is_empty() {
        eprintln!("未发现重复文件");
        return Ok(());
    }

    eprintln!(
        "发现 {} 组重复文件，共 {} 个文件，浪费空间 {}",
        report.groups.len(),
        report.total_duplicate_files,
        report.total_wasted
    );
    eprintln!();

    for (i, g) in report.groups.iter().enumerate() {
        // 哈希前 16 字符用于展示，避免硬编码切片长度
        let hash_preview: String = g.hash.chars().take(16).collect();
        eprintln!(
            "组 {}：{} × {} (浪费 {}) 哈希：{}",
            i + 1,
            g.paths.len(),
            g.size,
            g.wasted,
            hash_preview
        );
        for p in &g.paths {
            eprintln!("    {}", p.display());
        }
        eprintln!();
    }
    Ok(())
}

fn emit_html(args: &CliArgs, root: &FileNode, stats: &ScanStats) -> Result<()> {
    let path = args.out_file.as_deref().map(|p| {
        if matches!(args.output, OutputFormat::All) {
            with_extension(p, "html")
        } else {
            p.to_path_buf()
        }
    });

    let classify = if args.classify {
        Some(ClassifyService::analyze(root))
    } else {
        None
    };

    if let Some(file) = &path {
        let mut f = std::fs::File::create(file).map_err(DomainError::Io)?;
        ReportService::write_html(
            root,
            &mut f,
            stats,
            &args.path.clone().unwrap_or_else(|| std::path::PathBuf::from(".")),
            args.top,
            classify.as_ref(),
        )?;
        eprintln!("HTML 报表已写入：{}", file.display());
    } else {
        let mut buf = Vec::new();
        ReportService::write_html(
            root,
            &mut buf,
            stats,
            &args.path.clone().unwrap_or_else(|| std::path::PathBuf::from(".")),
            args.top,
            classify.as_ref(),
        )?;
        // stdout 输出 HTML
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        lock.write_all(&buf).map_err(DomainError::Io)?;
    }
    Ok(())
}

fn emit_icicle_svg(args: &CliArgs, root: &FileNode, direction: application::IcicleDirection) -> Result<()> {
    use application::{compute_layout_v2, render_svg, LayoutConfig};
    use std::path::PathBuf;

    let config = LayoutConfig {
        direction,
        ..Default::default()
    };
    let blocks = compute_layout_v2(root, &config);
    let svg = render_svg(&blocks, &config);

    let label = match direction {
        application::IcicleDirection::TopDown => "icicle",
        application::IcicleDirection::BottomUp => "flamegraph",
    };

    let path = args.out_file.as_deref().map(|p| {
        if matches!(args.output, OutputFormat::All) {
            let mut p = p.to_path_buf();
            p.set_extension(label);
            let mut s = p.to_string_lossy().to_string();
            s.push_str(".svg");
            PathBuf::from(s)
        } else {
            p.to_path_buf()
        }
    });

    if let Some(file) = &path {
        let mut f = std::fs::File::create(file).map_err(DomainError::Io)?;
        f.write_all(svg.as_bytes()).map_err(DomainError::Io)?;
        eprintln!("{} SVG 已写入：{}", label, file.display());
    } else {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        lock.write_all(svg.as_bytes()).map_err(DomainError::Io)?;
    }
    Ok(())
}

/// 替换文件扩展名
fn with_extension(path: &Path, ext: &str) -> std::path::PathBuf {
    let mut p = path.to_path_buf();
    p.set_extension(ext);
    p
}
