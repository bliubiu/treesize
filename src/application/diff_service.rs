//! 快照对比服务
//!
//! 比较两次扫描快照，生成差异报告。

use std::collections::HashMap;

use crate::application::models::{CategoryDiff, DirDiff, SnapshotDiff};
use crate::domain::scan_history::{CategorySnapshot, DirSizeSnapshot, ScanSnapshot};

/// 快照对比服务
pub struct SnapshotDiffService;

impl SnapshotDiffService {
    /// 对比两次快照，生成差异报告
    ///
    /// # 参数
    /// - `old`: 旧快照
    /// - `new`: 新快照
    /// - `old_cats`: 旧快照的分类统计
    /// - `new_cats`: 新快照的分类统计
    /// - `old_dirs`: 旧快照的目录统计
    /// - `new_dirs`: 新快照的目录统计
    pub fn compare(
        old: &ScanSnapshot,
        new: &ScanSnapshot,
        old_cats: &[CategorySnapshot],
        new_cats: &[CategorySnapshot],
        old_dirs: &[DirSizeSnapshot],
        new_dirs: &[DirSizeSnapshot],
    ) -> SnapshotDiff {
        // 计算总体变化
        let size_delta = new.total_size as i64 - old.total_size as i64;
        let size_delta_pct = if old.total_size > 0 {
            (size_delta as f64 / old.total_size as f64) * 100.0
        } else {
            0.0
        };
        let file_delta = new.total_files as i64 - old.total_files as i64;
        let dir_delta = new.total_dirs as i64 - old.total_dirs as i64;

        // 对比目录变化
        let (dir_diffs, new_dirs_list, removed_dirs_list) = Self::compare_dirs(old_dirs, new_dirs);

        // 对比分类变化
        let category_diffs = Self::compare_categories(old_cats, new_cats);

        SnapshotDiff {
            old: old.clone(),
            new: new.clone(),
            size_delta,
            size_delta_pct,
            file_delta,
            dir_delta,
            dir_diffs,
            category_diffs,
            new_dirs: new_dirs_list,
            removed_dirs: removed_dirs_list,
        }
    }

    /// 对比目录列表，返回 (共同目录的变化, 新增目录, 消失目录)
    fn compare_dirs(
        old_dirs: &[DirSizeSnapshot],
        new_dirs: &[DirSizeSnapshot],
    ) -> (Vec<DirDiff>, Vec<DirDiff>, Vec<DirDiff>) {
        let old_map: HashMap<&str, u64> = old_dirs.iter().map(|d| (d.relative_path.as_str(), d.size)).collect();

        let new_map: HashMap<&str, u64> = new_dirs.iter().map(|d| (d.relative_path.as_str(), d.size)).collect();

        let mut dir_diffs = Vec::new();
        let mut new_dirs_list = Vec::new();
        let mut removed_dirs_list = Vec::new();

        // 检查新快照中的目录
        for (name, new_size) in &new_map {
            if let Some(&old_size) = old_map.get(name) {
                // 共同目录，计算变化
                if old_size != *new_size {
                    dir_diffs.push(DirDiff {
                        name: name.to_string(),
                        old_size,
                        new_size: *new_size,
                    });
                }
            } else {
                // 新增目录
                new_dirs_list.push(DirDiff {
                    name: name.to_string(),
                    old_size: 0,
                    new_size: *new_size,
                });
            }
        }

        // 检查消失的目录
        for (name, old_size) in &old_map {
            if !new_map.contains_key(name) {
                removed_dirs_list.push(DirDiff {
                    name: name.to_string(),
                    old_size: *old_size,
                    new_size: 0,
                });
            }
        }

        // 按变化量绝对值降序排序
        dir_diffs.sort_by(|a, b| b.delta().abs().cmp(&a.delta().abs()));
        new_dirs_list.sort_by(|a, b| b.new_size.cmp(&a.new_size));
        removed_dirs_list.sort_by(|a, b| b.old_size.cmp(&a.old_size));

        (dir_diffs, new_dirs_list, removed_dirs_list)
    }

    /// 对比分类统计
    fn compare_categories(old_cats: &[CategorySnapshot], new_cats: &[CategorySnapshot]) -> Vec<CategoryDiff> {
        let old_map: HashMap<&str, &CategorySnapshot> = old_cats.iter().map(|c| (c.category.as_str(), c)).collect();

        let new_map: HashMap<&str, &CategorySnapshot> = new_cats.iter().map(|c| (c.category.as_str(), c)).collect();

        let mut diffs = Vec::new();

        // 检查所有分类（合并新旧）
        let all_categories: std::collections::HashSet<&str> = old_map.keys().chain(new_map.keys()).copied().collect();

        for category in all_categories {
            let old = old_map.get(category);
            let new = new_map.get(category);

            let diff = CategoryDiff {
                category: category.to_string(),
                old_size: old.map(|c| c.size).unwrap_or(0),
                new_size: new.map(|c| c.size).unwrap_or(0),
                old_count: old.map(|c| c.file_count).unwrap_or(0),
                new_count: new.map(|c| c.file_count).unwrap_or(0),
            };

            // 只保留有变化的分类
            if diff.size_delta() != 0 || diff.count_delta() != 0 {
                diffs.push(diff);
            }
        }

        // 按变化量绝对值降序排序
        diffs.sort_by(|a, b| b.size_delta().abs().cmp(&a.size_delta().abs()));

        diffs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;

    fn make_snapshot(path: &str, files: u64, dirs: u64, size: u64) -> ScanSnapshot {
        ScanSnapshot {
            id: None,
            scanned_path: path.to_string(),
            scanned_at: Local::now(),
            total_files: files,
            total_dirs: dirs,
            total_size: size,
            elapsed_ms: 100,
        }
    }

    #[test]
    fn test_compare_basic() {
        let old = make_snapshot("/test", 100, 10, 1000);
        let new = make_snapshot("/test", 120, 12, 1500);

        let diff = SnapshotDiffService::compare(&old, &new, &[], &[], &[], &[]);

        assert_eq!(diff.size_delta, 500);
        assert_eq!(diff.file_delta, 20);
        assert_eq!(diff.dir_delta, 2);
        assert!((diff.size_delta_pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_compare_dirs() {
        let old_dirs = vec![
            DirSizeSnapshot {
                relative_path: "dir1".to_string(),
                size: 100,
            },
            DirSizeSnapshot {
                relative_path: "dir2".to_string(),
                size: 200,
            },
            DirSizeSnapshot {
                relative_path: "dir3".to_string(),
                size: 300,
            },
        ];

        let new_dirs = vec![
            DirSizeSnapshot {
                relative_path: "dir1".to_string(),
                size: 150, // 增长
            },
            DirSizeSnapshot {
                relative_path: "dir2".to_string(),
                size: 200, // 不变
            },
            DirSizeSnapshot {
                relative_path: "dir4".to_string(),
                size: 400, // 新增
            },
        ];

        let (diffs, new_list, removed_list) = SnapshotDiffService::compare_dirs(&old_dirs, &new_dirs);

        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].name, "dir1");
        assert_eq!(diffs[0].delta(), 50);

        assert_eq!(new_list.len(), 1);
        assert_eq!(new_list[0].name, "dir4");
        assert_eq!(new_list[0].new_size, 400);

        assert_eq!(removed_list.len(), 1);
        assert_eq!(removed_list[0].name, "dir3");
        assert_eq!(removed_list[0].old_size, 300);
    }

    #[test]
    fn test_compare_categories() {
        let old_cats = vec![
            CategorySnapshot {
                category: "文档".to_string(),
                size: 100,
                file_count: 10,
            },
            CategorySnapshot {
                category: "图片".to_string(),
                size: 200,
                file_count: 20,
            },
        ];

        let new_cats = vec![
            CategorySnapshot {
                category: "文档".to_string(),
                size: 150,
                file_count: 15,
            },
            CategorySnapshot {
                category: "视频".to_string(),
                size: 300,
                file_count: 5,
            },
        ];

        let diffs = SnapshotDiffService::compare_categories(&old_cats, &new_cats);

        assert_eq!(diffs.len(), 3); // 文档变化 + 图片消失 + 视频新增

        // 按变化量排序，视频新增最大
        assert_eq!(diffs[0].category, "视频");
        assert_eq!(diffs[0].size_delta(), 300);

        // 文档增长
        let doc_diff = diffs.iter().find(|d| d.category == "文档").unwrap();
        assert_eq!(doc_diff.size_delta(), 50);
        assert_eq!(doc_diff.count_delta(), 5);

        // 图片消失
        let img_diff = diffs.iter().find(|d| d.category == "图片").unwrap();
        assert_eq!(img_diff.size_delta(), -200);
        assert_eq!(img_diff.count_delta(), -20);
    }

    #[test]
    fn test_compare_size_decrease() {
        let old = make_snapshot("/test", 200, 20, 2000);
        let new = make_snapshot("/test", 150, 15, 1000);

        let diff = SnapshotDiffService::compare(&old, &new, &[], &[], &[], &[]);

        assert_eq!(diff.size_delta, -1000);
        assert_eq!(diff.file_delta, -50);
        assert_eq!(diff.dir_delta, -5);
        assert!((diff.size_delta_pct - (-50.0)).abs() < 0.01);
    }

    #[test]
    fn test_compare_identical() {
        let old = make_snapshot("/test", 100, 10, 1000);
        let new = make_snapshot("/test", 100, 10, 1000);

        let diff = SnapshotDiffService::compare(&old, &new, &[], &[], &[], &[]);

        assert_eq!(diff.size_delta, 0);
        assert_eq!(diff.file_delta, 0);
        assert_eq!(diff.dir_delta, 0);
        assert!((diff.size_delta_pct - 0.0).abs() < 0.01);
        assert!(diff.dir_diffs.is_empty());
        assert!(diff.new_dirs.is_empty());
        assert!(diff.removed_dirs.is_empty());
        assert!(diff.category_diffs.is_empty());
    }
}
