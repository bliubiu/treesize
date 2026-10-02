//! MFT 直接读取扫描引擎（仅 Windows NTFS）
//!
//! 绕过文件系统遍历，通过原始卷设备句柄直接读取 MFT，
//! 实现类似 WizTree 的极速扫描。

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::domain::error::{DomainError, Result};
use crate::domain::file_node::FileNode;
use crate::domain::scan_engine::{should_exclude_entry, CancelToken, ScanEngine, ScanOptions, ScanProgress};

use super::ntfs_reader::{
    build_tree, get_volume_info, parse_mft_records, path_to_volume_device, read_mft_raw, MftFileEntry, NtfsError,
    VolumeHandle,
};

/// MFT 直接读取扫描引擎
///
/// # 原理
///
/// 1. 打开卷设备句柄（`\\.\C:`，需管理员权限）
/// 2. 读 NTFS 引导扇区取 MFT 位置；`FSCTL_GET_NTFS_VOLUME_DATA` 取有效长度
/// 3. 从卷原始读取 MFT 数据
/// 4. 解析每条 `FILE_RECORD_HEADER` + 属性
/// 5. 通过 `$FILE_NAME` 属性和 `parent_reference` 重建目录树
/// 6. 回溯聚合大小和计数
pub struct MftScanEngine;

impl MftScanEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MftScanEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanEngine for MftScanEngine {
    fn scan(
        &self,
        root: PathBuf,
        options: &ScanOptions,
        progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
        cancel: Option<&CancelToken>,
    ) -> Result<FileNode> {
        // 打开卷设备并获取信息（MFT 直读必须用 \\.\X: 卷设备路径）
        let device = path_to_volume_device(&root);
        let handle =
            VolumeHandle::open(Path::new(&device)).map_err(|e| DomainError::ScanFailed(format!("打开卷失败：{e}")))?;

        // 句柄由 VolumeHandle 的 Drop 自动关闭
        do_mft_scan(&handle, root, options, progress, cancel)
    }
}

/// MFT 扫描内部实现（拆分出来确保句柄在错误路径也能关闭）
fn do_mft_scan(
    handle: &VolumeHandle,
    root: PathBuf,
    options: &ScanOptions,
    progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
    cancel: Option<&CancelToken>,
) -> Result<FileNode> {
    let start = SystemTime::now();

    let volume_info = get_volume_info(handle).map_err(|e| {
        tracing::error!(target: "treesize::mft", "获取卷信息失败：{e}");
        DomainError::ScanFailed(format!("获取卷信息失败：{e}"))
    })?;

    tracing::info!(
        target: "treesize::mft",
        "卷信息：MFT 偏移={:#x}，记录大小={}，扇区大小={}，每簇扇区={}，LCN信息=[{}]",
        volume_info.mft_byte_offset(),
        volume_info.bytes_per_record,
        volume_info.bytes_per_sector,
        volume_info.sectors_per_cluster,
        volume_info.debug_lcn(),
    );

    // 读取 MFT 原始数据
    let max_records =
        100_000_000u64.min((volume_info.mft_valid_data_length.max(0) as u64) / volume_info.bytes_per_record as u64);

    let mut raw_data = match read_mft_raw(handle, &volume_info, max_records) {
        Ok(data) => data,
        Err(e) => {
            let (category, detail) = classify_mft_error(&e);
            tracing::error!(
                target: "treesize::mft",
                "读取 MFT 失败: [分类={}] {}: {}",
                category, detail, volume_info.debug_lcn(),
            );
            return Err(DomainError::ScanFailed(format!("读取 MFT 失败（{category}）：{e}")));
        },
    };

    tracing::info!(
        target: "treesize::mft",
        "读取 MFT 数据：{} 字节，{} 条记录",
        raw_data.len(),
        raw_data.len() / volume_info.bytes_per_record as usize,
    );

    if let Some(c) = cancel {
        if c.is_cancelled() {
            return Err(DomainError::ScanCancelled);
        }
    }

    // 解析 MFT 记录
    let parsed_records = parse_mft_records(&mut raw_data, volume_info.bytes_per_record).map_err(|e| {
        tracing::error!(
            target: "treesize::mft",
            "解析 MFT 失败: raw_len={}, bpr={}, error={e}",
            raw_data.len(),
            volume_info.bytes_per_record,
        );
        DomainError::ScanFailed(format!("解析 MFT 失败：{e}"))
    })?;

    tracing::info!(
        target: "treesize::mft",
        "解析 MFT 记录：{} 条有效记录",
        parsed_records.len(),
    );

    if let Some(c) = cancel {
        if c.is_cancelled() {
            return Err(DomainError::ScanCancelled);
        }
    }

    // 构建文件树
    let entries = build_tree(&parsed_records).map_err(|e| {
        tracing::error!(
            target: "treesize::mft",
            "构建 MFT 树失败: 有效记录数={}, error={e}",
            parsed_records.len(),
        );
        DomainError::ScanFailed(format!("构建 MFT 树失败：{e}"))
    })?;

    tracing::info!(
        target: "treesize::mft",
        "MFT 构建完成：{} 个条目",
        entries.len(),
    );

    if let Some(c) = cancel {
        if c.is_cancelled() {
            return Err(DomainError::ScanCancelled);
        }
    }

    // 重建 FileNode 树
    let root_node = rebuild_tree(entries, &root, options, progress, cancel)?;

    let elapsed_ms = SystemTime::now()
        .duration_since(start)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    tracing::info!(
        target: "treesize::mft",
        "MFT 扫描完成：文件 {} 个，目录 {} 个，总大小 {}，耗时 {} ms",
        root_node.file_count,
        root_node.dir_count,
        root_node.size,
        elapsed_ms,
    );

    Ok(root_node)
}

/// 分类 MFT 读取错误，返回 (分类标签, 详细说明)
///
/// 用于区分数据损坏、物理 I/O 错误、逻辑 bug 三种根因。
fn classify_mft_error(err: &NtfsError) -> (&'static str, String) {
    match err {
        NtfsError::MftInvalidOffset { lcn, .. } if *lcn < 0 => (
            "元数据损坏",
            format!("MFT 起始 LCN 为负值（{}），NTFS 卷元数据异常", lcn),
        ),
        NtfsError::MftInvalidOffset { lcn, .. } if *lcn == 0 => {
            ("元数据损坏", "MFT 起始 LCN 为 0，NTFS 卷元数据异常".into())
        },
        NtfsError::MftInvalidOffset { .. } => (
            "逻辑错误",
            "LCN×cluster_size 乘法溢出导致偏移为 0，需检查类型范围".into(),
        ),
        NtfsError::MftSeekError { code, .. } if *code == 55 || *code == 2 => {
            ("卷状态异常", format!("卷设备不存在或已卸载（错误码 {}）", code))
        },
        NtfsError::MftSeekError { offset, code, .. } if *code == 1 || *code == 87 => (
            "逻辑错误",
            format!("SetFilePointerEx 参数错误：offset={:#x}（错误码 {}）", offset, code),
        ),
        NtfsError::MftSeekError { code, .. } => ("磁盘 I/O 错误", format!("MFT 寻址失败（错误码 {}）", code)),
        NtfsError::MftReadError { code, .. } if *code == 23 => {
            ("物理损坏", "磁盘 CRC 校验失败，建议检查磁盘健康状态".into())
        },
        NtfsError::MftReadError { code, .. } if *code == 27 => ("物理损坏", "磁盘扇区未找到，存在坏道".into()),
        NtfsError::MftReadError { code, .. } if *code == 38 => (
            "元数据损坏",
            "读取到文件尾，MFT valid_data_length 与实际数据不匹配".into(),
        ),
        NtfsError::MftReadError { code, .. } if *code == 111 => {
            ("物理损坏", "磁盘 I/O 设备错误，建议检查磁盘连接".into())
        },
        NtfsError::MftReadError { code, .. } if *code == 55 => ("卷状态异常", "卷已被卸载".into()),
        NtfsError::MftReadError { expected, code, .. } => (
            "磁盘 I/O 错误",
            format!("MFT 数据读取失败：期望 {} 字节（错误码 {}）", expected, code),
        ),
        _ => ("未知错误", format!("{}", err)),
    }
}

/// 从 MFT 条目列表重建 FileNode 树
///
/// 使用 HashMap 建立父子关系索引，从根（记录 5）开始递归构建子树。
fn rebuild_tree(
    entries: Vec<MftFileEntry>,
    root_path: &Path,
    options: &ScanOptions,
    _progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
    _cancel: Option<&CancelToken>,
) -> Result<FileNode> {
    // 步骤 1：构建 parent → children 索引
    let mut parent_index: HashMap<u64, Vec<MftFileEntry>> = HashMap::new();
    for entry in entries {
        parent_index.entry(entry.parent_record).or_default().push(entry);
    }

    // 步骤 2：从根开始递归构建树
    fn build_recursive(
        record_number: u64,
        parent_index: &HashMap<u64, Vec<MftFileEntry>>,
        current_path: &Path,
        min_size: u64,
        exclude_dirs: &[String],
        exclude_exts: &[String],
    ) -> FileNode {
        let children = parent_index.get(&record_number);
        let mut node = FileNode::new_dir(current_path.to_path_buf(), None);

        if let Some(child_entries) = children {
            let mut child_nodes: Vec<FileNode> = Vec::with_capacity(child_entries.len());

            for entry in child_entries {
                let child_path = current_path.join(&entry.name);

                // 排除过滤（目录名、扩展名）
                if should_exclude_entry(&entry.name, entry.is_directory, exclude_dirs, exclude_exts) {
                    continue;
                }

                if entry.is_directory {
                    // 递归构建子目录
                    let child = build_recursive(
                        entry.record_number,
                        parent_index,
                        &child_path,
                        min_size,
                        exclude_dirs,
                        exclude_exts,
                    );

                    // 更新第一级子目录的文件计数
                    // 实际上 aggregate() 会在外层统一调用
                    child_nodes.push(child);
                } else {
                    // 文件节点
                    let effective_size = if entry.size < min_size { 0 } else { entry.size };
                    let child = FileNode::new_file(child_path, effective_size, None);
                    child_nodes.push(child);
                }
            }

            node.children = child_nodes;
        }

        node
    }

    // 根记录号固定为 5
    let mut root_node = build_recursive(
        5,
        &parent_index,
        root_path,
        options.min_size,
        &options.exclude_dirs,
        &options.exclude_exts,
    );

    // 回溯聚合
    root_node.aggregate();
    root_node.sort_by_size_desc();

    Ok(root_node)
}

// ─── 测试 ────────────────────────────────────────────────────────────────────
//
// MFT 扫描测试需要在真实的 NTFS 卷上运行，不适合常规单元测试。
// 集成测试可通过 `-- --ignored` 运行。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "需要 NTFS 卷才能运行"]
    fn mft_scan_current_drive() {
        let engine = MftScanEngine::new();
        let root = PathBuf::from("C:\\");
        let options = ScanOptions::default();
        let result = engine.scan(root, &options, None, None);
        assert!(result.is_ok(), "MFT 扫描失败：{:?}", result.err());
        let node = result.unwrap();
        assert!(node.is_dir());
        assert!(node.file_count > 0);
        assert!(node.size.0 > 0);
    }
}
