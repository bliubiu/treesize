//! 扫描引擎抽象
//!
//! 定义领域层的扫描接口，由基础设施层提供具体实现。
//! 这样领域层不依赖 `std::fs`，便于测试与替换实现（例如未来接入 Windows MFT）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::error::Result;
use super::file_node::FileNode;
use super::value_objects::ByteSize;

/// 扫描引擎类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanEngineType {
    /// 自动选择：NTFS 卷使用 MFT，非 NTFS 使用 Fs
    Auto,
    /// Fs 引擎：目录枚举并行遍历（跨平台）
    Fs,
    /// MFT 直接读取（仅 Windows NTFS）
    #[cfg(target_os = "windows")]
    Mft,
    /// USN Journal 增量扫描（仅 Windows NTFS）
    #[cfg(target_os = "windows")]
    Usn,
}

impl Default for ScanEngineType {
    fn default() -> Self {
        Self::Auto
    }
}

impl ScanOptions {
    /// 检查文件/目录名是否应被排除
    pub fn should_exclude(&self, name: &str, is_dir: bool) -> bool {
        should_exclude_entry(name, is_dir, &self.exclude_dirs, &self.exclude_exts)
    }
}

/// 检查文件/目录名是否应被排除（独立函数，用于闭包按值捕获场景）
pub fn should_exclude_entry(name: &str, is_dir: bool, exclude_dirs: &[String], exclude_exts: &[String]) -> bool {
    if is_dir {
        let lower = name.to_ascii_lowercase();
        exclude_dirs.iter().any(|d| d.eq_ignore_ascii_case(&lower))
    } else {
        if let Some(dot_pos) = name.rfind('.') {
            let ext = name[dot_pos + 1..].to_ascii_lowercase();
            exclude_exts.iter().any(|e| e == &ext)
        } else {
            false
        }
    }
}

impl std::fmt::Display for ScanEngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Fs => write!(f, "fs"),
            #[cfg(target_os = "windows")]
            Self::Mft => write!(f, "mft"),
            #[cfg(target_os = "windows")]
            Self::Usn => write!(f, "usn"),
        }
    }
}

impl std::str::FromStr for ScanEngineType {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "fs" => Ok(Self::Fs),
            #[cfg(target_os = "windows")]
            "mft" => Ok(Self::Mft),
            #[cfg(target_os = "windows")]
            "usn" => Ok(Self::Usn),
            _ => Err(format!(
                "未知引擎类型：{}（可用：auto, fs{}）",
                s,
                if cfg!(target_os = "windows") { ", mft, usn" } else { "" }
            )),
        }
    }
}

/// 扫描选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanOptions {
    /// 扫描引擎类型
    pub engine: ScanEngineType,
    /// 是否跟随符号链接
    pub follow_links: bool,
    /// 是否包含隐藏文件
    pub include_hidden: bool,
    /// 单目录最大深度（0 = 不限）
    pub max_depth: usize,
    /// 最小文件大小过滤（字节），小于此值的文件不计入大小但仍计入计数
    pub min_size: u64,
    /// 需要排除的目录名（精确匹配，不区分大小写）
    pub exclude_dirs: Vec<String>,
    /// 需要排除的扩展名（小写、无点）
    pub exclude_exts: Vec<String>,
    /// 资源限制配置
    pub resource_limits: ResourceLimits,
    /// 大小口径：true = 表观大小（文件逻辑长度），false = 分配大小（卷实际占用）
    ///
    /// 默认 false，与 TreeSize / WizTree / `du` 的口径一致：
    /// 分配大小才反映"这个文件占了多少盘"，表观大小会把 3 字节文件算成 3 字节，
    /// 而 NTFS 上它实际占满一个簇。稀疏文件、压缩文件的差距尤其大。
    pub apparent_size: bool,
    /// 是否启用增量扫描
    pub incremental: bool,
    /// 增量扫描的基准时间（只扫描此时间之后修改的文件）
    pub incremental_since: Option<std::time::SystemTime>,
}

/// 资源限制配置
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// 最大内存使用（MB），0 表示不限制
    pub max_memory_mb: u64,
    /// 最大扫描时间（秒），0 表示不限制
    pub max_time_sec: u64,
    /// 最大文件数，0 表示不限制
    pub max_files: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            // 0 表示不限制（系统级内存监控精度不足，不适合做进程级限制）
            max_memory_mb: 0,
            max_time_sec: 300,     // 默认限制 5 分钟
            max_files: 10_000_000, // 默认限制 1000 万文件
        }
    }
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            engine: ScanEngineType::Auto,
            follow_links: false,
            include_hidden: true,
            max_depth: 0,
            min_size: 0,
            exclude_dirs: vec![],
            exclude_exts: vec![],
            resource_limits: ResourceLimits::default(),
            apparent_size: false,
            incremental: false,
            incremental_since: None,
        }
    }
}

/// 扫描进度回调载荷
#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    /// 已扫描的文件数
    pub files_scanned: u64,
    /// 已扫描的目录数
    pub dirs_scanned: u64,
    /// 当前正在扫描的路径
    pub current_path: String,
    /// 累计已扫描字节数
    pub bytes_scanned: u64,
}

impl Default for ScanProgress {
    fn default() -> Self {
        Self {
            files_scanned: 0,
            dirs_scanned: 0,
            current_path: String::new(),
            bytes_scanned: 0,
        }
    }
}

/// 扫描统计汇总
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanStats {
    /// 总文件数
    pub total_files: u64,
    /// 总目录数
    pub total_dirs: u64,
    /// 总大小
    pub total_size: ByteSize,
    /// 扫描错误数
    pub error_count: u64,
    /// 扫描耗时（毫秒）
    pub elapsed_ms: u64,
}

/// 取消令牌：用于 GUI 中止扫描
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// 扫描引擎领域接口
///
/// 实现方负责遍历文件系统并构造 `FileNode` 树。
pub trait ScanEngine: Send + Sync {
    /// 扫描指定根路径，返回根节点
    ///
    /// `progress` 回调在每次访问节点时被调用，可用于 CLI 进度条或 GUI 刷新。
    /// `cancel` 用于异步中止扫描。
    fn scan(
        &self,
        root: PathBuf,
        options: &ScanOptions,
        progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
        cancel: Option<&CancelToken>,
    ) -> Result<FileNode>;
}

#[cfg(test)]
pub(crate) mod testing {
    //! 测试用扫描引擎，构造内存中的虚拟文件树

    use std::path::PathBuf;

    use super::*;
    use crate::domain::file_node::FileNode;

    pub struct MockScanEngine {
        pub tree: Option<FileNode>,
    }

    impl MockScanEngine {
        pub fn new(tree: FileNode) -> Self {
            Self { tree: Some(tree) }
        }
    }

    impl ScanEngine for MockScanEngine {
        fn scan(
            &self,
            _root: PathBuf,
            _options: &ScanOptions,
            _progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
            _cancel: Option<&CancelToken>,
        ) -> Result<FileNode> {
            self.tree
                .clone()
                .ok_or_else(|| crate::domain::DomainError::ScanFailed("虚拟树未设置".into()))
        }
    }
}
