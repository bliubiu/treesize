//! 扫描历史领域模型
//!
//! 定义扫描快照等核心领域实体，用于"空间增长趋势"跟踪。

use std::path::PathBuf;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// 单次扫描快照摘要（持久化存储）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanSnapshot {
    /// 数据库 ID（新记录为 None）
    pub id: Option<i64>,
    /// 扫描根路径
    pub scanned_path: String,
    /// 扫描时间
    pub scanned_at: DateTime<Local>,
    /// 总文件数
    pub total_files: u64,
    /// 总目录数
    pub total_dirs: u64,
    /// 总大小（字节）
    pub total_size: u64,
    /// 扫描耗时（毫秒）
    pub elapsed_ms: u64,
}

/// 文件大类快照（每扫描一条记录）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorySnapshot {
    /// 类别名
    pub category: String,
    /// 该类文件总大小
    pub size: u64,
    /// 该类文件数
    pub file_count: u64,
}

/// 顶层子目录大小快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirSizeSnapshot {
    /// 相对路径（相对于扫描根目录）
    pub relative_path: String,
    /// 大小（字节）
    pub size: u64,
}

impl ScanSnapshot {
    /// 创建新快照
    pub fn new(scanned_path: &PathBuf, total_files: u64, total_dirs: u64, total_size: u64, elapsed_ms: u64) -> Self {
        Self {
            id: None,
            scanned_path: scanned_path.to_string_lossy().to_string(),
            scanned_at: Local::now(),
            total_files,
            total_dirs,
            total_size,
            elapsed_ms,
        }
    }
}
