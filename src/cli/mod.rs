//! CLI 命令行界面
//!
//! 使用 clap 解析参数，编排扫描与报表生成。

pub mod args;
pub mod output;

pub use args::{CliArgs, CliMode, OutputFormat};

use std::path::PathBuf;
use std::sync::Arc;

use crate::application::{ScanService, TrendService};
use crate::domain::error::Result;
use crate::domain::scan_engine::{ScanEngineType, ScanProgress};
use crate::infrastructure::HistoryStorage;

#[cfg(not(target_os = "windows"))]
use crate::infrastructure::FsScanEngine;

#[cfg(target_os = "windows")]
use crate::infrastructure::{FsScanEngine, MftScanEngine, UsnScanEngine};

/// CLI 入口
pub fn run(args: &CliArgs) -> Result<()> {
    let path = args.path.clone().unwrap_or_else(|| PathBuf::from("."));
    let options = args.to_scan_options();

    // ── 历史模式：仅查看历史，不扫描 ──
    if args.history && !args.history_save {
        return display_history(args, &path);
    }

    // ── 执行扫描 ──
    let engine: Arc<dyn crate::domain::scan_engine::ScanEngine> = create_engine(args.engine);
    let service = ScanService::new(engine);

    // 进度回调：每 1000 个文件输出一次
    let progress = |p: &ScanProgress| {
        if p.files_scanned % 1000 == 0 && p.files_scanned > 0 {
            eprintln!(
                "[扫描中] 文件 {} | 目录 {} | 已扫描 {} | 当前：{}",
                p.files_scanned,
                p.dirs_scanned,
                crate::domain::value_objects::ByteSize(p.bytes_scanned),
                p.current_path
            );
        }
    };

    eprintln!("开始扫描：{}", path.display());
    let (root, stats) = service.scan(path.clone(), &options, Some(&progress), None)?;

    // 输出统计摘要
    eprintln!();
    eprintln!("========== 扫描结果 ==========");
    eprintln!("总文件数：{}", stats.total_files);
    eprintln!("总目录数：{}", stats.total_dirs);
    eprintln!("总大小：{}", stats.total_size);
    eprintln!("扫描错误：{} 个", stats.error_count);
    eprintln!("耗时：{} ms", stats.elapsed_ms);
    eprintln!("==============================");
    eprintln!();

    // 根据输出格式生成报表
    output::emit_reports(args, &root, &stats)?;

    // ── 保存到历史数据库 ──
    if args.history_save {
        save_to_history(args, &path, &root, stats.elapsed_ms)?;
    }

    // ── 显示历史趋势 ──
    if args.history {
        display_history(args, &path)?;
    }

    Ok(())
}

/// 保存扫描结果到历史数据库
fn save_to_history(
    args: &CliArgs,
    path: &PathBuf,
    root: &crate::domain::FileNode,
    elapsed_ms: u64,
) -> Result<()> {
    let storage = open_history(args)?;
    let (snapshot, categories, dirs) = TrendService::build_snapshot(path, root, elapsed_ms);
    let scan_id = storage
        .save_snapshot(&snapshot, &categories, &dirs)
        .map_err(|e| crate::domain::DomainError::ScanFailed(format!("保存历史失败：{e}")))?;
    eprintln!("历史已保存（ID: {scan_id}）");
    Ok(())
}

/// 显示扫描历史趋势
fn display_history(args: &CliArgs, path: &PathBuf) -> Result<()> {
    let storage = open_history(args)?;
    let path_str = path.to_string_lossy().to_string();

    let snapshots = storage
        .list_snapshots(&path_str)
        .map_err(|e| crate::domain::DomainError::ScanFailed(format!("读取历史失败：{e}")))?;

    if snapshots.is_empty() {
        eprintln!("暂无 [{}] 的扫描历史", path_str);
        return Ok(());
    }

    eprintln!("\n========== 扫描历史趋势 ==========");
    eprintln!("路径：{}", path_str);
    eprintln!("扫描次数：{}", snapshots.len());
    eprintln!();

    if snapshots.len() >= 2 {
        let first = snapshots.first().unwrap();
        let last = snapshots.last().unwrap();
        let growth = last.total_size.saturating_sub(first.total_size);
        let pct = if first.total_size > 0 {
            (growth as f64 / first.total_size as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "总增长：{}（{:.1}%）",
            crate::domain::value_objects::ByteSize(growth),
            pct,
        );
        eprintln!("首次扫描：{}", first.scanned_at.format("%Y-%m-%d %H:%M"));
        eprintln!("最近扫描：{}", last.scanned_at.format("%Y-%m-%d %H:%M"));
        eprintln!();
    }

    eprintln!("{:<4} {:<20} {:>12} {:>10} {:>10}", "序号", "扫描时间", "总大小", "文件数", "耗时(ms)");
    eprintln!("{}", "-".repeat(60));
    for (i, s) in snapshots.iter().enumerate() {
        eprintln!(
            "{:<4} {:<20} {:>12} {:>10} {:>10}",
            i + 1,
            s.scanned_at.format("%Y-%m-%d %H:%M"),
            crate::domain::value_objects::ByteSize(s.total_size),
            s.total_files,
            s.elapsed_ms,
        );
    }
    eprintln!();

    Ok(())
}

/// 根据引擎类型创建扫描引擎
#[cfg(not(target_os = "windows"))]
fn create_engine(engine_type: ScanEngineType) -> Arc<dyn crate::domain::scan_engine::ScanEngine> {
    // 非 Windows 平台只支持 Fs 引擎
    Arc::new(FsScanEngine::new())
}

/// 根据引擎类型创建扫描引擎
#[cfg(target_os = "windows")]
fn create_engine(engine_type: ScanEngineType) -> Arc<dyn crate::domain::scan_engine::ScanEngine> {
    match engine_type {
        ScanEngineType::Mft => {
            tracing::info!("使用 MFT 直接读取引擎");
            Arc::new(MftScanEngine::new())
        }
        ScanEngineType::Usn => {
            tracing::info!("使用 USN Journal 增量引擎");
            Arc::new(UsnScanEngine::new())
        }
        ScanEngineType::Fs => {
            tracing::info!("使用 FsScanEngine（文件系统遍历）");
            Arc::new(FsScanEngine::new())
        }
    }
}

/// 打开历史数据库
fn open_history(args: &CliArgs) -> Result<HistoryStorage> {
    if let Some(db_path) = &args.history_path {
        HistoryStorage::open(db_path)
            .map_err(|e| crate::domain::DomainError::ScanFailed(format!("打开历史数据库失败：{e}")))
    } else {
        HistoryStorage::open_default()
            .map_err(|e| crate::domain::DomainError::ScanFailed(format!("打开历史数据库失败：{e}")))
    }
}
