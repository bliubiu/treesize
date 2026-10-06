//! USN Journal 增量扫描引擎（仅 Windows NTFS）
//!
//! # 设计原理
//!
//! USN Journal 记录文件系统的变更，每条记录包含文件名、父目录引用和变更类型，
//! 但**不包含文件大小**。本引擎通过以下两步完成扫描：
//!
//! 1. **MFT 读取阶段**：读取卷的 MFT（Master File Table），
//!    解析所有文件的 `$DATA` 属性获取文件大小，构建 记录号→大小 映射表。
//!
//! 2. **USN 读取阶段**：从 USN Journal 读取文件变更记录，
//!    利用父引用重建目录树，并从 MFT 映射表中获取文件大小。
//!
//! ## 增量扫描
//!
//! 首次扫描后记录当前 NextUsn 到持久化存储。
//! 后续扫描从上次记录的 NextUsn 开始读取新增/变更的记录，
//! 避免重复扫描未变化的文件。USN Journal 被重置时自动降级为全量扫描。

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::domain::error::{DomainError, Result};
use crate::domain::file_node::FileNode;
use crate::domain::scan_engine::{CancelToken, ScanEngine, ScanOptions, ScanProgress};

use super::ntfs_reader::{
    build_size_map, get_volume_info, parse_mft_records, path_to_volume_device, query_usn_journal, read_mft_raw,
    read_usn_records, UsnRecord, VolumeHandle,
};

// ─── USN 扫描引擎 ──────────────────────────────────────────────────────────

/// USN Journal 增量扫描引擎
///
/// # 工作模式
///
/// 1. **全量扫描**（首次或 `--incremental` 未指定）：
///    - 读取全部 USN Journal 记录 + MFT 元数据
///    - 保存 NextUsn 以便后续增量
///
/// 2. **增量扫描**（`--incremental` 指定且有有效状态）：
///    - 从上次保存的 NextUsn 开始读取新记录
///    - 结合 MFT 元数据补充文件大小
///    - 更新 NextUsn
///
/// 3. **Journal 重置降级**：
///    - 检测到 Journal ID 变化时自动降级为全量扫描
pub struct UsnScanEngine;

impl UsnScanEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UsnScanEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanEngine for UsnScanEngine {
    fn scan(
        &self,
        root: PathBuf,
        options: &ScanOptions,
        progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
        cancel: Option<&CancelToken>,
    ) -> Result<FileNode> {
        let device = path_to_volume_device(&root);
        let handle =
            VolumeHandle::open(Path::new(&device)).map_err(|e| DomainError::ScanFailed(format!("打开卷失败：{e}")))?;

        // 句柄由 VolumeHandle 的 Drop 自动关闭
        do_usn_scan(&handle, root, options, progress, cancel)
    }
}

// ─── 增量扫描持久化状态 ─────────────────────────────────────────────────────

/// USN 增量扫描持久化状态（JSON 文件存储）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UsnIncrementalState {
    /// 卷标识（如 "C:"）
    pub volume: String,
    /// USN Journal ID（用于检测 Journal 重置）
    pub journal_id: u64,
    /// 上次扫描的 NextUsn（下次从此开始增量）
    pub next_usn: i64,
    /// 上次扫描时间
    pub scanned_at: String,
}

impl UsnIncrementalState {
    /// 加载指定卷的增量状态
    fn load(volume: &str) -> Option<Self> {
        let path = Self::state_path(volume);
        if !path.exists() {
            return None;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).ok(),
            Err(_) => None,
        }
    }

    /// 保存增量状态到磁盘
    fn save(&self) {
        let path = Self::state_path(&self.volume);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(content) => {
                if let Err(e) = std::fs::write(&path, &content) {
                    tracing::warn!(
                        target: "treesize::usn",
                        "保存 USN 增量状态失败：{e}",
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    target: "treesize::usn",
                    "序列化 USN 增量状态失败：{e}",
                );
            },
        }
    }

    /// 获取状态文件的路径
    fn state_path(volume: &str) -> PathBuf {
        let safe_vol = volume.replace(':', "");
        let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("treesize");
        base.join(format!("usn_state_{safe_vol}.json"))
    }
}

// ─── 主扫描逻辑 ────────────────────────────────────────────────────────────

/// USN 扫描内部实现（6 阶段流水线）
fn do_usn_scan(
    handle: &VolumeHandle,
    root: PathBuf,
    options: &ScanOptions,
    progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
    cancel: Option<&CancelToken>,
) -> Result<FileNode> {
    let start = SystemTime::now();
    let volume_letter = extract_volume_letter(&root);

    // ═══════════════════════════════════════════════════════════════════
    // Phase 1: 查询 USN Journal 状态
    // ═══════════════════════════════════════════════════════════════════
    let journal_state =
        query_usn_journal(handle).map_err(|e| DomainError::ScanFailed(format!("查询 USN Journal 失败：{e}")))?;

    tracing::info!(
        target: "treesize::usn",
        "USN Journal：ID={}, NextUsn={}, LowestUsn={}, MaxUsn={}",
        journal_state.usn_journal_id,
        journal_state.next_usn,
        journal_state.lowest_valid_usn,
        journal_state.max_usn,
    );

    if let Some(c) = cancel {
        if c.is_cancelled() {
            return Err(DomainError::ScanCancelled);
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 2: 读取 MFT 构建文件大小映射表
    //
    // USN Journal 记录不包含文件大小信息，需要通过 MFT 补充查询。
    // 读取 MFT 全量数据，解析后构建 记录号→大小 的 HashMap。
    // ═══════════════════════════════════════════════════════════════════
    tracing::info!(target: "treesize::usn", "Phase 2: 开始读取 MFT 获取文件大小...");

    let volume_info = get_volume_info(handle).map_err(|e| DomainError::ScanFailed(format!("获取卷信息失败：{e}")))?;

    let max_mft_records =
        100_000_000u64.min((volume_info.mft_valid_data_length.max(0) as u64) / volume_info.bytes_per_record as u64);

    let mut raw_mft = read_mft_raw(handle, &volume_info, max_mft_records)
        .map_err(|e| DomainError::ScanFailed(format!("读取 MFT 失败：{e}")))?;

    let parsed_mft = parse_mft_records(&mut raw_mft, volume_info.bytes_per_record)
        .map_err(|e| DomainError::ScanFailed(format!("解析 MFT 失败：{e}")))?;

    // 构建 记录号→文件大小 映射表
    let size_map = build_size_map(&parsed_mft);

    let mft_record_count = parsed_mft.len();
    drop(parsed_mft); // 尽早释放 MFT 内存

    tracing::info!(
        target: "treesize::usn",
        "Phase 2 完成：有效 MFT 记录 {} 条，大小映射 {} 条",
        mft_record_count,
        size_map.len(),
    );

    if let Some(c) = cancel {
        if c.is_cancelled() {
            return Err(DomainError::ScanCancelled);
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 3: 确定增量起始 USN
    //
    // 增量模式：先从持久化存储加载上次的 NextUsn，
    // 验证 Journal ID 匹配后从该点开始增量读取。
    // Journal 被重置时自动降级为全量扫描。
    // ═══════════════════════════════════════════════════════════════════
    let (since_usn, is_incremental_mode) = determine_start_usn(options, &journal_state, &volume_letter);

    // ═══════════════════════════════════════════════════════════════════
    // Phase 4: 读取 USN 记录
    // ═══════════════════════════════════════════════════════════════════
    let read_limit: u32 = 100_000;
    let usn_records = read_usn_records(handle, journal_state.usn_journal_id, since_usn, read_limit)
        .map_err(|e| DomainError::ScanFailed(format!("读取 USN 记录失败：{e}")))?;

    tracing::info!(
        target: "treesize::usn",
        "Phase 4 完成：读取 USN 记录 {} 条（从 USN {} 开始）",
        usn_records.len(),
        since_usn,
    );

    if let Some(c) = cancel {
        if c.is_cancelled() {
            return Err(DomainError::ScanCancelled);
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 5: 使用 USN + MFT 数据构建文件树
    //
    // 利用 USN 的父引用重建目录层次结构，从 MFT 大小映射获取真实文件大小。
    // ═══════════════════════════════════════════════════════════════════
    let root_node = build_tree_from_usn(&usn_records, &size_map, &root, options, progress, cancel)?;

    let elapsed_ms = SystemTime::now()
        .duration_since(start)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    tracing::info!(
        target: "treesize::usn",
        "Phase 5 完成：文件 {} 个，目录 {} 个，总大小 {}",
        root_node.file_count,
        root_node.dir_count,
        root_node.size,
    );

    // ═══════════════════════════════════════════════════════════════════
    // Phase 6: 保存增量扫描状态
    //
    // 全量扫描也保存状态，以便下次运行时支持增量。
    // Journal ID 不匹配的降级扫描也会覆盖旧状态。
    // ═══════════════════════════════════════════════════════════════════
    let new_state = UsnIncrementalState {
        volume: volume_letter.clone(),
        journal_id: journal_state.usn_journal_id,
        next_usn: journal_state.next_usn,
        scanned_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    new_state.save();

    tracing::info!(
        target: "treesize::usn",
        "USN 扫描完成：耗时 {} ms，增量模式={}，NextUsn={}",
        elapsed_ms,
        is_incremental_mode,
        new_state.next_usn,
    );

    Ok(root_node)
}

/// 确定增量扫描的起始 USN
///
/// 返回 (起始 USN, 是否为真正增量模式)
fn determine_start_usn(
    options: &ScanOptions,
    journal_state: &crate::infrastructure::ntfs_reader::UsnJournalState,
    volume_letter: &str,
) -> (i64, bool) {
    if !options.incremental {
        return (journal_state.lowest_valid_usn, false);
    }

    // 尝试加载增量状态
    let saved_state = match UsnIncrementalState::load(volume_letter) {
        Some(s) => s,
        None => {
            tracing::info!(
                target: "treesize::usn",
                "首次扫描，无增量状态，执行全量扫描",
            );
            return (journal_state.lowest_valid_usn, false);
        },
    };

    // 验证 Journal ID 是否匹配
    if saved_state.journal_id != journal_state.usn_journal_id {
        tracing::warn!(
            target: "treesize::usn",
            "USN Journal ID 不匹配（上次={}, 当前={})，Journal 可能已被重置，降级为全量扫描",
            saved_state.journal_id,
            journal_state.usn_journal_id,
        );
        return (journal_state.lowest_valid_usn, false);
    }

    // Journal ID 匹配，执行真正的增量扫描
    let next_usn = saved_state.next_usn.max(journal_state.lowest_valid_usn);
    tracing::info!(
        target: "treesize::usn",
        "增量扫描模式：从 USN {} 开始读取（上次扫描时间：{}）",
        next_usn,
        saved_state.scanned_at,
    );
    (next_usn, true)
}

/// 提取卷盘符（如 "C:"）
fn extract_volume_letter(path: &Path) -> String {
    let s = path.to_string_lossy();
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        format!("{}:", &s[..1])
    } else {
        "C:".to_string()
    }
}

// ─── 文件树构建（核心）─────────────────────────────────────────────────────

/// 从 USN 记录 + MFT 大小映射构建完整的 FileNode 树
///
/// # 算法
///
/// 1. 用 USN 记录构建 parent_reference → [child_record_number] 的索引
/// 2. 从 MFT 根（记录号 5）开始递归遍历
/// 3. 目录节点从 USN 记录获取子节点列表，文件大小从 MFT size_map 获取
///
/// 此方法与 `mft_scanner::rebuild_tree` 类似，但用 USN 记录替代 MFT 记录
/// 作为目录结构的来源，MFT 仅用于文件大小补充。
fn build_tree_from_usn(
    records: &[UsnRecord],
    size_map: &HashMap<u64, u64>,
    root_path: &Path,
    _options: &ScanOptions,
    _progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
    _cancel: Option<&CancelToken>,
) -> Result<FileNode> {
    // ── 步骤 1：用 USN 记录构建 parent → children 索引 ──
    // 同一条记录号可能出现多次（多次修改），只保留最新
    let mut best_entries: HashMap<u64, &UsnRecord> = HashMap::new();
    for rec in records {
        let file_ref = rec.file_reference_number & 0x0000_FFFF_FFFF_FFFF;
        best_entries.insert(file_ref, rec);
    }

    // 构建 parent_record → [child_record_number] 索引
    let mut parent_index: HashMap<u64, Vec<u64>> = HashMap::new();
    for (&file_ref, rec) in &best_entries {
        let parent_ref = rec.parent_file_reference_number & 0x0000_FFFF_FFFF_FFFF;
        parent_index.entry(parent_ref).or_default().push(file_ref);
    }

    // ── 步骤 2：从根（记录号 5）递归构建树 ──
    fn build_recursive(
        record_number: u64,
        parent_index: &HashMap<u64, Vec<u64>>,
        best_entries: &HashMap<u64, &UsnRecord>,
        size_map: &HashMap<u64, u64>,
        current_path: &Path,
    ) -> Option<FileNode> {
        if record_number == 5 {
            // MFT 根目录
            let mut node = FileNode::new_dir(current_path.to_path_buf(), None);
            if let Some(child_refs) = parent_index.get(&record_number) {
                let mut child_nodes: Vec<FileNode> = Vec::with_capacity(child_refs.len());
                for &child_ref in child_refs {
                    if let Some(child_rec) = best_entries.get(&child_ref) {
                        let child_path = current_path.join(&child_rec.file_name);

                        if child_rec.is_directory {
                            if let Some(child_node) =
                                build_recursive(child_ref, parent_index, best_entries, size_map, &child_path)
                            {
                                child_nodes.push(child_node);
                            }
                        } else {
                            let size = size_map.get(&child_ref).copied().unwrap_or(0);
                            child_nodes.push(FileNode::new_file(child_path, size, None));
                        }
                    }
                }
                node.children = child_nodes;
            }
            Some(node)
        } else {
            // 非根节点：必须是 USN 中存在的目录
            let rec = best_entries.get(&record_number)?;
            if !rec.is_directory {
                return None;
            }

            let mut node = FileNode::new_dir(current_path.to_path_buf(), None);
            if let Some(child_refs) = parent_index.get(&record_number) {
                let mut child_nodes: Vec<FileNode> = Vec::with_capacity(child_refs.len());
                for &child_ref in child_refs {
                    if let Some(child_rec) = best_entries.get(&child_ref) {
                        let child_path = current_path.join(&child_rec.file_name);

                        if child_rec.is_directory {
                            if let Some(child_node) =
                                build_recursive(child_ref, parent_index, best_entries, size_map, &child_path)
                            {
                                child_nodes.push(child_node);
                            }
                        } else {
                            let size = size_map.get(&child_ref).copied().unwrap_or(0);
                            child_nodes.push(FileNode::new_file(child_path, size, None));
                        }
                    }
                }
                node.children = child_nodes;
            }
            Some(node)
        }
    }

    let mut root_node = stacker::maybe_grow(64 * 1024, 256 * 1024 * 1024, || {
        build_recursive(5, &parent_index, &best_entries, size_map, root_path)
    })
    .unwrap_or_else(|| FileNode::new_dir(root_path.to_path_buf(), None));

    root_node.aggregate();
    root_node.sort_by_size_desc();

    Ok(root_node)
}

// ─── 测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "需要管理员权限的 NTFS 卷和 USN Journal"]
    fn usn_query_current_drive() {
        let handle = VolumeHandle::open(Path::new(&path_to_volume_device(Path::new("C:\\")))).unwrap();
        let state = query_usn_journal(&handle);
        assert!(state.is_ok(), "USN 查询失败：{:?}", state.err());
        let s = state.unwrap();
        assert!(s.usn_journal_id != 0);
    }

    #[test]
    fn extract_volume_letter_works() {
        assert_eq!(extract_volume_letter(Path::new("C:\\Users")), "C:");
        assert_eq!(extract_volume_letter(Path::new("D:\\")), "D:");
        // 无盘符路径默认为 C:
        assert_eq!(extract_volume_letter(Path::new("\\\\server\\share")), "C:");
    }

    #[test]
    fn determine_start_usn_full_scan() {
        let opts = ScanOptions::default(); // incremental = false
        let state = crate::infrastructure::ntfs_reader::UsnJournalState {
            usn_journal_id: 123,
            next_usn: 5000,
            lowest_valid_usn: 1000,
            max_usn: 10000,
        };
        let (usn, is_inc) = determine_start_usn(&opts, &state, "C:");
        assert_eq!(usn, 1000);
        assert!(!is_inc);
    }
}
