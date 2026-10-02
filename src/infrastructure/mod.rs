//! 基础设施层
//!
//! 提供文件系统访问、日志、内存监控等具体实现。

use std::path::Path;

use crate::domain::scan_engine::ScanEngineType;

pub mod dir_reader;
pub mod fs_scanner;
pub mod history_storage;
pub mod logging;
pub mod memory_monitor;

// Windows 快速目录枚举（FileIdExtdDirectoryInfo）
#[cfg(target_os = "windows")]
pub mod win_enum;

// Windows NTFS 直接读取（MFT + USN Journal）
#[cfg(target_os = "windows")]
pub mod mft_scanner;
#[cfg(target_os = "windows")]
pub mod ntfs_reader;
#[cfg(target_os = "windows")]
pub mod usn_scanner;

pub use fs_scanner::FsScanEngine;
pub use history_storage::HistoryStorage;
pub use memory_monitor::MemoryMonitor;

#[cfg(target_os = "windows")]
pub use mft_scanner::MftScanEngine;
#[cfg(target_os = "windows")]
pub use usn_scanner::UsnScanEngine;

/// 根据扫描路径所在的文件系统智能选择最佳扫描引擎
///
/// Windows NTFS 卷 **且** 当前进程有直读 MFT 的权限（管理员）→ MFT 引擎（极速）
/// 其他情况 → Fs 引擎（跨平台并行遍历）
///
/// MFT 直读是 NTFS 的权限约束：`\\.\X:` 卷设备在非管理员进程下一律
/// `ERROR_ACCESS_DENIED`。若只按「是不是 NTFS」判断，普通用户使用默认
/// `Auto` 会直接扫描失败，因此这里必须同时探测权限可用性。
pub fn detect_best_engine(path: &Path) -> ScanEngineType {
    #[cfg(target_os = "windows")]
    {
        if ntfs_reader::is_ntfs_volume(path) {
            if ntfs_reader::can_read_mft(path) {
                tracing::info!(target: "treesize::engine", "检测到 NTFS 卷且具备 MFT 直读权限 → MFT 引擎");
                return ScanEngineType::Mft;
            }
            tracing::warn!(
                target: "treesize::engine",
                "检测到 NTFS 卷，但当前进程无 MFT 直读权限（需管理员）→ 回落到 Fs 引擎"
            );
            return ScanEngineType::Fs;
        }
        tracing::info!(target: "treesize::engine", "非 NTFS 卷 → Fs 引擎");
    }
    #[cfg(not(target_os = "windows"))]
    {
        tracing::info!(target: "treesize::engine", "非 Windows 平台 → Fs 引擎");
    }
    ScanEngineType::Fs
}
