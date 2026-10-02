//! 目录条目读取门面
//!
//! 扫描器只需要"把一个目录列出来，每条带上名称、类型、大小、修改时间"。
//! 本模块把这一个能力收敛到一处，并在 Windows 上优选
//! `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)`：
//! 它一次调用填满 64 KB 缓冲，**每个文件省下标准库"列举 + 逐条 stat"的两次系统调用**，
//! 并且额外给出分配大小（`AllocationSize`）—— 稀疏文件、压缩文件、簇尾补齐
//! 只有它才对得上 `du` / TreeSize / WizTree。
//!
//! 该接口在部分文件系统与网络重定向器上不被支持，此时自动回落到
//! `std::fs::read_dir` + `entry.metadata()`，功能等价，只是慢一些、
//! 且拿不到分配大小（此时退化为表观大小）。

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::time::SystemTime;

/// 目录条目类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// 普通目录
    Directory,
    /// 符号链接、联接点或挂载点（Windows 上即 `FILE_ATTRIBUTE_REPARSE_POINT`
    /// 且 reparse tag 属于"指向另一个路径"的那一类）
    Link,
    /// 普通文件
    File,
}

/// 一次目录列举得到的条目原始信息
#[derive(Debug)]
pub struct RawEntry {
    /// 文件名（不含目录）。保留操作系统给出的原始形态，
    /// 含未配对 UTF-16 代理项的遗留文件名也能正常打开
    pub name: Box<OsStr>,
    /// 条目类型
    pub kind: EntryKind,
    /// 表观大小（`EndOfFile`）：`ls -l` 显示的那个数字，稀疏文件会偏大
    pub apparent_size: u64,
    /// 分配大小（`AllocationSize`）：卷真正花掉的字节数
    pub allocated_size: u64,
    /// 最后修改时间，文件系统未提供时为 `None`
    pub modified: Option<SystemTime>,
}

impl RawEntry {
    /// 按选项取大小：`apparent` 为真取表观大小，否则取分配大小
    pub fn size(&self, apparent: bool) -> u64 {
        if apparent {
            self.apparent_size
        } else {
            self.allocated_size
        }
    }

    /// 是否为目录（不含链接）
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory
    }

    /// 是否为符号链接 / 联接点
    pub fn is_link(&self) -> bool {
        self.kind == EntryKind::Link
    }
}

/// 列举一个目录的所有条目（不含 `.` 与 `..`）。
///
/// 返回的迭代器会持有目录句柄，可跨线程移动。调用者负责消费到 `None`
/// 或直接丢弃迭代器（丢弃即关闭句柄）。
pub fn read_dir(dir: &Path) -> io::Result<Box<dyn Iterator<Item = io::Result<RawEntry>> + Send>> {
    #[cfg(target_os = "windows")]
    {
        match super::win_enum::WindowsDir::open(dir) {
            Ok(iter) => return Ok(Box::new(iter)),
            Err(error) if super::win_enum::is_unsupported(&error) => {
                tracing::debug!(
                    target: "treesize::scanner",
                    "快速目录枚举不被该文件系统支持，回落到标准库：{}（错误码 {:?}）",
                    dir.display(),
                    error.raw_os_error(),
                );
            },
            Err(error) => return Err(error),
        }
    }
    let inner = std::fs::read_dir(dir)?;
    Ok(Box::new(StdDir { inner }))
}

/// 标准库回落路径：列举后逐条取元数据
struct StdDir {
    inner: std::fs::ReadDir,
}

impl Iterator for StdDir {
    type Item = io::Result<RawEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.inner.next()?;
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => return Some(Err(error)),
        };
        // 符号链接的元数据要跟随到目标，否则拿到的是链接自身的长度
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            // 断链的符号链接、扫描期间消失的条目：记为 0 字节而不是中断整个目录
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Some(Ok(RawEntry {
                    name: entry.file_name().into(),
                    kind: EntryKind::Link,
                    apparent_size: 0,
                    allocated_size: 0,
                    modified: None,
                }))
            },
            Err(error) => return Some(Err(error)),
        };
        let file_type = entry.file_type().ok();
        let kind = match file_type {
            Some(ft) if ft.is_symlink() => EntryKind::Link,
            Some(ft) if ft.is_dir() => EntryKind::Directory,
            _ => EntryKind::File,
        };
        Some(Ok(RawEntry {
            name: entry.file_name().into(),
            kind,
            apparent_size: meta.len(),
            // 标准库不提供分配大小，退化为表观大小
            allocated_size: meta.len(),
            modified: meta.modified().ok(),
        }))
    }
}

/// 取单个文件的分配大小（卷实际占用字节数）。
///
/// 目录枚举能从 `FILE_ID_EXTD_DIR_INFO` 白拿 `AllocationSize`，但单文件扫描
/// （`treesize <file>`）走不到枚举路径，这里补一次 `FileStandardInfo` 查询。
/// 失败时返回 `None`，由调用方退化为表观大小。
///
/// 不用 `GetCompressedFileSizeW`：它对普通文件返回的是**文件长度**而不是占用空间，
/// 只有压缩/稀疏文件才给"实际落盘量"，与这里要的语义正好相反。
#[cfg(target_os = "windows")]
pub fn allocated_size_of(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileStandardInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY：`wide` 以 NUL 结尾且在调用期间存活；只读属性、共享全开、不创建文件。
    // `FILE_FLAG_BACKUP_SEMANTICS` 让目录也能打开。
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    // 用守卫持有句柄，任何提前返回都不会漏掉 CloseHandle
    let guard = HandleGuard(handle);
    // SAFETY：`FILE_STANDARD_INFO` 是全零有效值，查询会填满整个结构
    let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    // SAFETY：`guard.0` 是有效句柄，`info` 有 `size_of` 那么多个可写字节
    let ok = unsafe {
        GetFileInformationByHandleEx(
            guard.0,
            FileStandardInfo,
            (&raw mut info).cast::<std::ffi::c_void>(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    u64::try_from(info.AllocationSize).ok()
}

/// 句柄守卫：任何提前返回都不会漏掉 `CloseHandle`
#[cfg(target_os = "windows")]
struct HandleGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        // SAFETY：`self.0` 是 `CreateFileW` 成功返回的句柄，且只会被关闭一次
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// 非 Windows 平台拿不到分配大小，调用方退化为表观大小
#[cfg(not(target_os = "windows"))]
pub fn allocated_size_of(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn std_path_lists_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.txt"), "hello").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();

        let mut names: Vec<String> = read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().name.to_string_lossy().into_owned())
            .collect();
        names.sort();

        assert_eq!(names, vec!["a.txt", "sub"]);
    }

    #[test]
    fn std_path_reports_a_missing_directory() {
        let result = read_dir(Path::new("不存在的目录/xxx"));
        assert!(result.is_err(), "不存在的目录应报错");
    }

    #[test]
    fn size_follows_the_apparent_flag() {
        let entry = RawEntry {
            name: OsStr::new("x").into(),
            kind: EntryKind::File,
            apparent_size: 10,
            allocated_size: 4096,
            modified: None,
        };
        assert_eq!(entry.size(true), 10);
        assert_eq!(entry.size(false), 4096);
        assert!(!entry.is_dir());
        assert!(!entry.is_link());
    }
}
