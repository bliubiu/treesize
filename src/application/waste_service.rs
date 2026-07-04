//! 空间浪费检测服务
//!
//! 检测多种类型的空间浪费，帮助用户识别可清理的文件和目录。
//!
//! ## 检测类型
//!
//! | 类型 | 说明 |
//! |------|------|
//! | 空目录 | 无任何文件或子目录的目录 |
//! | 零字节文件 | 大小为 0 的文件 |
//! | 临时文件 | .tmp, .temp, .bak 等临时文件 |
//! | 过期文件 | 超过指定天数未修改的文件 |
//! | 日志文件 | .log 文件，可能占用大量空间 |
//! | 缓存文件 | node_modules, .cache, __pycache__ 等 |
//! | 重复文件 | 内容完全相同的文件（复用 DuplicateService）|
//! | 长路径文件 | 路径深度过深的文件 |
//! | 锁定文件 | .lock 文件 |
//! | 冗余归档 | 压缩包与解压内容同时存在 |
//! | 系统残留 | 软件卸载后遗留的文件 |

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::domain::file_node::FileNode;
use crate::domain::value_objects::ByteSize;
use crate::domain::waste_type::WasteType;

// ─── 配置 ──────────────────────────────────────────────────────────────────

/// 空间浪费检测配置
#[derive(Debug, Clone)]
pub struct WasteConfig {
    /// 过期文件阈值（天数），默认 365 天
    pub stale_days: u32,
    /// 长路径阈值（深度），默认 8 层
    pub deep_path_threshold: usize,
    /// 是否检测重复文件（耗时较长），默认 false
    pub detect_duplicates: bool,
    /// 重复文件最小大小（字节），默认 1KB
    pub duplicate_min_size: u64,
}

impl Default for WasteConfig {
    fn default() -> Self {
        Self {
            stale_days: 365,
            deep_path_threshold: 8,
            detect_duplicates: false,
            duplicate_min_size: 1024,
        }
    }
}

// ─── 输出类型 ──────────────────────────────────────────────────────────────

/// 空间浪费项
#[derive(Debug, Serialize)]
pub struct WasteItem {
    /// 浪费类型
    pub waste_type: WasteType,
    /// 文件/目录路径
    pub path: PathBuf,
    /// 占用空间（字节）
    pub size: ByteSize,
    /// 额外说明（如过期天数、重复组数等）
    pub note: Option<String>,
}

/// 按类型分组的浪费统计
#[derive(Debug, Serialize)]
pub struct WasteTypeSummary {
    pub waste_type: WasteType,
    pub count: u64,
    pub total_size: ByteSize,
}

/// 空间浪费检测报告
#[derive(Debug, Serialize, Default)]
pub struct WasteReport {
    /// 所有浪费项
    pub items: Vec<WasteItem>,
    /// 按类型分组的统计
    pub by_type: Vec<WasteTypeSummary>,
    /// 总浪费空间
    pub total_wasted: ByteSize,
    /// 可自动清理的空间
    pub auto_cleanable: ByteSize,
}

// ─── 服务 ──────────────────────────────────────────────────────────────────

/// 空间浪费检测服务
pub struct WasteService;

impl WasteService {
    /// 扫描空间浪费（使用默认配置）
    pub fn scan(root: &FileNode) -> WasteReport {
        Self::scan_with_config(root, &WasteConfig::default())
    }

    /// 扫描空间浪费（自定义配置）
    pub fn scan_with_config(root: &FileNode, config: &WasteConfig) -> WasteReport {
        let mut items: Vec<WasteItem> = Vec::new();

        // 收集所有节点用于分析
        let all_nodes: Vec<&FileNode> = root.iter_all().collect();

        // 1. 空目录检测
        items.extend(Self::detect_empty_directories(&all_nodes));

        // 2. 零字节文件检测
        items.extend(Self::detect_zero_byte_files(&all_nodes));

        // 3. 临时文件检测
        items.extend(Self::detect_temporary_files(&all_nodes));

        // 4. 过期文件检测
        items.extend(Self::detect_stale_files(&all_nodes, config.stale_days));

        // 5. 日志文件检测
        items.extend(Self::detect_log_files(&all_nodes));

        // 6. 缓存文件检测
        items.extend(Self::detect_cache_files(&all_nodes));

        // 7. 长路径文件检测
        items.extend(Self::detect_deep_path_files(&all_nodes, config.deep_path_threshold));

        // 8. 锁定文件检测
        items.extend(Self::detect_lock_files(&all_nodes));

        // 9. 冗余归档检测
        items.extend(Self::detect_redundant_archives(&all_nodes));

        // 10. 系统残留检测
        items.extend(Self::detect_system_residues(&all_nodes));

        // 11. 重复文件检测（可选，耗时较长）
        if config.detect_duplicates {
            items.extend(Self::detect_duplicate_files(root, config.duplicate_min_size));
        }

        // 构建报告
        Self::build_report(items)
    }

    /// 检测空目录
    fn detect_empty_directories(nodes: &[&FileNode]) -> Vec<WasteItem> {
        nodes
            .iter()
            .filter(|n| n.is_dir() && n.children.is_empty())
            .map(|n| WasteItem {
                waste_type: WasteType::EmptyDirectory,
                path: n.path.clone(),
                size: ByteSize(0),
                note: None,
            })
            .collect()
    }

    /// 检测零字节文件
    fn detect_zero_byte_files(nodes: &[&FileNode]) -> Vec<WasteItem> {
        nodes
            .iter()
            .filter(|n| n.is_file() && n.size.0 == 0)
            .map(|n| WasteItem {
                waste_type: WasteType::ZeroByteFile,
                path: n.path.clone(),
                size: ByteSize(0),
                note: None,
            })
            .collect()
    }

    /// 检测临时文件
    fn detect_temporary_files(nodes: &[&FileNode]) -> Vec<WasteItem> {
        let temp_extensions = [
            "tmp", "temp", "bak", "old", "orig", "swp", "swo",
        ];

        nodes
            .iter()
            .filter(|n| {
                n.is_file() && temp_extensions.iter().any(|ext| n.extension == *ext)
            })
            .map(|n| WasteItem {
                waste_type: WasteType::TemporaryFile,
                path: n.path.clone(),
                size: n.size,
                note: None,
            })
            .collect()
    }

    /// 检测过期文件
    fn detect_stale_files(nodes: &[&FileNode], stale_days: u32) -> Vec<WasteItem> {
        let now = SystemTime::now();
        // 使用 saturating_mul 避免整数溢出，限制最大值为 100 年
        let max_days = 365 * 100; // 100 年
        let safe_days = stale_days.min(max_days);
        let threshold = Duration::from_secs(safe_days as u64 * 24 * 60 * 60);

        nodes
            .iter()
            .filter(|n| {
                n.is_file() && n.modified.map_or(false, |m| {
                    now.duration_since(m).unwrap_or(Duration::ZERO) > threshold
                })
            })
            .map(|n| {
                let days = n.modified.and_then(|m| {
                    now.duration_since(m).ok().map(|d| d.as_secs() / 86400)
                }).unwrap_or(0);

                WasteItem {
                    waste_type: WasteType::StaleFile,
                    path: n.path.clone(),
                    size: n.size,
                    note: Some(format!("{} 天未修改", days)),
                }
            })
            .collect()
    }

    /// 检测日志文件
    fn detect_log_files(nodes: &[&FileNode]) -> Vec<WasteItem> {
        nodes
            .iter()
            .filter(|n| n.is_file() && n.extension == "log")
            .map(|n| WasteItem {
                waste_type: WasteType::LogFile,
                path: n.path.clone(),
                size: n.size,
                note: None,
            })
            .collect()
    }

    /// 检测缓存文件
    fn detect_cache_files(nodes: &[&FileNode]) -> Vec<WasteItem> {
        // 缓存目录精确匹配列表
        let cache_dir_names = [
            "node_modules", "__pycache__", ".cache", ".npm", ".yarn",
            "target", "build", "dist", ".gradle", ".m2",
            ".cargo", ".rustup", ".conda", ".venv", "venv",
        ];

        nodes
            .iter()
            .filter(|n| {
                n.is_dir() && cache_dir_names.iter().any(|name| n.name == *name)
            })
            .map(|n| WasteItem {
                waste_type: WasteType::CacheFile,
                path: n.path.clone(),
                size: n.size,
                note: Some("缓存目录".to_string()),
            })
            .collect()
    }

    /// 检测长路径文件
    fn detect_deep_path_files(nodes: &[&FileNode], threshold: usize) -> Vec<WasteItem> {
        nodes
            .iter()
            .filter(|n| {
                let depth = n.path.components().count();
                depth > threshold
            })
            .map(|n| {
                let depth = n.path.components().count();
                WasteItem {
                    waste_type: WasteType::DeepPathFile,
                    path: n.path.clone(),
                    size: n.size,
                    note: Some(format!("路径深度 {} 层", depth)),
                }
            })
            .collect()
    }

    /// 检测锁定文件
    fn detect_lock_files(nodes: &[&FileNode]) -> Vec<WasteItem> {
        nodes
            .iter()
            .filter(|n| n.is_file() && n.extension == "lock")
            .map(|n| WasteItem {
                waste_type: WasteType::LockFile,
                path: n.path.clone(),
                size: n.size,
                note: None,
            })
            .collect()
    }

    /// 检测冗余归档
    fn detect_redundant_archives(nodes: &[&FileNode]) -> Vec<WasteItem> {
        let archive_extensions = ["zip", "rar", "7z", "tar", "gz", "bz2", "xz"];

        // 收集所有归档文件
        let archives: Vec<&&FileNode> = nodes
            .iter()
            .filter(|n| n.is_file() && archive_extensions.iter().any(|ext| n.extension == *ext))
            .collect();

        let mut items = Vec::new();

        for archive in archives {
            // 检查是否存在同名目录（可能是解压后的内容）
            let archive_stem = archive.path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| {
                    // 处理 .tar.gz 等双扩展名
                    if s.ends_with(".tar") {
                        s.trim_end_matches(".tar")
                    } else {
                        s
                    }
                });

            if let Some(stem) = archive_stem {
                let parent = archive.path.parent().unwrap_or(Path::new(""));
                let extracted_dir = parent.join(stem);

                // 检查是否存在同名目录
                if nodes.iter().any(|n| n.is_dir() && n.path == extracted_dir) {
                    items.push(WasteItem {
                        waste_type: WasteType::RedundantArchive,
                        path: archive.path.clone(),
                        size: archive.size,
                        note: Some(format!("可能存在解压目录: {}", extracted_dir.display())),
                    });
                }
            }
        }

        items
    }

    /// 检测系统残留
    fn detect_system_residues(nodes: &[&FileNode]) -> Vec<WasteItem> {
        let residue_patterns = [
            ".uninstall", ".installer", "unins",
            ".backup", ".migrate", ".upgrade",
        ];

        nodes
            .iter()
            .filter(|n| {
                let name_lower = n.name.to_lowercase();
                residue_patterns.iter().any(|p| name_lower.contains(p))
            })
            .map(|n| WasteItem {
                waste_type: WasteType::SystemResidue,
                path: n.path.clone(),
                size: n.size,
                note: Some("可能的系统残留文件".to_string()),
            })
            .collect()
    }

    /// 检测重复文件（复用 DuplicateService）
    fn detect_duplicate_files(root: &FileNode, min_size: u64) -> Vec<WasteItem> {
        use crate::application::DuplicateService;

        let report = DuplicateService::scan(root, min_size);
        let mut items = Vec::new();

        for group in report.groups {
            // 每组中保留第一个，其余标记为重复
            for path in group.paths.iter().skip(1) {
                items.push(WasteItem {
                    waste_type: WasteType::DuplicateFile,
                    path: path.clone(),
                    size: group.size,
                    note: Some(format!("共 {} 个重复", group.paths.len())),
                });
            }
        }

        items
    }

    /// 构建报告
    fn build_report(mut items: Vec<WasteItem>) -> WasteReport {
        // 按类型分组统计
        let mut by_type_map: HashMap<WasteType, (u64, u64)> = HashMap::new();
        let mut auto_cleanable: u64 = 0;

        for item in &items {
            let entry = by_type_map.entry(item.waste_type).or_insert((0, 0));
            entry.0 += 1;
            entry.1 = entry.1.saturating_add(item.size.0);

            if item.waste_type.safe_to_auto_clean() {
                auto_cleanable = auto_cleanable.saturating_add(item.size.0);
            }
        }

        let mut by_type: Vec<WasteTypeSummary> = by_type_map
            .into_iter()
            .map(|(waste_type, (count, total_size))| WasteTypeSummary {
                waste_type,
                count,
                total_size: ByteSize(total_size),
            })
            .collect();

        // 按总大小降序排序
        by_type.sort_by(|a, b| b.total_size.cmp(&a.total_size));

        // 按类型优先级和大小排序 items
        items.sort_by(|a, b| {
            b.waste_type.cleanup_priority()
                .cmp(&a.waste_type.cleanup_priority())
                .then_with(|| b.size.cmp(&a.size))
        });

        let total_wasted: u64 = items.iter().map(|i| i.size.0).sum();

        WasteReport {
            items,
            by_type,
            total_wasted: ByteSize(total_wasted),
            auto_cleanable: ByteSize(auto_cleanable),
        }
    }
}

// ─── 测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detect_empty_directories() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::create_dir(root.join("empty_dir")).unwrap();
        fs::write(root.join("file.txt"), "content").unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = WasteService::scan(&tree);

        let empty_dirs: Vec<_> = report.items
            .iter()
            .filter(|i| i.waste_type == WasteType::EmptyDirectory)
            .collect();

        assert_eq!(empty_dirs.len(), 1);
    }

    #[test]
    fn detect_zero_byte_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("empty.txt"), "").unwrap();
        fs::write(root.join("normal.txt"), "content").unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = WasteService::scan(&tree);

        let zero_files: Vec<_> = report.items
            .iter()
            .filter(|i| i.waste_type == WasteType::ZeroByteFile)
            .collect();

        assert_eq!(zero_files.len(), 1);
    }

    #[test]
    fn detect_temporary_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("file.tmp"), "temp").unwrap();
        fs::write(root.join("file.bak"), "backup").unwrap();
        fs::write(root.join("normal.txt"), "content").unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = WasteService::scan(&tree);

        let temp_files: Vec<_> = report.items
            .iter()
            .filter(|i| i.waste_type == WasteType::TemporaryFile)
            .collect();

        assert_eq!(temp_files.len(), 2);
    }

    #[test]
    fn detect_log_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("app.log"), "log content").unwrap();
        fs::write(root.join("normal.txt"), "content").unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = WasteService::scan(&tree);

        let log_files: Vec<_> = report.items
            .iter()
            .filter(|i| i.waste_type == WasteType::LogFile)
            .collect();

        assert_eq!(log_files.len(), 1);
    }

    #[test]
    fn detect_cache_directories() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/package.json"), "{}").unwrap();
        fs::create_dir(root.join(".cache")).unwrap();
        fs::write(root.join(".cache/data"), "cache").unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = WasteService::scan(&tree);

        let cache_dirs: Vec<_> = report.items
            .iter()
            .filter(|i| i.waste_type == WasteType::CacheFile)
            .collect();

        assert_eq!(cache_dirs.len(), 2);
    }

    #[test]
    fn detect_lock_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("package.lock"), "lock").unwrap();
        fs::write(root.join("normal.txt"), "content").unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = WasteService::scan(&tree);

        let lock_files: Vec<_> = report.items
            .iter()
            .filter(|i| i.waste_type == WasteType::LockFile)
            .collect();

        assert_eq!(lock_files.len(), 1);
    }
}
