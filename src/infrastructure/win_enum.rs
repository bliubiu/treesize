//! Windows 快速目录枚举
//!
//! 通过 `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` 一次调用填满
//! 一个 64 KB 缓冲，从每条记录里直接取出名称、类型、表观大小
//! （`EndOfFile`）、分配大小（`AllocationSize`）、修改时间和 reparse tag。
//!
//! 对比标准库 `std::fs::read_dir` + `entry.metadata()` 的"列举 + 逐条 stat"，
//! 每个文件省下两次系统调用；对比 jwalk 同。分配大小是卷真正花掉的空间，
//! 稀疏文件、压缩文件、小文件（簇尾补齐）只有它才对得上 `du` 与 TreeSize。
//!
//! 缓冲区按 8 字节对齐分配，字段一律用 `from_ne_bytes` 逐字节读取，
//! 因此不对内存对齐做任何额外要求，且每个长度都在使用前与缓冲做边界检查。
//!
//! 该接口在部分文件系统（FAT、部分网络重定向器）上不被支持，
//! 此时 [`open`](self::WindowsDir::open) 返回错误，由调用方回落到标准库路径。

#![cfg(target_os = "windows")]

use std::ffi::{c_void, OsStr, OsString};
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::ptr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_FUNCTION, ERROR_INVALID_LEVEL, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
    ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileIdExtdDirectoryInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_EXTD_DIR_INFO, FILE_LIST_DIRECTORY,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use super::dir_reader::{EntryKind, RawEntry};

/// 每次调用向文件系统申请的字节数：足够填满几百条记录。
const BUFFER_BYTES: usize = 64 * 1024;

/// 扩展路径前缀 `\\?\`，用于突破 `MAX_PATH` 限制
const EXT_PREFIX: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];

/// UNC 前缀
const UNC_PREFIX: &[u16] = &[b'U' as u16, b'N' as u16, b'C' as u16, b'\\' as u16];

/// 反斜杠的 UTF-16 码元
const BSLASH: u16 = b'\\' as u16;

/// reparse tag 中代表"指向另一个路径"的那一位：符号链接、联接点、挂载点。
/// 正好与标准库认作符号链接的那批一致，所以枚举结果与 `std::fs::FileType` 自洽。
const NAME_SURROGATE: u32 = 0x2000_0000;

/// `FILETIME` 记的是 1601 年起的 100 纳秒刻度，到 1970 年的这一 tick 数
const EPOCH_TICKS_1601: i64 = 116_444_736_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;

// 记录内字段偏移，全部从结构体定义推出，避免手写常量漂移
const OFF_NEXT: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, NextEntryOffset);
const OFF_WRITE: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, LastWriteTime);
const OFF_EOF: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, EndOfFile);
const OFF_ALLOC: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, AllocationSize);
const OFF_ATTR: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, FileAttributes);
const OFF_NAME_LEN: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength);
const OFF_TAG: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, ReparsePointTag);
const OFF_NAME: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);

/// 一个已打开的目录的条目迭代器
///
/// 与 [`std::fs::ReadDir`] 一样不含 `.` 与 `..`。
pub struct WindowsDir {
    /// 独占持有的目录句柄，`Drop` 时关闭
    handle: OwnedHandle,
    /// 8 字节对齐的接收缓冲
    buffer: Box<[u64]>,
    /// 上一条记录之后的下一条记录偏移；`None` 表示需要向文件系统再要一批
    ///
    /// 用 `Option` 而不是"偏移 + 结束标记"，是为了让"缓冲已走完、需要重新填充"
    /// 与"记录链已终止"这两种终止原因不会互相误判。
    cursor: Option<usize>,
    /// 文件系统已无更多记录
    done: bool,
}

// SAFETY：`WindowsDir` 独占其句柄，不共享也不跨线程传递引用，
// 移动到别的线程只改变运行位置，不改变句柄的唯一所有权。
unsafe impl Send for WindowsDir {}

impl WindowsDir {
    /// 打开目录准备列举；失败时返回错误，调用方应回落到标准库路径
    ///
    /// 打开成功后会立刻试填一次缓冲：某些文件系统（如 FAT32、部分网络重定向器）
    /// 允许 `CreateFileW` 打开目录，却在列举时才回答"不支持"。
    /// 提前探明，调用方才能干净地回落，而不会在已经吐了半个目录之后才失败。
    pub fn open(dir: &Path) -> io::Result<Self> {
        let wide = wide_path(dir)?;
        // SAFETY：`wide` 以 NUL 结尾且在调用期间存活，安全属性为 null 是允许的，
        // `OPEN_EXISTING` 不创建新文件，`FILE_FLAG_BACKUP_SEMANTICS` 是打开目录必需。
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut iter = Self {
            handle: OwnedHandle(handle),
            buffer: vec![0u64; BUFFER_BYTES / size_of::<u64>()].into_boxed_slice(),
            cursor: None,
            done: false,
        };
        iter.fill()?;
        Ok(iter)
    }

    /// 向文件系统要下一批记录
    ///
    /// 注意 `GetFileInformationByHandleEx` 返回的是 `BOOL`，**不是写入字节数**——
    /// 写了多少条只能靠记录链上 `NextEntryOffset == 0` 的 terminator 判定。
    /// 因此成功时把整个缓冲都标成"可用"，解析走链即自然停在 terminator 上；
    /// NTFS 保证整条记录进缓冲，装不下时它会自己少放几条而不是截断记录。
    fn fill(&mut self) -> io::Result<()> {
        // SAFETY：句柄是 `self` 独占的有效目录句柄；指针与长度描述
        // `self.buffer` 中恰好那么多个可写字节，且在调用期间不被别名借用；
        // 使用的类别与读取的结构体一致。
        let ok = unsafe {
            GetFileInformationByHandleEx(
                self.handle.0,
                FileIdExtdDirectoryInfo,
                self.buffer.as_mut_ptr().cast::<c_void>(),
                BUFFER_BYTES as u32,
            )
        };
        if ok != 0 {
            self.cursor = Some(0);
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
            self.done = true;
            self.cursor = None;
            return Ok(());
        }
        Err(error)
    }

    /// 缓冲的原始字节。缓冲以 `u64` 分配，因此首地址 8 字节对齐。
    fn bytes(&self) -> &[u8] {
        // SAFETY：`[u64]` 可以当作八倍长度的 `[u8]` 读取：`u8` 无对齐要求，
        // 且 `vec![0u64; ..]` 保证每个字节都已初始化。
        unsafe { std::slice::from_raw_parts(self.buffer.as_ptr().cast::<u8>(), BUFFER_BYTES) }
    }

    /// 解析 `at` 处的记录，返回条目与下一条记录的偏移
    fn record(&self, at: usize) -> io::Result<(RawEntry, Option<usize>)> {
        let bytes = self.bytes();
        let malformed = || io::Error::other("文件系统返回了格式错误的目录记录");

        let name_len = u32_at(bytes, at + OFF_NAME_LEN).ok_or_else(malformed)? as usize;
        // 名称是 UTF-16，长度必须为偶数，且完整落在缓冲内
        if name_len % 2 != 0 {
            return Err(malformed());
        }
        let name_start = at + OFF_NAME;
        let name_bytes = bytes.get(name_start..name_start + name_len).ok_or_else(malformed)?;

        let next = match u32_at(bytes, at + OFF_NEXT).ok_or_else(malformed)? {
            0 => None,
            step if step as usize >= OFF_NAME + name_len => Some(at + step as usize),
            _ => return Err(malformed()),
        };

        // UTF-16 名称解码为文本：绝大多数是 ASCII，一次分配即最终大小
        let units: Vec<u16> = name_bytes
            .chunks_exact(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .collect();
        // 名称原样保留（含未配对代理项），保证拼接出的路径总能打开
        let name: Box<OsStr> = OsString::from_wide(&units).into();

        let attributes = u32_at(bytes, at + OFF_ATTR).ok_or_else(malformed)?;
        let tag = u32_at(bytes, at + OFF_TAG).ok_or_else(malformed)?;
        let kind = if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && tag & NAME_SURROGATE != 0 {
            EntryKind::Link
        } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            EntryKind::Directory
        } else {
            EntryKind::File
        };

        let entry = RawEntry {
            name,
            kind,
            apparent_size: bytes_from(i64_at(bytes, at + OFF_EOF).ok_or_else(malformed)?),
            allocated_size: bytes_from(i64_at(bytes, at + OFF_ALLOC).ok_or_else(malformed)?),
            modified: unix_seconds(i64_at(bytes, at + OFF_WRITE).ok_or_else(malformed)?),
        };
        Ok((entry, next))
    }
}

impl Iterator for WindowsDir {
    type Item = io::Result<RawEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.done {
                return None;
            }
            // 缓冲里没有下一条记录了，向文件系统要新的一批
            let Some(at) = self.cursor else {
                if let Err(error) = self.fill() {
                    self.done = true;
                    // 首次填充就失败说明该接口不可用，交给调用方回落
                    return Some(Err(error));
                }
                continue;
            };
            match self.record(at) {
                Ok((entry, next)) => {
                    self.cursor = next;
                    // 记录里的名字不能是空串，也不能是 `.` / `..`
                    if is_dot(&entry.name) {
                        continue;
                    }
                    return Some(Ok(entry));
                },
                Err(error) => {
                    // 缓冲损坏：无法安全继续，停在这里而不是返回垃圾数据
                    self.done = true;
                    return Some(Err(error));
                },
            }
        }
    }
}

/// `\\server\share` 形式要转成 `\\?\UNC\server\share`，扩展前缀下才会被接受
fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut raw: Vec<u16> = path.as_os_str().encode_wide().collect();
    if raw.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "路径不能包含 NUL 字符"));
    }
    // 扩展路径下只接受反斜杠
    for unit in raw.iter_mut() {
        if *unit == b'/' as u16 {
            *unit = b'\\' as u16;
        }
    }
    if raw.starts_with(EXT_PREFIX) {
        raw.push(0);
        return Ok(raw);
    }

    // 相对路径先补全为绝对路径：扩展前缀不解析 `.` 与 `..`
    let absolute: Vec<u16> = if is_absolute_wide(&raw) {
        raw
    } else {
        std::path::absolute(path)?.as_os_str().encode_wide().collect()
    };

    let mut wide = Vec::with_capacity(EXT_PREFIX.len() + absolute.len() + 1);
    wide.extend_from_slice(EXT_PREFIX);
    if absolute.first() == Some(&BSLASH) && absolute.get(1) == Some(&BSLASH) {
        wide.extend_from_slice(UNC_PREFIX);
        wide.extend_from_slice(&absolute[2..]);
    } else {
        wide.extend_from_slice(&absolute);
    }
    wide.push(0);
    Ok(wide)
}

/// 判别 UTF-16 形式（斜杠已归一为反斜杠）的路径是否已带根
///
/// 手写判据而不是走 `Path`：转换后的 `Vec<u16>` 尚未以 NUL 结尾，
/// 借 `OsStr::from_encoded_bytes_unchecked` 读进来既绕圈子又依赖内存布局。
fn is_absolute_wide(units: &[u16]) -> bool {
    // `\\server\share` 或扩展前缀 `\\?\`
    if units.starts_with(&[BSLASH, BSLASH]) {
        return true;
    }
    // `C:\...`：盘符 + 冒号 + 分隔符，盘符只可能是 ASCII 字母
    units.len() >= 3
        && matches!(units[0], 0x41..=0x5A | 0x61..=0x7A)
        && units[1] == u16::from(b':')
        && units[2] == BSLASH
}

/// 是否是文件系统自带的 `.` / `..` 占位项（Windows 上枚举也会返回它们）
fn is_dot(name: &OsStr) -> bool {
    name.is_empty() || name == OsStr::new(".") || name == OsStr::new("..")
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|s| u32::from_ne_bytes([s[0], s[1], s[2], s[3]]))
}

fn i64_at(bytes: &[u8], at: usize) -> Option<i64> {
    let slice = bytes.get(at..at + 8)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(slice);
    Some(i64::from_ne_bytes(buf))
}

/// 文件系统以有符号数报告的字节数，实际不会是负数
fn bytes_from(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// `FILETIME` 转 Unix 秒，1970 年之前返回 `None`
fn unix_seconds(ticks: i64) -> Option<SystemTime> {
    if ticks <= EPOCH_TICKS_1601 {
        return None;
    }
    let secs = u64::try_from((ticks - EPOCH_TICKS_1601) / TICKS_PER_SECOND).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// 文件系统或重定向器对"不支持该列举方式"的回答
pub fn is_unsupported(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if code == ERROR_INVALID_FUNCTION as i32
            || code == ERROR_NOT_SUPPORTED as i32
            || code == ERROR_INVALID_PARAMETER as i32
            || code == ERROR_INVALID_LEVEL as i32
    )
}

/// 独占持有的句柄，析构时关闭
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY：`self.0` 是 `open` 成功返回的目录句柄，且只会被关闭一次
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_names_kinds_and_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.txt"), "hello").unwrap();
        fs::write(root.join("b.log"), "world!").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();

        let entries: Vec<RawEntry> = WindowsDir::open(root).unwrap().map(|e| e.unwrap()).collect();

        assert_eq!(entries.len(), 3);
        let mut names: Vec<String> = entries.iter().map(|e| e.name.to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["a.txt", "b.log", "sub"]);

        let sub = entries
            .iter()
            .find(|e| &*e.name == OsStr::new("sub"))
            .expect("应存在 sub 目录");
        assert_eq!(sub.kind, EntryKind::Directory);

        let a = entries
            .iter()
            .find(|e| &*e.name == OsStr::new("a.txt"))
            .expect("应存在 a.txt");
        assert_eq!(a.kind, EntryKind::File);
        // 表观大小就是文件长度
        assert_eq!(a.apparent_size, 5);
        // 分配大小按簇向上取整，至少一个簇
        assert!(
            a.allocated_size >= 5,
            "分配大小应不小于表观大小，实际 {}",
            a.allocated_size
        );
        assert!(a.modified.is_some(), "应能取到修改时间");
    }

    #[test]
    fn allocation_is_at_least_a_cluster_and_apparent_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // 一个 1 字节文件：表观 1 字节，分配至少 4 KB
        fs::write(root.join("tiny.bin"), b"x").unwrap();

        let entry = WindowsDir::open(root)
            .unwrap()
            .map(|e| e.unwrap())
            .find(|e| &*e.name == OsStr::new("tiny.bin"))
            .expect("应存在 tiny.bin");

        assert_eq!(entry.apparent_size, 1);
        assert!(
            entry.allocated_size > entry.apparent_size,
            "NTFS 上 1 字节文件应按簇取整：表观 {}，分配 {}",
            entry.apparent_size,
            entry.allocated_size
        );
    }

    #[test]
    fn opening_a_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "x").unwrap();

        let result = WindowsDir::open(&file);
        assert!(result.is_err(), "普通文件不应能作为目录打开");
    }

    #[test]
    fn wide_path_adds_the_extended_prefix() {
        let wide = wide_path(Path::new(r"C:\tmp")).unwrap();
        let text = String::from_utf16_lossy(&wide[..wide.len() - 1]);
        assert_eq!(text, r"\\?\C:\tmp");
    }

    #[test]
    fn wide_path_converts_unc_to_the_extended_form() {
        let wide = wide_path(Path::new(r"\\server\share\dir")).unwrap();
        let text = String::from_utf16_lossy(&wide[..wide.len() - 1]);
        assert_eq!(text, r"\\?\UNC\server\share\dir");
    }

    #[test]
    fn wide_path_keeps_an_existing_prefix() {
        let wide = wide_path(Path::new(r"\\?\D:\data")).unwrap();
        let text = String::from_utf16_lossy(&wide[..wide.len() - 1]);
        assert_eq!(text, r"\\?\D:\data");
    }
}
