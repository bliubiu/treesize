//! CLI 参数定义
//!
//! 使用 clap derive 风格定义命令行参数。

use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::domain::scan_engine::{ResourceLimits, ScanEngineType, ScanOptions};
use crate::infrastructure::logging::LogLevel;

/// 运行模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
pub enum CliMode {
    /// 命令行模式
    Cli,
    /// 图形界面模式
    Gui,
}

/// 输出格式
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default, Serialize, Deserialize)]
pub enum OutputFormat {
    /// 树形文本（默认）
    #[default]
    Tree,
    /// JSON
    Json,
    /// CSV
    Csv,
    /// HTML 交互式报表
    Html,
    /// Icicle 图 SVG
    Icicle,
    /// Flame Graph SVG
    FlameGraph,
    /// 全部格式
    All,
}

/// treesize：磁盘占用分析工具
///
/// 集成 WizTree、TreeSize Free、WinDirStat 等工具的优势，
/// 提供树形目录、Treemap、多维报表、文件分类统计、重复文件扫描。
#[derive(Parser, Debug, Serialize, Deserialize)]
#[command(name = "treesize", version, about, long_about = None)]
pub struct CliArgs {
    /// 待扫描的路径（默认当前目录）
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// 运行模式：cli 或 gui
    #[arg(long = "mode", value_enum, default_value_t = CliMode::Cli)]
    pub mode: CliMode,

    /// 输出格式
    #[arg(short = 'o', long = "output", value_enum, default_value_t = OutputFormat::Tree)]
    pub output: OutputFormat,

    /// 输出文件路径（不指定则输出到 stdout）
    #[arg(long = "out-file", value_name = "FILE")]
    pub out_file: Option<PathBuf>,

    /// 树形显示最大深度（0 = 不限）
    #[arg(short = 'd', long = "depth", default_value_t = 10)]
    pub depth: usize,

    /// 显示阈值：占比小于此百分比的节点折叠（0.0 = 显示全部）
    #[arg(long = "min-percent", default_value_t = 0.0)]
    pub min_percent: f64,

    /// 显示 TopN 大文件
    #[arg(long = "top", value_name = "N", default_value_t = 20)]
    pub top: usize,

    /// 启用文件分类统计
    #[arg(long = "classify", action = ArgAction::SetTrue)]
    pub classify: bool,

    /// 启用重复文件扫描
    #[arg(long = "duplicates", action = ArgAction::SetTrue)]
    pub duplicates: bool,

    /// 重复文件扫描的最小大小（字节），默认 1024
    #[arg(long = "dup-min-size", default_value_t = 1024)]
    pub dup_min_size: u64,

    /// 启用扫描历史趋势追踪
    #[arg(long = "history", action = ArgAction::SetTrue)]
    pub history: bool,

    /// 扫描后是否保存到历史数据库
    #[arg(long = "history-save", action = ArgAction::SetTrue)]
    pub history_save: bool,

    /// 历史数据库路径（默认用户数据目录）
    #[arg(long = "history-path")]
    pub history_path: Option<PathBuf>,

    /// 跟随符号链接
    #[arg(long = "follow-links", action = ArgAction::SetTrue)]
    pub follow_links: bool,

    /// 不包含隐藏文件
    #[arg(long = "no-hidden", action = ArgAction::SetTrue)]
    pub no_hidden: bool,

    /// 最小文件大小过滤（字节）
    #[arg(long = "min-size", default_value_t = 0)]
    pub min_size: u64,

    /// 排除目录名（可多次指定）
    #[arg(long = "exclude-dir", value_name = "NAME")]
    pub exclude_dirs: Vec<String>,

    /// 排除扩展名（可多次指定）
    #[arg(long = "exclude-ext", value_name = "EXT")]
    pub exclude_exts: Vec<String>,

    /// 日志级别
    #[arg(long = "log-level", value_enum, default_value_t = LogLevel::Info)]
    pub log_level: LogLevel,

    /// 日志目录
    #[arg(long = "log-dir", default_value = "logs")]
    pub log_dir: PathBuf,

    /// 扫描引擎类型：fs（默认，跨平台）| mft（仅 Windows NTFS 快速）| usn（仅 Windows NTFS 增量）
    #[arg(long = "engine", default_value_t = ScanEngineType::Fs)]
    pub engine: ScanEngineType,
}

impl CliArgs {
    /// 将 CLI 参数转换为扫描选项
    pub fn to_scan_options(&self) -> ScanOptions {
        ScanOptions {
            engine: self.engine,
            follow_links: self.follow_links,
            include_hidden: !self.no_hidden,
            max_depth: 0,
            min_size: self.min_size,
            exclude_dirs: self.exclude_dirs.clone(),
            exclude_exts: self.exclude_exts.clone(),
            resource_limits: ResourceLimits::default(),
            incremental: false,
            incremental_since: None,
        }
    }
}

// 手动实现 Default，因为 clap 的 default_value 已覆盖大部分字段
impl Default for CliArgs {
    fn default() -> Self {
        Self {
            path: None,
            mode: CliMode::Cli,
            output: OutputFormat::Tree,
            out_file: None,
            depth: 10,
            min_percent: 0.0,
            top: 20,
            classify: false,
            duplicates: false,
            dup_min_size: 1024,
            history: false,
            history_save: false,
            history_path: None,
            follow_links: false,
            no_hidden: false,
            min_size: 0,
            exclude_dirs: vec![],
            exclude_exts: vec![],
            log_level: LogLevel::Info,
            log_dir: PathBuf::from("logs"),
            engine: ScanEngineType::Fs,
        }
    }
}

// LogLevel 需要实现 ValueEnum
impl clap::ValueEnum for LogLevel {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Debug, Self::Info, Self::Error]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        match self {
            Self::Debug => Some(clap::builder::PossibleValue::new("debug")),
            Self::Info => Some(clap::builder::PossibleValue::new("info")),
            Self::Error => Some(clap::builder::PossibleValue::new("error")),
        }
    }
}
