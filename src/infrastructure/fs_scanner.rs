//! 文件系统扫描器
//!
//! 使用 `jwalk` 并行遍历替代手动递归遍历，大幅提升大目录扫描性能。
//! jwalk 内部使用 rayon 线程池并行执行 read_dir 操作，并在 depth-first 顺序下 yield 结果。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use jwalk::WalkDir;

use crate::domain::error::{DomainError, Result};
use crate::domain::file_node::FileNode;
use crate::domain::scan_engine::{
    should_exclude_entry, CancelToken, ResourceLimits, ScanEngine, ScanOptions, ScanProgress,
};
use crate::infrastructure::memory_monitor::MemoryMonitor;

/// 基于 jwalk 的并行扫描引擎
pub struct FsScanEngine;

impl FsScanEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FsScanEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanEngine for FsScanEngine {
    fn scan(
        &self,
        root: PathBuf,
        options: &ScanOptions,
        progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
        cancel: Option<&CancelToken>,
    ) -> Result<FileNode> {
        // 检查根路径是否存在
        let meta = std::fs::metadata(&root).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DomainError::PathNotFound(root.display().to_string())
            } else {
                DomainError::PathInaccessible(root.display().to_string())
            }
        })?;

        let start = SystemTime::now();
        let mut stats = ScanState::new(start, &options.resource_limits);

        let mut root_node = if meta.is_dir() {
            // 使用 jwalk 并行遍历扫描所有条目
            let entries = collect_entries(&root, options, progress, cancel, &mut stats)?;
            let root_modified = meta.modified().ok();
            build_tree(entries, &root, root_modified, options)
        } else {
            // 单文件扫描
            let modified = meta.modified().ok();
            let mut node = FileNode::new_file(root.clone(), meta.len(), modified);
            node.errors = Vec::new();
            stats.files.fetch_add(1, Ordering::Relaxed);
            stats.bytes.fetch_add(meta.len(), Ordering::Relaxed);
            emit_progress(progress, &stats, &root);
            node
        };

        root_node.aggregate();
        root_node.sort_by_size_desc();

        let elapsed_ms = SystemTime::now()
            .duration_since(start)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        tracing::info!(
            target: "treesize::scanner",
            "扫描完成：文件 {} 个，目录 {} 个，总大小 {}，耗时 {} ms，错误 {} 个",
            root_node.file_count,
            root_node.dir_count,
            root_node.size,
            elapsed_ms,
            stats.errors.load(Ordering::Relaxed),
        );

        Ok(root_node)
    }
}

/// 扫描内部状态（线程安全计数器）
struct ScanState {
    files: AtomicU64,
    dirs: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
    start_time: SystemTime,
    limits: ResourceLimits,
    memory_monitor: MemoryMonitor,
    /// 上次进度更新时的文件计数（用于节流）
    last_progress_files: AtomicU64,
    /// 上次检查限制时的文件计数（用于节流）
    last_check_limits_files: AtomicU64,
}

impl ScanState {
    fn new(start_time: SystemTime, limits: &ResourceLimits) -> Self {
        Self {
            files: AtomicU64::new(0),
            dirs: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            start_time,
            limits: *limits,
            memory_monitor: MemoryMonitor::new(limits.max_memory_mb),
            last_progress_files: AtomicU64::new(0),
            last_check_limits_files: AtomicU64::new(0),
        }
    }

    fn check_limits(&mut self) -> Result<()> {
        let files = self.files.load(Ordering::Relaxed);

        // 节流：每处理 1000 个文件才检查一次限制，减少系统调用开销
        let last_check = self.last_check_limits_files.load(Ordering::Relaxed);
        if files.saturating_sub(last_check) < 1000 {
            return Ok(());
        }
        self.last_check_limits_files.store(files, Ordering::Relaxed);

        if self.limits.max_files > 0 {
            if files >= self.limits.max_files {
                return Err(DomainError::FileLimitExceeded(self.limits.max_files));
            }
        }

        if self.limits.max_time_sec > 0 {
            let elapsed = self.start_time.elapsed().unwrap_or(Duration::ZERO);
            if elapsed.as_secs() >= self.limits.max_time_sec {
                return Err(DomainError::TimeLimitExceeded(self.limits.max_time_sec));
            }
        }

        if self.limits.max_memory_mb > 0 {
            if !self.memory_monitor.check_memory() {
                return Err(DomainError::MemoryLimitExceeded(self.limits.max_memory_mb));
            }
        }

        Ok(())
    }
}

fn emit_progress(
    progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
    stats: &ScanState,
    path: &Path,
) {
    if let Some(cb) = progress {
        // 节流：每处理 5000 个文件才更新一次进度，避免频繁锁竞争
        // 在大目录扫描时，减少进度更新频率可以显著提升性能
        let current_files = stats.files.load(Ordering::Relaxed);
        let last_update = stats.last_progress_files.load(Ordering::Relaxed);
        
        // 使用 saturating_sub 避免 wrapping_sub 在回绕时产生极大值导致节流失效
        if current_files.saturating_sub(last_update) >= 5000 {
            stats.last_progress_files.store(current_files, Ordering::Relaxed);
            cb(&ScanProgress {
                files_scanned: current_files,
                dirs_scanned: stats.dirs.load(Ordering::Relaxed),
                current_path: path.display().to_string(),
                bytes_scanned: stats.bytes.load(Ordering::Relaxed),
            });
        }
    }
}

/// 扁平条目数据，在构建 FileNode 树之前暂存
struct EntryData {
    path: PathBuf,
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    modified: Option<SystemTime>,
}

/// 使用 jwalk 并行遍历目录，收集所有扁平条目
///
/// 通过 `process_read_dir` 回调在 yield 之前完成过滤，
/// 避免后续条目产生不必要的 IO。
fn collect_entries(
    root: &Path,
    options: &ScanOptions,
    progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
    cancel: Option<&CancelToken>,
    stats: &mut ScanState,
) -> Result<Vec<EntryData>> {
    // 克隆选项值供闭包使用（闭包按值捕获）
    let include_hidden = options.include_hidden;
    let exclude_dirs = options.exclude_dirs.clone();
    let exclude_exts = options.exclude_exts.clone();
    let max_depth = options.max_depth;
    let follow_links = options.follow_links;

    // 构建 jwalk walker
    //
    // process_read_dir 在 yield 每个条目之前回调，允许我们：
    // 1. 过滤条目（筛选掉排除项）
    // 2. 阻止深入子目录（read_children_path = None）
    //
    // jwalk 默认使用 rayon 并行执行，无需手动设置线程数
    let walker = WalkDir::new(root)
        .follow_links(follow_links)
        .process_read_dir(move |_parent_depth, _path, _state, entries| {
            // 使用原地分区模式过滤并修改条目：
            // 先检查并修改 read_children_path，再压缩移除已排除条目
            let mut write_idx = 0;
            for i in 0..entries.len() {
                let keep = match &mut entries[i] {
                    Ok(ref mut entry) => {
                        // 隐藏文件过滤
                        if !include_hidden {
                            if let Some(name) = entry.file_name.to_str() {
                                if name.starts_with('.') {
                                    continue; // 跳过隐藏文件
                                }
                            }
                        }

                        // 排除过滤（目录名、扩展名）
                        if let Some(name) = entry.file_name.to_str() {
                            if should_exclude_entry(name, entry.file_type.is_dir(), &exclude_dirs, &exclude_exts) {
                                if entry.file_type.is_dir() {
                                    entry.read_children_path = None; // 不深入
                                }
                                continue;
                            }
                        }

                        // 深度限制：达到最大深度后不再深入，但保留当前条目
                        if max_depth > 0 && entry.depth >= max_depth {
                            entry.read_children_path = None;
                        }

                        true
                    }
                    Err(_) => true, // 保留错误条目，由主循环处理
                };

                if keep {
                    if write_idx != i {
                        entries.swap(write_idx, i);
                    }
                    write_idx += 1;
                }
            }
            entries.truncate(write_idx);
        });

    // 遍历收集所有条目
    let mut entries = Vec::new();

    for result in walker {
        if let Some(c) = cancel {
            if c.is_cancelled() {
                return Err(DomainError::ScanCancelled);
            }
        }

        stats.check_limits()?;

        match result {
            Ok(entry) => {
                // 跳过根节点自身（depth == 0），由 scan() 单独处理
                if entry.depth == 0 {
                    continue;
                }

                let path = entry.path();
                let is_symlink = entry.path_is_symlink();
                let is_dir = entry.file_type.is_dir();
                let _depth = entry.depth;

                // 获取元数据（大小、修改时间），失败时记录错误
                let (size, modified) = match entry.metadata() {
                    Ok(meta) => {
                        let sz = if is_dir { 0 } else { meta.len() };
                        (sz, meta.modified().ok())
                    }
                    Err(_) => {
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                        // 权限相关的错误降级为 debug，避免刷屏（如回收站、系统目录）
                        tracing::debug!(
                            target: "treesize::scanner",
                            "无法读取条目元数据：{}",
                            path.display()
                        );
                        (0, None)
                    }
                };

                // 更新统计计数器
                if is_dir {
                    stats.dirs.fetch_add(1, Ordering::Relaxed);
                } else {
                    stats.files.fetch_add(1, Ordering::Relaxed);
                    stats.bytes.fetch_add(size, Ordering::Relaxed);
                }

                emit_progress(progress, stats, &path);

                entries.push(EntryData {
                    path,
                    is_dir,
                    is_symlink,
                    size,
                    modified,
                });
            }
            Err(e) => {
                stats.errors.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    target: "treesize::scanner",
                    "扫描过程错误：{}",
                    e
                );
            }
        }
    }

    Ok(entries)
}

/// 从扁平条目列表重建 FileNode 树
///
/// 使用 Vec arena + 路径→索引 HashMap 代替双 HashMap，减少路径冗余存储。
/// 反向迭代挂载子节点，避免递归构建的栈溢出风险和 children_idx 中重复的 PathBuf。
/// 时间复杂度 O(n)，空间复杂度 O(n)（但常数因子更小）。
fn build_tree(
    entries: Vec<EntryData>,
    root_path: &Path,
    root_modified: Option<SystemTime>,
    options: &ScanOptions,
) -> FileNode {
    use std::collections::HashMap;

    tracing::debug!(target: "treesize::scanner", "build_tree: 根路径={:?}, 条目数={}", root_path, entries.len());

    let total = entries.len() + 1;
    let mut path_to_idx: HashMap<PathBuf, usize> = HashMap::with_capacity(total);
    let mut nodes: Vec<Option<FileNode>> = Vec::with_capacity(total);

    // 1. 插入根节点
    path_to_idx.insert(root_path.to_path_buf(), 0);
    nodes.push(Some(FileNode::new_dir(root_path.to_path_buf(), root_modified)));

    // 2. 处理所有收集到的条目，存入 arena
    for entry in entries {
        let idx = nodes.len();
        path_to_idx.insert(entry.path.clone(), idx);

        let node = if entry.is_dir {
            FileNode::new_dir(entry.path, entry.modified)
        } else {
            // min_size 过滤：小于最小值的文件计为 0 字节
            let effective_size = if entry.size < options.min_size {
                0
            } else {
                entry.size
            };
            let mut node = FileNode::new_file(entry.path, effective_size, entry.modified);
            if entry.is_symlink && !options.follow_links {
                node.errors.push("符号链接（未跟随）".into());
            }
            node
        };

        nodes.push(Some(node));
    }

    // 3. 反向遍历，将子节点挂载到父节点
    //
    // 从后向前处理保证子节点先于父节点被处理（叶节点先挂载），
    // 且每个子节点只被处理一次。
    for i in (1..nodes.len()).rev() {
        // 提取路径（需要 clone 因为 take 后会移走 FileNode）
        let path = nodes[i].as_ref().unwrap().path.clone();
        if let Some(parent_path) = path.parent() {
            if let Some(&parent_idx) = path_to_idx.get(parent_path) {
                if let Some(child) = nodes[i].take() {
                    nodes[parent_idx].as_mut().unwrap().children.push(child);
                }
            }
        }
    }

    tracing::debug!(target: "treesize::scanner", "build_tree 完成: 根节点子节点数={}, 剩余未挂载节点={}",
        nodes[0].as_ref().unwrap().children.len(), nodes.len() - 1);

    nodes.swap_remove(0).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn test_options() -> ScanOptions {
        ScanOptions {
            resource_limits: ResourceLimits {
                max_memory_mb: 0,
                max_time_sec: 0,
                max_files: 0,
            },
            ..ScanOptions::default()
        }
    }

    #[test]
    fn scan_basic_directory() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("a.txt"), "hello").unwrap();
        fs::write(root.join("b.log"), "world").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("c.txt"), "rust").unwrap();

        let engine = FsScanEngine::new();
        let node = engine
            .scan(root.to_path_buf(), &test_options(), None, None)
            .unwrap();

        assert!(node.is_dir());
        assert_eq!(node.file_count, 3);
        assert_eq!(node.dir_count, 1);
        assert_eq!(node.size.0, 5 + 5 + 4);
    }

    #[test]
    fn scan_respects_exclude_dirs() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("keep.txt"), "x").unwrap();
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules").join("big.bin"), "xxxxx").unwrap();

        let mut options = test_options();
        options.exclude_dirs = vec!["node_modules".into()];
        let engine = FsScanEngine::new();
        let node = engine.scan(root.to_path_buf(), &options, None, None).unwrap();

        assert_eq!(node.file_count, 1);
        assert_eq!(node.size.0, 1);
    }

    #[test]
    fn scan_respects_exclude_exts() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("a.txt"), "xxx").unwrap();
        fs::write(root.join("b.log"), "yyyyy").unwrap();

        let mut options = test_options();
        options.exclude_exts = vec!["log".into()];
        let engine = FsScanEngine::new();
        let node = engine.scan(root.to_path_buf(), &options, None, None).unwrap();

        assert_eq!(node.file_count, 1);
        assert_eq!(node.size.0, 3);
    }

    #[test]
    fn scan_nonexistent_path() {
        let engine = FsScanEngine::new();
        let result = engine.scan(
            PathBuf::from("/nonexistent/path/xyz"),
            &test_options(),
            None,
            None,
        );
        assert!(matches!(result, Err(DomainError::PathNotFound(_))));
    }

    #[test]
    fn scan_single_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("single.txt");
        fs::write(&file, "hello world").unwrap();

        let engine = FsScanEngine::new();
        let node = engine.scan(file.clone(), &test_options(), None, None).unwrap();

        assert!(node.is_file());
        assert_eq!(node.size.0, 11);
    }
}
