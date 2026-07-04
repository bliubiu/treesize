//! 趋势分析服务
//!
//! 对扫描历史数据进行统计分析，识别增长趋势。
//! 领域逻辑集中在 `TrendReport` 和 `GrowthEntry` 的构造。

use crate::domain::file_node::FileNode;
use crate::application::models::{
    compute_top_growing, CategoryTrend, SizePoint, TrendReport,
};
use crate::domain::scan_history::{
    CategorySnapshot, DirSizeSnapshot, ScanSnapshot,
};

/// 趋势分析服务
pub struct TrendService;

impl TrendService {
    /// 从扫描结果构建快照数据
    pub fn build_snapshot(
        path: &std::path::PathBuf,
        node: &FileNode,
        elapsed_ms: u64,
    ) -> (ScanSnapshot, Vec<CategorySnapshot>, Vec<DirSizeSnapshot>) {
        let snapshot = ScanSnapshot::new(path, node.file_count, node.dir_count, node.size.0, elapsed_ms);

        // 分类统计
        let mut category_map: std::collections::HashMap<String, (u64, u64)> =
            std::collections::HashMap::new();
        for n in node.iter_all() {
            if n.is_file() {
                let entry = category_map
                    .entry(n.category.label().to_string())
                    .or_insert((0, 0));
                entry.0 += n.size.0;
                entry.1 += 1;
            }
        }
        let categories: Vec<CategorySnapshot> = category_map
            .into_iter()
            .map(|(cat, (size, count))| CategorySnapshot {
                category: cat,
                size,
                file_count: count,
            })
            .collect();

        // 顶层目录大小
        let mut dirs: Vec<DirSizeSnapshot> = node
            .children
            .iter()
            .filter(|c| c.is_dir() && c.size.0 > 0)
            .map(|c| DirSizeSnapshot {
                relative_path: c.name.clone(),
                size: c.size.0,
            })
            .collect();
        dirs.sort_by(|a, b| b.size.cmp(&a.size));

        (snapshot, categories, dirs)
    }

    /// 从历史记录生成趋势报告
    pub fn build_trend_report(
        snapshots: &[ScanSnapshot],
        categories_by_scan: &[Vec<CategorySnapshot>],
        dirs_by_scan: &[Vec<DirSizeSnapshot>],
    ) -> TrendReport {
        if snapshots.is_empty() {
            return TrendReport {
                path: String::new(),
                snapshots: vec![],
                size_trend: vec![],
                top_growing: vec![],
                category_trends: vec![],
            };
        }

        let path = snapshots[0].scanned_path.clone();

        // 总大小趋势
        let size_trend: Vec<SizePoint> = snapshots
            .iter()
            .map(|s| SizePoint {
                date: s.scanned_at.date_naive(),
                total_size: s.total_size,
            })
            .collect();

        // 类别趋势
        let mut category_trends: Vec<CategoryTrend> = Vec::new();
        for (i, snap) in snapshots.iter().enumerate() {
            if let Some(cats) = categories_by_scan.get(i) {
                for cat in cats {
                    let trend = category_trends
                        .iter_mut()
                        .find(|t: &&mut CategoryTrend| t.category == cat.category);
                    if let Some(t) = trend {
                        t.size_history.push(SizePoint {
                            date: snap.scanned_at.date_naive(),
                            total_size: cat.size,
                        });
                    } else {
                        category_trends.push(CategoryTrend {
                            category: cat.category.clone(),
                            size_history: vec![SizePoint {
                                date: snap.scanned_at.date_naive(),
                                total_size: cat.size,
                            }],
                        });
                    }
                }
            }
        }

        // 增长最快的顶层子目录（Top 10）
        let top_growing = compute_top_growing(snapshots, dirs_by_scan);

        TrendReport {
            path,
            snapshots: snapshots.to_vec(),
            size_trend,
            top_growing,
            category_trends,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::file_node::FileNode;
    use crate::domain::scan_history::CategorySnapshot;
    use std::path::PathBuf;

    fn build_test_tree() -> FileNode {
        let mut root = FileNode::new_dir(PathBuf::from("/test"), None);
        root.children
            .push(FileNode::new_file(PathBuf::from("/test/a.txt"), 100, None));
        root.children
            .push(FileNode::new_file(PathBuf::from("/test/b.mp4"), 500, None));
        root.children
            .push(FileNode::new_file(PathBuf::from("/test/c.txt"), 50, None));
        let mut sub = FileNode::new_dir(PathBuf::from("/test/sub"), None);
        sub.children
            .push(FileNode::new_file(PathBuf::from("/test/sub/d.rs"), 200, None));
        root.children.push(sub);
        root.aggregate();
        root
    }

    #[test]
    fn build_snapshot_returns_correct_stats() {
        let root = build_test_tree();
        let path = PathBuf::from("/test");

        let (snapshot, categories, dirs) = TrendService::build_snapshot(&path, &root, 1234);

        // 验证 ScanSnapshot 基本字段
        assert_eq!(snapshot.scanned_path, "/test");
        assert_eq!(snapshot.total_files, 4);
        assert_eq!(snapshot.total_dirs, 1);
        assert_eq!(snapshot.total_size, 850);
        assert_eq!(snapshot.elapsed_ms, 1234);
        assert!(snapshot.id.is_none());

        // 验证分类统计
        assert!(!categories.is_empty());
        let doc_cat = categories
            .iter()
            .find(|c| c.category == "文档")
            .expect("应有文档分类");
        assert_eq!(doc_cat.file_count, 2); // a.txt + c.txt
        assert_eq!(doc_cat.size, 150);

        let video_cat = categories
            .iter()
            .find(|c| c.category == "视频")
            .expect("应有视频分类");
        assert_eq!(video_cat.file_count, 1);
        assert_eq!(video_cat.size, 500);

        let source_cat = categories
            .iter()
            .find(|c| c.category == "源代码")
            .expect("应有源代码分类");
        assert_eq!(source_cat.file_count, 1);
        assert_eq!(source_cat.size, 200);

        // 验证顶层目录统计
        let dir_names: Vec<&str> = dirs.iter().map(|d| d.relative_path.as_str()).collect();
        assert!(dir_names.contains(&"sub"), "应包含 sub 目录");
        let sub_dir = dirs.iter().find(|d| d.relative_path == "sub").unwrap();
        assert_eq!(sub_dir.size, 200);
    }

    #[test]
    fn build_snapshot_with_single_file() {
        let file = FileNode::new_file(PathBuf::from("/test/alone.bin"), 42, None);
        let path = PathBuf::from("/test");

        let (snapshot, categories, _dirs) = TrendService::build_snapshot(&path, &file, 0);

        // 单文件场景：file_count 应为 0（非聚合值）
        assert_eq!(snapshot.total_files, 0);
        // 分类应包含"其他"
        assert!(categories.iter().any(|c| c.category == "其他"));
    }

    #[test]
    fn build_trend_report_empty_returns_default() {
        let report = TrendService::build_trend_report(&[], &[], &[]);

        assert_eq!(report.path, "");
        assert!(report.snapshots.is_empty());
        assert!(report.size_trend.is_empty());
        assert!(report.top_growing.is_empty());
        assert!(report.category_trends.is_empty());
    }

    #[test]
    fn build_trend_report_single_snapshot() {
        let snap =
            ScanSnapshot::new(&PathBuf::from("/test"), 100, 10, 1_000_000, 500);
        let cats = vec![CategorySnapshot {
            category: "文档".to_string(),
            size: 600_000,
            file_count: 50,
        }];

        let report = TrendService::build_trend_report(&[snap], &[cats], &[vec![]]);

        assert_eq!(report.path, "/test");
        assert_eq!(report.snapshots.len(), 1);
        assert_eq!(report.size_trend.len(), 1);
        assert_eq!(report.size_trend[0].total_size, 1_000_000);
        // 单次快照无增长数据
        assert!(report.top_growing.is_empty());
        // 分类趋势
        assert_eq!(report.category_trends.len(), 1);
        assert_eq!(report.category_trends[0].category, "文档");
        assert_eq!(report.category_trends[0].size_history.len(), 1);
    }

    #[test]
    fn build_trend_report_multiple_snapshots() {
        let snap1 =
            ScanSnapshot::new(&PathBuf::from("/test"), 50, 5, 500_000, 300);
        let snap2 =
            ScanSnapshot::new(&PathBuf::from("/test"), 100, 10, 1_000_000, 500);

        let cats1 = vec![
            CategorySnapshot {
                category: "文档".to_string(),
                size: 300_000,
                file_count: 30,
            },
        ];
        let cats2 = vec![
            CategorySnapshot {
                category: "文档".to_string(),
                size: 600_000,
                file_count: 60,
            },
            CategorySnapshot {
                category: "视频".to_string(),
                size: 400_000,
                file_count: 5,
            },
        ];

        let dirs1 = vec![DirSizeSnapshot {
            relative_path: "src".to_string(),
            size: 300_000,
        }];
        let dirs2 = vec![
            DirSizeSnapshot {
                relative_path: "src".to_string(),
                size: 600_000,
            },
            DirSizeSnapshot {
                relative_path: "assets".to_string(),
                size: 400_000,
            },
        ];

        let report =
            TrendService::build_trend_report(&[snap1, snap2], &[cats1, cats2], &[dirs1, dirs2]);

        // 总趋势
        assert_eq!(report.size_trend.len(), 2);
        assert_eq!(report.size_trend[0].total_size, 500_000);
        assert_eq!(report.size_trend[1].total_size, 1_000_000);

        // 增长数据：assets 新增 400K，src 增长 300K，降序
        assert_eq!(report.top_growing.len(), 2);
        assert_eq!(report.top_growing[0].name, "assets");
        assert_eq!(report.top_growing[0].growth_bytes, 400_000);
        assert_eq!(report.top_growing[1].name, "src");
        assert_eq!(report.top_growing[1].growth_bytes, 300_000);

        // 分类趋势聚合
        assert_eq!(report.category_trends.len(), 2);
        let doc_trend = report
            .category_trends
            .iter()
            .find(|t| t.category == "文档")
            .expect("应有文档趋势");
        assert_eq!(doc_trend.size_history.len(), 2);
        assert_eq!(doc_trend.size_history[0].total_size, 300_000);
        assert_eq!(doc_trend.size_history[1].total_size, 600_000);
    }

    #[test]
    fn build_trend_report_category_trend_merge() {
        // 测试同一个分类在多张快照中出现时的趋势合并
        let snap1 =
            ScanSnapshot::new(&PathBuf::from("/test"), 10, 1, 1000, 100);
        let snap2 =
            ScanSnapshot::new(&PathBuf::from("/test"), 20, 2, 2000, 200);
        let snap3 =
            ScanSnapshot::new(&PathBuf::from("/test"), 30, 3, 3000, 300);

        let cats1 = vec![CategorySnapshot {
            category: "图片".to_string(),
            size: 1000,
            file_count: 10,
        }];
        let cats2 = vec![CategorySnapshot {
            category: "图片".to_string(),
            size: 1500,
            file_count: 15,
        }];
        let cats3 = vec![CategorySnapshot {
            category: "图片".to_string(),
            size: 2000,
            file_count: 20,
        }];

        let report =
            TrendService::build_trend_report(&[snap1, snap2, snap3], &[cats1, cats2, cats3], &[vec![], vec![], vec![]]);

        let img_trend = report
            .category_trends
            .iter()
            .find(|t| t.category == "图片")
            .expect("应有图片趋势");
        assert_eq!(img_trend.size_history.len(), 3);
        assert_eq!(img_trend.size_history[0].total_size, 1000);
        assert_eq!(img_trend.size_history[1].total_size, 1500);
        assert_eq!(img_trend.size_history[2].total_size, 2000);
    }
}
