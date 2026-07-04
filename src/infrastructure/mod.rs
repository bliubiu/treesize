//! 基础设施层
//!
//! 提供文件系统访问、日志、内存监控等具体实现。

pub mod fs_scanner;
pub mod history_storage;
pub mod logging;
pub mod memory_monitor;

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
