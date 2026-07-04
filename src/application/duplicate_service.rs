//! 重复文件扫描服务（7 阶段去重流水线）
//!
//! ## 流水线阶段
//!
//! | 阶段 | 操作 | 输入 → 输出 | 作用 |
//! |------|------|-------------|------|
//! | 1 | 收集 | 文件树 → 平铺文件列表 | 规整化，过滤小于 `min_size` 的文件 |
//! | 2 | 大小分桶 | 文件列表 → 按大小分组 | 仅同大小文件可能重复 |
//! | 3 | 快速哈希 | 候选文件 → 头部哈希 | 读取头部 4KB，避免大文件全量 I/O |
//! | 4 | 快速去重 | 按(大小,头哈希)分组 | 头部不同的唯一文件提前淘汰 |
//! | 5 | 全量哈希 | 剩余候选 → blake3 全哈希 | 确认内容完全相同 |
//! | 6 | 全量去重 | 按(大小,全哈希)分组 | 最终确认重复文件组 |
//! | 7 | 报告 | 重复组 → DuplicateReport | 按浪费空间降序排序输出 |
//!
//! ## 优化说明
//!
//! - 小于 `head_size`（默认 4KB）的文件跳过阶段 3-4，直接计算全量哈希
//! - 每阶段输出数量已知，便于诊断性能瓶颈

use std::collections::HashMap;
use std::path::PathBuf;

use rayon::prelude::*;
use serde::Serialize;

use crate::domain::file_node::FileNode;
use crate::domain::value_objects::ByteSize;

// ─── 配置 ──────────────────────────────────────────────────────────────────

/// 重复文件扫描配置
#[derive(Debug, Clone, Copy)]
pub struct DuplicateConfig {
    /// 最小关注大小（字节），小于此值的文件忽略
    pub min_size: u64,
    /// 快速哈希读取的头部字节数（默认 4096）
    pub head_size: u64,
}

impl Default for DuplicateConfig {
    fn default() -> Self {
        Self {
            min_size: 1024,
            head_size: 4096,
        }
    }
}

// ─── 输出类型 ──────────────────────────────────────────────────────────────

/// 重复文件组
#[derive(Debug, Serialize)]
pub struct DuplicateGroup {
    /// 组内文件大小
    pub size: ByteSize,
    /// 文件内容哈希（blake3 十六进制）
    pub hash: String,
    /// 重复文件路径列表
    pub paths: Vec<PathBuf>,
    /// 浪费的空间（(n-1) * size）
    pub wasted: ByteSize,
}

/// 重复文件扫描报表
#[derive(Debug, Serialize, Default)]
pub struct DuplicateReport {
    pub groups: Vec<DuplicateGroup>,
    pub total_duplicate_files: u64,
    pub total_wasted: ByteSize,
}

// ─── 服务 ──────────────────────────────────────────────────────────────────

/// 重复文件扫描服务
pub struct DuplicateService;

impl DuplicateService {
    /// 扫描重复文件（使用默认配置）
    ///
    /// `min_size` 为最小关注大小（字节），小于此值的文件忽略。
    pub fn scan(root: &FileNode, min_size: u64) -> DuplicateReport {
        let config = DuplicateConfig {
            min_size,
            ..Default::default()
        };
        Self::scan_with_config(root, &config)
    }

    /// 扫描重复文件（自定义配置，7 阶段流水线）
    pub fn scan_with_config(root: &FileNode, config: &DuplicateConfig) -> DuplicateReport {
        let min_size = config.min_size;
        let head_size = config.head_size;

        // ═════════════════════════════════════════════════════════════════
        // Stage 1: 收集 — 遍历文件树，收集所有符合大小条件的文件
        // ═════════════════════════════════════════════════════════════════
        let files: Vec<(PathBuf, u64)> = root
            .iter_all()
            .filter(|n| n.is_file() && n.size.0 >= min_size)
            .map(|n| (n.path.clone(), n.size.0))
            .collect();

        let stage1_count = files.len();
        tracing::debug!(
            "Stage 1 [收集]: 收集 {} 个文件（min_size={})",
            stage1_count,
            min_size,
        );

        if files.is_empty() {
            return DuplicateReport::default();
        }

        // ═════════════════════════════════════════════════════════════════
        // Stage 2: 大小分桶 — 按精确字节数分组，仅保留大小相同的文件
        // ═════════════════════════════════════════════════════════════════
        let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
        for (path, size) in files {
            by_size.entry(size).or_default().push(path);
        }

        // 过滤出有 ≥2 个文件的桶（只有这些可能重复）
        let size_buckets: Vec<(u64, Vec<PathBuf>)> = by_size
            .into_iter()
            .filter(|(_, paths)| paths.len() >= 2)
            .collect();

        let stage2_candidates: usize = size_buckets.iter().map(|(_, p)| p.len()).sum();
        let stage2_buckets = size_buckets.len();
        tracing::debug!(
            "Stage 2 [大小分桶]: {} 个文件进入 {} 个桶（淘汰 {} 个唯一大小文件）",
            stage2_candidates,
            stage2_buckets,
            stage1_count.saturating_sub(stage2_candidates),
        );

        if size_buckets.is_empty() {
            return DuplicateReport::default();
        }

        // ═════════════════════════════════════════════════════════════════
        // Stage 3: 快速哈希 — 对每个候选文件计算头部哈希
        //
        // 小于 head_size 的文件直接计算全量哈希（标记为 has_full=true）
        // ═════════════════════════════════════════════════════════════════
        let quick_hash_results: Vec<((u64, String), PathBuf, bool)> = size_buckets
            .into_par_iter()
            .flat_map(|(size, paths)| {
                paths
                    .into_par_iter()
                    .filter_map(move |path| {
                        if size < head_size {
                            // 小文件：直接全量哈希，标记已完成
                            match compute_hash(&path) {
                                Ok(hash) => Some(((size, hash), path, true)),
                                Err(e) => {
                                    tracing::debug!(
                                        target: "treesize::duplicate",
                                        "无法计算文件哈希：{} - {}",
                                        path.display(),
                                        e
                                    );
                                    None
                                }
                            }
                        } else {
                            // 大文件：只读头部
                            match compute_head_hash(&path, head_size) {
                                Ok(hash) => Some(((size, hash), path, false)),
                                Err(e) => {
                                    tracing::debug!(
                                        target: "treesize::duplicate",
                                        "无法计算文件头部哈希：{} - {}",
                                        path.display(),
                                        e
                                    );
                                    None
                                }
                            }
                        }
                    })
            })
            .collect();

        let stage3_count = quick_hash_results.len();
        tracing::debug!(
            "Stage 3 [快速哈希]: 计算 {} 个头部哈希（head_size={})",
            stage3_count,
            head_size,
        );

        // ═════════════════════════════════════════════════════════════════
        // Stage 4: 快速去重 — 按 (大小, 头部哈希) 分组
        //
        // - 分组后仅有 1 个文件的组 → 头部唯一，直接淘汰
        // - 分组后 ≥2 个文件且全部标记 has_full → 全量哈希已确认，进入最终分组
        // - 分组后 ≥2 个文件且部分需要全量哈希 → 进入阶段 5
        // ═════════════════════════════════════════════════════════════════
        // 最终去重结果容器（key = (size, full_hash)）
        let mut full_groups: HashMap<(u64, String), Vec<PathBuf>> = HashMap::new();
        // 需要进一步全量哈希的候选
        let mut need_full: Vec<(PathBuf, u64)> = Vec::new();

        // 按 (size, quick_hash) 分组
        let mut by_quick: HashMap<(u64, String), Vec<(PathBuf, bool)>> = HashMap::new();
        for ((size, hash), path, has_full) in quick_hash_results {
            by_quick.entry((size, hash)).or_default().push((path, has_full));
        }

        let stage4_groups = by_quick.len();
        let mut stage4_passed = 0;

        for ((size, _hash), group) in by_quick {
            if group.len() < 2 {
                // 快速哈希唯一：淘汰
                continue;
            }
            // 检查是否所有文件都已拥有全量哈希（即小文件）
            let all_have_full = group.iter().all(|(_, has_full)| *has_full);
            stage4_passed += group.len();

            if all_have_full {
                // 小文件组：直接使用头部哈希（即全量哈希）
                full_groups
                    .entry((size, _hash))
                    .or_default()
                    .extend(group.into_iter().map(|(p, _)| p));
            } else {
                // 大文件组：需要进一步计算全量哈希
                need_full.extend(group.into_iter().map(|(p, _)| (p, size)));
            }
        }

        let stage4_eliminated = stage3_count.saturating_sub(stage4_passed);
        tracing::debug!(
            "Stage 4 [快速去重]: {} 个分组，通过 {} 个候选，淘汰 {} 个唯一头部文件",
            stage4_groups,
            stage4_passed,
            stage4_eliminated,
        );

        // ═════════════════════════════════════════════════════════════════
        // Stage 5: 全量哈希 — 对剩余候选计算完整的 blake3 哈希
        // ═════════════════════════════════════════════════════════════════
        let full_hash_results: Vec<((u64, String), PathBuf)> = need_full
            .into_par_iter()
            .filter_map(|(path, size)| {
                compute_hash(&path).ok().map(|hash| ((size, hash), path))
            })
            .collect();

        let stage5_count = full_hash_results.len();
        tracing::debug!("Stage 5 [全量哈希]: 计算 {} 个全量哈希", stage5_count);

        // ═════════════════════════════════════════════════════════════════
        // Stage 6: 全量去重 — 按 (大小, 全量哈希) 分组
        // ═════════════════════════════════════════════════════════════════
        for ((size, hash), path) in full_hash_results {
            full_groups.entry((size, hash)).or_default().push(path);
        }

        let stage6_groups = full_groups.len();
        let stage6_dup_groups = full_groups.iter().filter(|(_, p)| p.len() > 1).count();
        tracing::debug!(
            "Stage 6 [全量去重]: {} 个分组，其中 {} 组确认重复",
            stage6_groups,
            stage6_dup_groups,
        );

        // ═════════════════════════════════════════════════════════════════
        // Stage 7: 报告 — 构建 DuplicateReport，按浪费空间降序排序
        // ═════════════════════════════════════════════════════════════════
        let mut groups: Vec<DuplicateGroup> = full_groups
            .into_iter()
            .filter(|(_, paths)| paths.len() > 1)
            .map(|((size, hash), mut paths)| {
                paths.sort();
                let count = paths.len() as u64;
                let wasted = size.saturating_mul(count.saturating_sub(1));
                DuplicateGroup {
                    size: ByteSize(size),
                    hash,
                    paths,
                    wasted: ByteSize(wasted),
                }
            })
            .collect();

        groups.sort_by(|a, b| b.wasted.cmp(&a.wasted));

        let total_duplicate_files: u64 = groups.iter().map(|g| g.paths.len() as u64).sum();
        let total_wasted: u64 = groups.iter().map(|g| g.wasted.0).sum();

        let report = DuplicateReport {
            groups,
            total_duplicate_files,
            total_wasted: ByteSize(total_wasted),
        };

        tracing::info!(
            "Stage 7 [报告]: {} 组重复文件, {} 个文件, 浪费 {}",
            report.groups.len(),
            report.total_duplicate_files,
            report.total_wasted,
        );

        report
    }
}

// ─── 哈希计算 ──────────────────────────────────────────────────────────────

/// 计算文件头部哈希（读取前 `head_size` 字节）
fn compute_head_hash(path: &PathBuf, head_size: u64) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; head_size as usize];
    let n = file.read(&mut buf)?;
    if n > 0 {
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// 计算文件完整 blake3 哈希
fn compute_hash(path: &PathBuf) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

// ─── 测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn finds_duplicates() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("a.txt"), "hello world").unwrap();
        fs::write(root.join("b.txt"), "hello world").unwrap();
        fs::write(root.join("c.txt"), "different").unwrap();
        fs::write(root.join("d.txt"), "hello world").unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = DuplicateService::scan(&tree, 1);

        assert!(report.groups.len() >= 1);
        let group = report
            .groups
            .iter()
            .find(|g| g.paths.len() == 3)
            .expect("应找到 3 个重复文件");
        assert_eq!(group.size.0, 11);
        assert_eq!(group.wasted.0, 22); // (3-1) * 11
    }

    #[test]
    fn ignores_small_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("a.txt"), "ab").unwrap();
        fs::write(root.join("b.txt"), "ab").unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = DuplicateService::scan(&tree, 100);

        assert!(report.groups.is_empty());
    }

    #[test]
    fn no_duplicates() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("a.txt"), "aaa").unwrap();
        fs::write(root.join("b.txt"), "bbb").unwrap();
        fs::write(root.join("c.txt"), "ccc").unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = DuplicateService::scan(&tree, 1);

        assert!(report.groups.is_empty());
    }

    #[test]
    fn dedup_small_files_via_head_hash() {
        // 文件小于 head_size（4KB），应直接通过全量哈希路径完成去重
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("tiny_a.txt"), "A".repeat(100)).unwrap();
        fs::write(root.join("tiny_b.txt"), "A".repeat(100)).unwrap();
        fs::write(root.join("tiny_c.txt"), "B".repeat(100)).unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = DuplicateService::scan(&tree, 1);

        assert!(report.groups.len() >= 1);
        let group = report
            .groups
            .iter()
            .find(|g| g.paths.len() == 2)
            .expect("应找到 2 个重复的小文件");
        assert_eq!(group.size.0, 100);
    }

    #[test]
    fn dedup_large_files_uses_head_hash_filter() {
        // 创建一些足够大的文件（超过 head_size），验证快速哈希过滤
        let dir = tempdir().unwrap();
        let root = dir.path();

        // 8192 字节 > 默认 head_size (4096)
        let content_a = "X".repeat(8192);
        let content_b = "Y".repeat(8192);

        // 3 个相同的大文件（重复）
        fs::write(root.join("big_a.dat"), &content_a).unwrap();
        fs::write(root.join("big_b.dat"), &content_a).unwrap();
        fs::write(root.join("big_c.dat"), &content_a).unwrap();
        // 1 个不同的大文件（唯一）
        fs::write(root.join("big_d.dat"), &content_b).unwrap();

        let tree = crate::test_utils::build_tree_from_dir(root);
        let report = DuplicateService::scan(&tree, 1);

        assert!(report.groups.len() >= 1);
        let group = report
            .groups
            .iter()
            .find(|g| g.paths.len() == 3)
            .expect("应找到 3 个重复的大文件");
        assert_eq!(group.size.0, 8192);
        assert_eq!(group.wasted.0, 16384); // (3-1) * 8192
    }
}
