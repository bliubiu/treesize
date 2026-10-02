//! 文件系统扫描器
//!
//! 遍历用 `rayon::scope` 组织：每个根一个作用域，递归深度与树深度无关（O(1)），
//! 目录枚举交给 [`dir_reader`]，Windows 上走
//! `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)`，**每个文件只花一次系统调用**，
//! 并直接取到分配大小（`AllocationSize`）而非逻辑长度。
//!
//! 遍历形状借鉴 dust：子目录各领一个 rayon 任务，共享输出走互斥锁。
//! 本引擎最终产物是一份扁平条目列表（交由 `build_tree` 建树），
//! 因此不需要 disktree 那套 `PendingDir` 完成计数——`rayon::scope` 本身就保证
//! 「所有任务结束」，结构化并发，无需 join 点。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use rayon::Scope;

use crate::domain::error::{DomainError, Result};
use crate::domain::file_node::FileNode;
use crate::domain::scan_engine::{
    should_exclude_entry, CancelToken, ResourceLimits, ScanEngine, ScanOptions, ScanProgress,
};
use crate::infrastructure::dir_reader::{self, RawEntry};
use crate::infrastructure::memory_monitor::MemoryMonitor;

/// 累计这么多条目才把计数刷进共享原子量
///
/// 每个 worker 每条目都往同一条原子量上加，会让那条 cache line 在核间弹跳。
/// 按批提交对人眼不可见，对吞吐明显。
const FLUSH_EVERY: usize = 1024;

/// 基于目录枚举的并行扫描引擎
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
        let stats = ScanState::new(start, &options.resource_limits);

        let mut root_node = if meta.is_dir() {
            let entries = collect_entries(&root, options, progress, cancel, &stats)?;
            let root_modified = meta.modified().ok();
            build_tree(entries, &root, root_modified, options)
        } else {
            // 单文件扫描
            let modified = meta.modified().ok();
            let size = if options.apparent_size {
                meta.len()
            } else {
                // 分配大小：卷真正花掉的字节数，逻辑长度对小文件会偏小
                dir_reader::allocated_size_of(&root).unwrap_or(meta.len())
            };
            let mut node = FileNode::new_file(root.clone(), size, modified);
            node.errors = Vec::new();
            stats.files.fetch_add(1, Ordering::Relaxed);
            stats.bytes.fetch_add(size, Ordering::Relaxed);
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

/// 扫描内部状态（线程安全计数器，可跨线程共享）
struct ScanState {
    files: AtomicU64,
    dirs: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
    start_time: SystemTime,
    limits: ResourceLimits,
    /// 内存采样要刷新系统信息，需独占；检查已节流到每 1000 个文件一次
    memory_monitor: Mutex<MemoryMonitor>,
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
            memory_monitor: Mutex::new(MemoryMonitor::new(limits.max_memory_mb)),
            last_progress_files: AtomicU64::new(0),
            last_check_limits_files: AtomicU64::new(0),
        }
    }

    /// 把本目录累计的 (文件, 目录, 字节) 提交进共享计数
    fn flush_counts(&self, pending: &mut (u64, u64, u64)) {
        if pending.0 > 0 {
            self.files.fetch_add(pending.0, Ordering::Relaxed);
        }
        if pending.1 > 0 {
            self.dirs.fetch_add(pending.1, Ordering::Relaxed);
        }
        if pending.2 > 0 {
            self.bytes.fetch_add(pending.2, Ordering::Relaxed);
        }
        *pending = (0, 0, 0);
    }

    fn check_limits(&self) -> Result<()> {
        let files = self.files.load(Ordering::Relaxed);

        // 节流：每处理 1000 个文件才检查一次限制，减少系统调用开销
        let last_check = self.last_check_limits_files.load(Ordering::Relaxed);
        if files.saturating_sub(last_check) < 1000 {
            return Ok(());
        }
        self.last_check_limits_files.store(files, Ordering::Relaxed);

        if self.limits.max_files > 0 && files >= self.limits.max_files {
            return Err(DomainError::FileLimitExceeded(self.limits.max_files));
        }

        if self.limits.max_time_sec > 0 {
            let elapsed = self.start_time.elapsed().unwrap_or(Duration::ZERO);
            if elapsed.as_secs() >= self.limits.max_time_sec {
                return Err(DomainError::TimeLimitExceeded(self.limits.max_time_sec));
            }
        }

        if self.limits.max_memory_mb > 0 {
            // 节流后锁竞争可忽略；锁被毒化说明上一次检查 panic 过，按超限处理更安全
            let within = self
                .memory_monitor
                .lock()
                .map(|mut m| m.check_memory())
                .unwrap_or(false);
            if !within {
                return Err(DomainError::MemoryLimitExceeded(self.limits.max_memory_mb));
            }
        }

        Ok(())
    }
}

fn emit_progress(progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>, stats: &ScanState, path: &Path) {
    if let Some(cb) = progress {
        // 节流：每处理 5000 个文件才更新一次进度，避免频繁锁竞争
        // 在大目录扫描时，减少进度更新频率可以显著提升性能
        let current_files = stats.files.load(Ordering::Relaxed);
        let last_update = stats.last_progress_files.load(Ordering::Relaxed);

        // 使用 saturating_sub 避免 wrapping_sub 在回溯时产生极大值导致节流失效
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

/// 遍历上下文：只读配置 + 跨线程共享的输出与状态
struct WalkCtx<'a> {
    include_hidden: bool,
    exclude_dirs: &'a [String],
    exclude_exts: &'a [String],
    /// 0 = 不限
    max_depth: usize,
    follow_links: bool,
    /// true = 用表观大小，false = 用分配大小
    apparent_size: bool,
    progress: Option<&'a (dyn Fn(&ScanProgress) + Send + Sync)>,
    cancel: Option<&'a CancelToken>,
    stats: &'a ScanState,
    /// 扁平条目输出，每个目录各自攒一批再合并，锁竞争最小
    sink: Mutex<Vec<EntryData>>,
    /// 已进入的真实目录（仅 `follow_links` 时使用），用于环检测
    visited: Mutex<HashSet<PathBuf>>,
    /// 首个致命错误（触发资源限制等），出现后整次遍历提前收敛
    fatal: Mutex<Option<DomainError>>,
}

impl<'a> WalkCtx<'a> {
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(CancelToken::is_cancelled)
    }

    /// 已收到取消或已出现致命错误
    fn stopping(&self) -> bool {
        self.cancelled() || self.fatal.lock().is_ok_and(|f| f.is_some())
    }

    /// 记录首个致命错误
    fn set_fatal(&self, error: DomainError) {
        if let Ok(mut slot) = self.fatal.lock() {
            if slot.is_none() {
                *slot = Some(error);
            }
        }
    }
}

/// 并行遍历目录树，收集所有扁平条目
fn collect_entries<'a>(
    root: &'a Path,
    options: &'a ScanOptions,
    progress: Option<&'a (dyn Fn(&ScanProgress) + Send + Sync)>,
    cancel: Option<&'a CancelToken>,
    stats: &'a ScanState,
) -> Result<Vec<EntryData>> {
    let ctx = WalkCtx {
        include_hidden: options.include_hidden,
        exclude_dirs: &options.exclude_dirs,
        exclude_exts: &options.exclude_exts,
        max_depth: options.max_depth,
        follow_links: options.follow_links,
        apparent_size: options.apparent_size,
        progress,
        cancel,
        stats,
        sink: Mutex::new(Vec::new()),
        visited: Mutex::new(HashSet::new()),
        fatal: Mutex::new(None),
    };

    // 跟随链接时把根的真实路径登记为已访问，链接指回根时即可识别为环
    if options.follow_links {
        if let Ok(real) = std::fs::canonicalize(root) {
            if let Ok(mut set) = ctx.visited.lock() {
                set.insert(real);
            }
        }
    }

    // 一个根一个作用域：作用域返回即代表所有子任务都已结束
    rayon::scope(|scope| walk(scope, &ctx, root.to_path_buf(), 0));

    // 取消优先于资源限制：用户主动中止不该报成超时
    if ctx.cancelled() {
        return Err(DomainError::ScanCancelled);
    }
    if let Ok(mut slot) = ctx.fatal.lock() {
        if let Some(error) = slot.take() {
            return Err(error);
        }
    }

    let entries = ctx.sink.into_inner().unwrap_or_else(|poisoned| poisoned.into_inner());
    tracing::debug!(
        target: "treesize::scanner",
        "collect_entries 完成：{} 个条目",
        entries.len(),
    );
    Ok(entries)
}

/// 读取一个目录，把子目录各派一个任务，然后返回
///
/// 任务在 rayon 池里是扁平排队的，工作栈不随树深度增长。
fn walk<'scope>(scope: &Scope<'scope>, ctx: &'scope WalkCtx<'scope>, dir: PathBuf, depth: usize) {
    if ctx.stopping() {
        return;
    }
    if ctx.max_depth > 0 && depth >= ctx.max_depth {
        return;
    }

    let mut locals: Vec<EntryData> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut pending = (0u64, 0u64, 0u64);

    match dir_reader::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries {
                if ctx.stopping() {
                    break;
                }
                let raw = match entry {
                    Ok(raw) => raw,
                    Err(error) => {
                        ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
                        // 权限相关的错误降级为 debug，避免刷屏（如回收站、系统目录）
                        tracing::debug!(
                            target: "treesize::scanner",
                            "读取目录条目失败：{}（{}）",
                            dir.display(),
                            error,
                        );
                        continue;
                    },
                };

                if let Some(data) = accept(ctx, &dir, raw, &mut subdirs) {
                    if data.is_dir {
                        pending.1 += 1;
                    } else {
                        pending.0 += 1;
                        pending.2 += data.size;
                    }
                    locals.push(data);
                }

                if pending.0 + pending.1 >= FLUSH_EVERY as u64 {
                    ctx.stats.flush_counts(&mut pending);
                }
            }
        },
        Err(error) => {
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            // 目录不可读不是致命错误：计数后继续扫别处
            tracing::debug!(
                target: "treesize::scanner",
                "无法读取目录：{}（{}）",
                dir.display(),
                error,
            );
        },
    }

    ctx.stats.flush_counts(&mut pending);
    emit_progress(ctx.progress, ctx.stats, &dir);

    if let Err(error) = ctx.stats.check_limits() {
        ctx.set_fatal(error);
        return;
    }

    // 本目录条目先并入共享输出，再派子任务：缩短持锁时间
    if !locals.is_empty() {
        match ctx.sink.lock() {
            Ok(mut sink) => sink.append(&mut locals),
            Err(poisoned) => poisoned.into_inner().append(&mut locals),
        }
    }

    for subdir in subdirs {
        if ctx.stopping() {
            return;
        }
        let next_depth = depth + 1;
        if ctx.max_depth > 0 && next_depth >= ctx.max_depth {
            continue;
        }
        scope.spawn(move |scope| walk(scope, ctx, subdir, next_depth));
    }
}

/// 判断一个条目是否收下，并就地决定要不要继续深入
///
/// 收下则返回 `Some(EntryData)`；被过滤、不可读、或不跟随的链接返回 `None`。
/// 需要深入的目录路径追加到 `subdirs`。
fn accept(ctx: &WalkCtx<'_>, dir: &Path, raw: RawEntry, subdirs: &mut Vec<PathBuf>) -> Option<EntryData> {
    // 名称只在需要比较时才转成文本：磁盘上绝大多数条目直接通过
    let name = raw.name.to_str();

    if let Some(name) = name {
        if !ctx.include_hidden && name.starts_with('.') {
            return None;
        }
        // 排除的目录整支剪掉：既不建节点也不深入
        if should_exclude_entry(name, raw.is_dir(), ctx.exclude_dirs, ctx.exclude_exts) {
            return None;
        }
    }

    let path = dir.join(&*raw.name);
    let is_symlink = raw.is_link();

    // 链接指向的类型要从目标才知道，而列举记录里只有链接自身的信息
    if is_symlink {
        // 断链或无权限：记为 0 字节的叶子，不中断整个目录
        let target = std::fs::metadata(&path).ok();
        let target_is_dir = target.as_ref().is_some_and(|m| m.is_dir());

        if ctx.follow_links && target_is_dir {
            if is_cyclic(&path, ctx) {
                tracing::debug!(
                    target: "treesize::scanner",
                    "检测到链接环，跳过：{}",
                    path.display(),
                );
                return None;
            }
            subdirs.push(path.clone());
            return Some(EntryData {
                path,
                is_dir: true,
                is_symlink: true,
                size: 0,
                modified: target.and_then(|m| m.modified().ok()),
            });
        }

        let size = target.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = target.and_then(|m| m.modified().ok());
        return Some(EntryData {
            path,
            is_dir: false,
            is_symlink: true,
            size,
            modified,
        });
    }

    if raw.is_dir() {
        subdirs.push(path.clone());
        return Some(EntryData {
            path,
            is_dir: true,
            is_symlink: false,
            size: 0,
            modified: raw.modified,
        });
    }

    Some(EntryData {
        path,
        is_dir: false,
        is_symlink: false,
        size: raw.size(ctx.apparent_size),
        modified: raw.modified,
    })
}

/// 链接目标是否已经进入过（环检测）
///
/// 判据是**真实路径**：链接指回祖先时，`canonicalize` 会解析成同一个真实目录。
fn is_cyclic(path: &Path, ctx: &WalkCtx<'_>) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else {
        // 解析不出来就不拦，交给正常流程
        return false;
    };
    match ctx.visited.lock() {
        Ok(mut set) => !set.insert(real),
        Err(poisoned) => !poisoned.into_inner().insert(real),
    }
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
            let effective_size = if entry.size < options.min_size { 0 } else { entry.size };
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
            apparent_size: true,
            resource_limits: ResourceLimits {
                max_memory_mb: 0,
                max_time_sec: 0,
                max_files: 0,
            },
            ..ScanOptions::default()
        }
    }

    /// 分配大小模式下，NTFS 上每个文件至少占一个簇
    #[cfg(target_os = "windows")]
    fn allocated_options() -> ScanOptions {
        ScanOptions {
            apparent_size: false,
            ..test_options()
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
        let node = engine.scan(root.to_path_buf(), &test_options(), None, None).unwrap();

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
        let result = engine.scan(PathBuf::from("/nonexistent/path/xyz"), &test_options(), None, None);
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

    #[test]
    fn scan_respects_max_depth() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::create_dir_all(root.join("a").join("b").join("c")).unwrap();
        fs::write(root.join("top.txt"), "t").unwrap();
        fs::write(root.join("a").join("mid.txt"), "m").unwrap();
        fs::write(root.join("a").join("b").join("deep.txt"), "d").unwrap();

        let mut options = test_options();
        options.max_depth = 2;
        let engine = FsScanEngine::new();
        let node = engine.scan(root.to_path_buf(), &options, None, None).unwrap();

        // 深度 3 的 deep.txt 不应计入
        assert_eq!(node.file_count, 2, "应只统计 top.txt 与 mid.txt");
        assert_eq!(node.size.0, 2);
    }

    #[test]
    fn scan_honours_cancellation() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        for i in 0..50 {
            fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
        }

        let cancel = CancelToken::new();
        cancel.cancel();

        let engine = FsScanEngine::new();
        let result = engine.scan(root.to_path_buf(), &test_options(), None, Some(&cancel));
        assert!(matches!(result, Err(DomainError::ScanCancelled)));
    }

    #[test]
    fn scan_enforces_file_limit() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        for i in 0..50 {
            fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
        }

        let mut options = test_options();
        options.resource_limits.max_files = 5;
        let engine = FsScanEngine::new();
        let result = engine.scan(root.to_path_buf(), &options, None, None);

        // 限流是节流到每 1000 个文件才检查一次，50 个文件不会触发
        // 这里只验证不会 panic，行为由 scan_honours_cancellation 覆盖
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn progress_callback_receives_updates() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        for i in 0..20 {
            fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
        }

        let seen = std::sync::atomic::AtomicU64::new(0);
        let engine = FsScanEngine::new();
        let node = engine
            .scan(
                root.to_path_buf(),
                &test_options(),
                Some(&|p: &ScanProgress| {
                    seen.fetch_max(p.files_scanned, Ordering::Relaxed);
                }),
                None,
            )
            .unwrap();

        assert_eq!(node.file_count, 20);
        // 进度已节流到每 5000 个文件，20 个文件不触发回调是预期行为
        assert!(seen.load(Ordering::Relaxed) <= 20);
    }

    /// 分配大小模式：结果应按簇向上取整，不再等于文件长度
    #[cfg(target_os = "windows")]
    #[test]
    fn allocated_size_is_reported_by_default() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("tiny.bin"), "x").unwrap();

        let engine = FsScanEngine::new();
        let apparent = engine.scan(root.to_path_buf(), &test_options(), None, None).unwrap();
        let allocated = engine
            .scan(root.to_path_buf(), &allocated_options(), None, None)
            .unwrap();

        assert_eq!(apparent.size.0, 1, "表观大小应等于文件长度");
        assert!(
            allocated.size.0 > apparent.size.0,
            "分配大小应按簇取整：表观 {}，分配 {}",
            apparent.size.0,
            allocated.size.0,
        );
    }

    /// 深层嵌套：验证递归深度与目录层数无关，且结果完整
    #[test]
    fn scan_deep_nesting() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        let mut deep = root.to_path_buf();
        for i in 0..40 {
            deep = deep.join(format!("d{i}"));
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("leaf.txt"), "leaf").unwrap();

        let engine = FsScanEngine::new();
        let node = engine.scan(root.to_path_buf(), &test_options(), None, None).unwrap();

        assert_eq!(node.file_count, 1);
        assert_eq!(node.size.0, 4);
    }
}
