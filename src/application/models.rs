//! 应用层 DTO（数据传输对象）
//!
//! 包含趋势报告、快照对比等应用层数据结构。
//! 这些类型由应用服务构建，用于在层之间传递数据。

use serde::{Deserialize, Serialize};

use crate::domain::scan_history::{DirSizeSnapshot, ScanSnapshot};

// ─── 趋势分析类型 ──────────────────────────────────────────────────────────

/// 时间点数据（折线图的一个点）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SizePoint {
    /// 日期
    pub date: chrono::NaiveDate,
    /// 总大小（字节）
    pub total_size: u64,
}

/// 增长最快的条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthEntry {
    /// 条目名称（目录名或类别名）
    pub name: String,
    /// 最早记录大小
    pub initial_size: u64,
    /// 最新记录大小
    pub latest_size: u64,
    /// 增长量（字节）
    pub growth_bytes: u64,
    /// 增长率（百分比）
    pub growth_pct: f64,
}

/// 类别趋势
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryTrend {
    /// 类别名
    pub category: String,
    /// 大小历史
    pub size_history: Vec<SizePoint>,
}

/// 完整趋势报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendReport {
    /// 扫描路径
    pub path: String,
    /// 快照列表
    pub snapshots: Vec<ScanSnapshot>,
    /// 总大小趋势（折线图数据）
    pub size_trend: Vec<SizePoint>,
    /// 增长最快的顶层目录
    pub top_growing: Vec<GrowthEntry>,
    /// 各文件大类趋势
    pub category_trends: Vec<CategoryTrend>,
}

// ─── 快照对比类型 ──────────────────────────────────────────────────────────

/// 目录大小变化条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirDiff {
    /// 目录名
    pub name: String,
    /// 旧大小（字节）
    pub old_size: u64,
    /// 新大小（字节）
    pub new_size: u64,
}

impl DirDiff {
    /// 变化量（字节），正数为增长，负数为缩小
    pub fn delta(&self) -> i64 {
        self.new_size as i64 - self.old_size as i64
    }

    /// 变化率（百分比）
    pub fn delta_pct(&self) -> f64 {
        if self.old_size == 0 {
            if self.new_size == 0 { 0.0 } else { 100.0 }
        } else {
            (self.delta() as f64 / self.old_size as f64) * 100.0
        }
    }
}

/// 文件大类变化条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryDiff {
    /// 类别名
    pub category: String,
    /// 旧大小
    pub old_size: u64,
    /// 新大小
    pub new_size: u64,
    /// 旧文件数
    pub old_count: u64,
    /// 新文件数
    pub new_count: u64,
}

impl CategoryDiff {
    pub fn size_delta(&self) -> i64 {
        self.new_size as i64 - self.old_size as i64
    }

    pub fn count_delta(&self) -> i64 {
        self.new_count as i64 - self.old_count as i64
    }
}

/// 快照对比报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDiff {
    /// 旧快照
    pub old: ScanSnapshot,
    /// 新快照
    pub new: ScanSnapshot,
    /// 总大小变化
    pub size_delta: i64,
    /// 总大小变化率
    pub size_delta_pct: f64,
    /// 文件数变化
    pub file_delta: i64,
    /// 目录数变化
    pub dir_delta: i64,
    /// 顶层目录变化（按变化量绝对值降序）
    pub dir_diffs: Vec<DirDiff>,
    /// 文件大类变化（按变化量绝对值降序）
    pub category_diffs: Vec<CategoryDiff>,
    /// 新增目录（旧快照中不存在）
    pub new_dirs: Vec<DirDiff>,
    /// 消失目录（新快照中不存在）
    pub removed_dirs: Vec<DirDiff>,
}

/// 计算两次扫描之间增长最快的顶层子目录（Top 10）
///
/// 对比最早和最晚快照中各子目录的大小变化，按增长量降序排列。
/// 仅包含正增长的目录（新出现的目录视为 100% 增长）。
pub fn compute_top_growing(
    snapshots: &[ScanSnapshot],
    dirs_by_scan: &[Vec<DirSizeSnapshot>],
) -> Vec<GrowthEntry> {
    if snapshots.len() < 2 || dirs_by_scan.len() < 2 {
        return vec![];
    }

    let earliest_dirs = &dirs_by_scan[0];
    let latest_dirs = &dirs_by_scan[dirs_by_scan.len() - 1];

    // 最早快照的目录大小索引
    let earliest_map: std::collections::HashMap<&str, u64> = earliest_dirs
        .iter()
        .map(|d| (d.relative_path.as_str(), d.size))
        .collect();

    let mut entries: Vec<GrowthEntry> = latest_dirs
        .iter()
        .map(|dir| {
            let initial_size = earliest_map.get(dir.relative_path.as_str()).copied().unwrap_or(0);
            let latest_size = dir.size;
            let growth_bytes = latest_size.saturating_sub(initial_size);
            let growth_pct = if initial_size > 0 {
                (growth_bytes as f64 / initial_size as f64) * 100.0
            } else if latest_size > 0 {
                100.0 // 新目录标记为 100% 增长
            } else {
                0.0
            };

            GrowthEntry {
                name: dir.relative_path.clone(),
                initial_size,
                latest_size,
                growth_bytes,
                growth_pct,
            }
        })
        .filter(|e| e.growth_bytes > 0)
        .collect();

    entries.sort_by(|a, b| b.growth_bytes.cmp(&a.growth_bytes));
    entries.truncate(10);
    entries
}
