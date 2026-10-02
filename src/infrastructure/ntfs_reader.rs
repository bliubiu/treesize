//! NTFS 底层读取模块（仅 Windows）
//!
//! 提供 MFT 直接读取、MFT 记录解析、USN Journal 查询等底层函数。
//! 本模块自行定义 windows-sys 0.59 中缺失的 FSCTL 常量和外联函数。

#![cfg(target_os = "windows")]

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use rayon::prelude::*;
use thiserror::Error;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

// ─── 手动定义的 IOCTL／常量 ──────────────────────────────────────────────

const FSCTL_GET_NTFS_VOLUME_DATA: u32 = 0x00090064;
const FSCTL_QUERY_USN_JOURNAL: u32 = 0x000900ec;
/// `ERROR_ACCESS_DENIED`
const ERROR_ACCESS_DENIED: u32 = 5;
const FSCTL_READ_USN_JOURNAL: u32 = 0x000901eb;
const OPEN_EXISTING: u32 = 3;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const FILE_READ_DATA: u32 = 1;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;

// ─── 外部函数声明 ──────────────────────────────────────────────────────────

extern "system" {
    fn CreateFileW(
        lpfilename: *const u16,
        dwdesiredaccess: u32,
        dwsharemode: u32,
        lpsecurityattributes: *const std::ffi::c_void,
        dwcreationdisposition: u32,
        dwflagsandattributes: u32,
        htemplatefile: HANDLE,
    ) -> HANDLE;
    fn ReadFile(
        hfile: HANDLE,
        lpbuffer: *mut u8,
        nnumberofbytestoread: u32,
        lpnumberofbytesread: *mut u32,
        lpoverlapped: *mut std::ffi::c_void,
    ) -> i32;
    fn SetFilePointerEx(hfile: HANDLE, lidistancetomove: i64, lpnewfilepointer: *mut i64, dwmovemethod: u32) -> i32;
    fn DeviceIoControl(
        hdevice: HANDLE,
        dwiocontrolcode: u32,
        lpinbuffer: *const std::ffi::c_void,
        ninbuffersize: u32,
        lpoutbuffer: *mut std::ffi::c_void,
        noutbuffersize: u32,
        lpbytesreturned: *mut u32,
        lpoverlapped: *mut std::ffi::c_void,
    ) -> i32;
    fn GetLastError() -> u32;
    fn GetVolumeInformationW(
        lprootpathname: *const u16,
        lpvolumenamebuffer: *mut u16,
        nvolumenamesize: u32,
        lpvolumeserialnumber: *mut u32,
        lpmaximumcomponentlength: *mut u32,
        lpfilesystemflags: *mut u32,
        lpfilesystemnamebuffer: *mut u16,
        nfilesystemnamebufsize: u32,
    ) -> i32;
}

#[inline]
fn last_error(context: impl Into<String>) -> NtfsError {
    NtfsError::ApiError {
        context: context.into(),
        code: unsafe { GetLastError() },
    }
}

// ─── 错误类型 ────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum NtfsError {
    #[error("Windows API 调用失败：{context}（错误码：{code}）")]
    ApiError { context: String, code: u32 },
    #[error("无效的 MFT 记录（记录号 {record}）：预期 'FILE'，实际为 {magic:#04x?}")]
    InvalidMftMagic { record: u64, magic: [u8; 4] },
    #[error("数据不足：需要 {needed} 字节，实际 {actual} 字节")]
    InsufficientData { needed: usize, actual: usize },
    /// 引导扇区 OEM ID 不是 `NTFS    `，说明该卷不是 NTFS
    #[error("引导扇区 OEM ID 不是 NTFS，该卷不是 NTFS 文件系统")]
    NotNtfs,
    /// 引导扇区里的几何字段取值非法
    #[error("引导扇区几何信息非法（读到的值：{value}）")]
    InvalidBootGeometry { value: u32 },
    /// 直读 MFT 需要管理员权限，当前进程被拒绝
    #[error(
        "直读 MFT 需要管理员权限：无法打开卷设备 {path}（错误码：{code}）。\
         请以管理员身份重新运行，或改用 Fs 引擎（--engine fs）"
    )]
    NeedAdministrator { code: u32, path: String },
    #[error("USN Journal 不存在")]
    NoUsnJournal,
    #[error("文件名包含无效 UTF-16")]
    InvalidFileNameUtf16,
    #[error("NTFS I/O 错误：{0}")]
    Io(#[from] std::io::Error),
    /// MFT 起始 LCN 计算出的字节偏移为 0（lcn 负值/乘法溢出/元数据损坏）
    #[error("MFT 字节偏移无效: lcn={lcn}, cluster_size={cluster_size}, byte_offset={byte_offset:#x}")]
    MftInvalidOffset {
        lcn: i64,
        cluster_size: u64,
        byte_offset: u64,
    },
    /// SetFilePointerEx 定位到 MFT 偏移时失败（磁盘/卷状态异常）
    #[error("MFT 定位失败: offset={offset:#x}, lcn={lcn}, cluster_size={cluster_size}（错误码：{code}）")]
    MftSeekError {
        offset: u64,
        lcn: i64,
        cluster_size: u64,
        code: u32,
    },
    /// ReadFile 读取 MFT 数据失败（磁盘 I/O 错误/卷被卸载/数据损坏）
    #[error("MFT 读取失败: offset={offset:#x}, expected={expected}（错误码：{code}）")]
    MftReadError { offset: u64, expected: u64, code: u32 },
}

pub type NtfsResult<T> = Result<T, NtfsError>;

// ─── 卷设备操作 ────────────────────────────────────────────────────────────

/// 已打开的 NTFS 卷设备句柄
///
/// 用 newtype 包裹裸 `HANDLE`，把「解引用裸指针」的 `unsafe` 责任收敛到
/// [`VolumeHandle::open`] 一处。这样其余所有读卷函数都能保持为**安全**函数，
/// 既满足 `clippy::not_unsafe_ptr_arg_deref`，也让句柄在提前返回/报错时
/// 由 `Drop` 自动关闭（原先需要调用方手动 `close_handle`，错误分支极易漏掉）。
#[derive(Debug)]
pub struct VolumeHandle(HANDLE);

impl VolumeHandle {
    /// 打开卷设备（如 `\\.\C:`）
    ///
    /// **必须用卷设备路径 `\\.\X:`，不能用目录路径 `\\?\X:\`。**
    /// 目录句柄只支持元数据类 `DeviceIoControl`（如 `FSCTL_GET_NTFS_VOLUME_DATA`），
    /// 按字节偏移 `ReadFile` 会返回 `ERROR_INVALID_FUNCTION(1)`。
    ///
    /// 卷设备直读是 NTFS 的设计约束：`\\.\X:`、`$MFT`、`$Boot` 三者在
    /// 非管理员进程下均返回 `ERROR_ACCESS_DENIED(5)`。
    /// 因此失败时映射为 [`NtfsError::NeedAdministrator`] 以便上层回落 Fs 引擎。
    pub fn open(device_path: &Path) -> NtfsResult<Self> {
        let wide: Vec<u16> = device_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_DATA,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            let code = unsafe { GetLastError() };
            if code == ERROR_ACCESS_DENIED {
                return Err(NtfsError::NeedAdministrator {
                    code,
                    path: device_path.display().to_string(),
                });
            }
            return Err(last_error(format!("打开卷设备失败：{}", device_path.display())));
        }
        Ok(Self(h))
    }

    #[inline]
    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for VolumeHandle {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// 当前进程是否有权限直读该卷的 MFT（用于引擎自动选择的可用性预检）
pub fn can_read_mft(path: &Path) -> bool {
    VolumeHandle::open(Path::new(&path_to_volume_device(path))).is_ok()
}

// ─── NTFS 卷信息 ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VolumeInfo {
    pub mft_start_lcn: i64,
    pub bytes_per_sector: u32,
    pub sectors_per_cluster: u32,
    pub bytes_per_record: u32,
    pub mft_valid_data_length: i64,
    pub volume_serial_number: i64,
}

impl VolumeInfo {
    pub fn bytes_per_cluster(&self) -> u64 {
        self.bytes_per_sector as u64 * self.sectors_per_cluster as u64
    }
    pub fn mft_byte_offset(&self) -> u64 {
        let lcn = self.mft_start_lcn as u64;
        let cluster_size = self.bytes_per_cluster();
        lcn.checked_mul(cluster_size).unwrap_or(0)
    }
    pub fn mft_max_bytes(&self) -> u64 {
        let valid = self.mft_valid_data_length.max(0) as u64;
        valid / self.bytes_per_record as u64 * self.bytes_per_record as u64
    }
    /// 返回完整的 LCN 诊断字段（用于错误日志追踪）
    pub fn debug_lcn(&self) -> String {
        format!(
            "mft_start_lcn={}, bytes_per_sector={}, sectors_per_cluster={}, \
             cluster_size={}, byte_offset={:#x}, valid_data_len={}",
            self.mft_start_lcn,
            self.bytes_per_sector,
            self.sectors_per_cluster,
            self.bytes_per_cluster(),
            self.mft_byte_offset(),
            self.mft_valid_data_length,
        )
    }
}

// ─── NTFS 引导扇区 ──────────────────────────────────────────────────────────

/// `NTFS_BOOT_SECTOR` 关键字段偏移（字节）
///
/// 引导扇区是卷上偏移 0 处的固定结构，字段偏移由 NTFS 格式规范固定，
/// 比 `FSCTL_GET_NTFS_VOLUME_DATA` 的返回布局可靠（后者的字段偏移与
/// MSDN 文档不符，实测在 Windows 11 上取到垃圾值）。
mod boot {
    /// OEM ID，必须是 `b"NTFS    "`（8 字节）
    pub const OEM_ID: usize = 0x03;
    /// 每扇区字节数（u16）
    pub const BYTES_PER_SECTOR: usize = 0x0B;
    /// 每簇扇区数（u8）
    pub const SECTORS_PER_CLUSTER: usize = 0x0D;
    /// $MFT 起始簇号 LCN（u64）
    pub const MFT_START_LCN: usize = 0x30;
    /// 每条 MFT 记录的簇数（i8，见 [`BootGeometry::decode_record_size`]）
    pub const BYTES_PER_MFT_RECORD: usize = 0x40;
    /// 解析所需的最小长度
    pub const MIN_LEN: usize = 0x48;
}

/// 引导扇区中与 MFT 直读相关的几何信息
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootGeometry {
    pub bytes_per_sector: u32,
    pub sectors_per_cluster: u32,
    pub mft_start_lcn: u64,
    pub bytes_per_mft_record: u32,
}

impl BootGeometry {
    pub fn bytes_per_cluster(&self) -> u32 {
        self.bytes_per_sector * self.sectors_per_cluster
    }

    /// 解码偏移 `0x40` 的 `clusters_per_mft_record`（i8）
    ///
    /// NTFS 的编码约定：正数表示「每条记录占多少簇」，负数表示
    /// 「记录大小是 2^(-v) 字节」。NTFS 实际使用 -10（即 1024 字节）。
    fn decode_record_size(raw: i8, cluster_size: u32) -> Option<u32> {
        let size = if raw > 0 {
            (raw as u32).checked_mul(cluster_size)?
        } else if raw < 0 {
            // 先转成 i32 再取负：`-raw` 会让 i8::MIN(-128) 在 debug 下溢出 panic
            1u32.checked_shl((-(raw as i32)) as u32)?
        } else {
            return None;
        };
        // 记录大小必须落在 NTFS 允许的区间内，且是 2 的幂
        (size.is_power_of_two() && (256..=65536).contains(&size)).then_some(size)
    }
}

/// 解析 NTFS 引导扇区
pub fn parse_boot_sector(buf: &[u8]) -> NtfsResult<BootGeometry> {
    if buf.len() < boot::MIN_LEN {
        return Err(NtfsError::InsufficientData {
            needed: boot::MIN_LEN,
            actual: buf.len(),
        });
    }
    if &buf[boot::OEM_ID..boot::OEM_ID + 8] != b"NTFS    " {
        return Err(NtfsError::NotNtfs);
    }

    let bytes_per_sector = u16::from_le_bytes([buf[boot::BYTES_PER_SECTOR], buf[boot::BYTES_PER_SECTOR + 1]]) as u32;
    let sectors_per_cluster = buf[boot::SECTORS_PER_CLUSTER] as u32;
    let mft_start_lcn = u64::from_le_bytes(buf[boot::MFT_START_LCN..boot::MFT_START_LCN + 8].try_into().unwrap());

    // 扇区/簇大小必须是 2 的幂且在合理区间，否则引导扇区不可信
    if !(512..=4096).contains(&bytes_per_sector) || !bytes_per_sector.is_power_of_two() {
        return Err(NtfsError::InvalidBootGeometry {
            value: bytes_per_sector,
        });
    }
    if sectors_per_cluster == 0 || sectors_per_cluster > 128 || !sectors_per_cluster.is_power_of_two() {
        return Err(NtfsError::InvalidBootGeometry {
            value: sectors_per_cluster,
        });
    }
    let cluster_size = bytes_per_sector * sectors_per_cluster;
    let record_size_raw = buf[boot::BYTES_PER_MFT_RECORD] as i8;
    let bytes_per_mft_record =
        BootGeometry::decode_record_size(record_size_raw, cluster_size).ok_or(NtfsError::InvalidBootGeometry {
            value: record_size_raw as u32,
        })?;

    Ok(BootGeometry {
        bytes_per_sector,
        sectors_per_cluster,
        mft_start_lcn,
        bytes_per_mft_record,
    })
}

/// 在卷设备句柄上按绝对字节偏移读取指定长度
fn read_at(handle: &VolumeHandle, offset: u64, len: u32) -> NtfsResult<Vec<u8>> {
    let mut buf = vec![0u8; len as usize];
    let mut read: u32 = 0;
    unsafe {
        if SetFilePointerEx(handle.raw(), offset as i64, std::ptr::null_mut(), 0) == 0 {
            return Err(last_error(format!("卷定位失败：offset={offset:#x}")));
        }
        if ReadFile(handle.raw(), buf.as_mut_ptr(), len, &mut read, std::ptr::null_mut()) == 0 {
            return Err(last_error(format!("卷读取失败：offset={offset:#x}, len={len}")));
        }
    }
    buf.truncate(read as usize);
    Ok(buf)
}

/// 获取 NTFS 卷信息
///
/// **几何信息来自引导扇区**（`bytes_per_sector` / `sectors_per_cluster` /
/// `$MFT` 起始 LCN / MFT 记录大小），因为这四项在 NTFS 引导扇区里有固定偏移，
/// 可以直接读出并校验。
///
/// 仅 `MftValidDataLength` 与卷序列号取自 `FSCTL_GET_NTFS_VOLUME_DATA`：
/// 前者不在引导扇区里，后者仅用于日志追踪。
pub fn get_volume_info(handle: &VolumeHandle) -> NtfsResult<VolumeInfo> {
    let boot = read_at(handle, 0, 512)?;
    let geo = parse_boot_sector(&boot).map_err(|e| {
        tracing::error!(target: "treesize::ntfs", "引导扇区解析失败：{e}");
        e
    })?;

    // FSCTL_GET_NTFS_VOLUME_DATA：只取 MftValidDataLength(56) 与卷序列号(0)
    let mut buf = vec![0u8; 104];
    let mut ret: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle.raw(),
            FSCTL_GET_NTFS_VOLUME_DATA,
            std::ptr::null(),
            0,
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error("FSCTL_GET_NTFS_VOLUME_DATA 失败"));
    }
    buf.truncate(ret as usize);
    if buf.len() < 64 {
        return Err(NtfsError::InsufficientData {
            needed: 64,
            actual: buf.len(),
        });
    }
    let r = |off: usize| -> i64 { i64::from_le_bytes(buf[off..off + 8].try_into().unwrap()) };

    Ok(VolumeInfo {
        mft_start_lcn: geo.mft_start_lcn as i64,
        mft_valid_data_length: r(56),
        volume_serial_number: r(0),
        bytes_per_sector: geo.bytes_per_sector,
        sectors_per_cluster: geo.sectors_per_cluster,
        bytes_per_record: geo.bytes_per_mft_record,
    })
}

/// 提取路径所属卷的盘符大写字母（如 "C:\Users" → 'C'）
fn volume_letter(path: &Path) -> char {
    let s = path.to_string_lossy();
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        (b[0] as char).to_ascii_uppercase()
    } else {
        'C'
    }
}

/// 卷设备路径（如 "C:\Users" → `\\.\C:`）
///
/// 这是 MFT 直读**唯一**可用的路径形式；`\\?\C:\` 只能做元数据 FSCTL。
pub fn path_to_volume_device(path: &Path) -> OsString {
    OsString::from(format!("\\\\.\\{}:", volume_letter(path)))
}

// ─── MFT 原始数据读取 ──────────────────────────────────────────────────────

pub fn read_mft_raw(handle: &VolumeHandle, vol: &VolumeInfo, max_records: u64) -> NtfsResult<Vec<u8>> {
    let total = (max_records * vol.bytes_per_record as u64).min(vol.mft_max_bytes());
    if total == 0 {
        return Ok(Vec::new());
    }

    let offset = vol.mft_byte_offset();

    tracing::debug!(
        target: "treesize::ntfs",
        "read_mft_raw: max_records={}, total={}, offset={:#x}, lcn={}, cluster_size={}",
        max_records,
        total,
        offset,
        vol.mft_start_lcn,
        vol.bytes_per_cluster(),
    );

    if offset == 0 {
        tracing::error!(
            target: "treesize::ntfs",
            "MFT 字节偏移无效: {}",
            vol.debug_lcn(),
        );
        return Err(NtfsError::MftInvalidOffset {
            lcn: vol.mft_start_lcn,
            cluster_size: vol.bytes_per_cluster(),
            byte_offset: offset,
        });
    }

    let mut buf = vec![0u8; total as usize];
    let mut read: u32 = 0;
    unsafe {
        if SetFilePointerEx(handle.raw(), offset as i64, std::ptr::null_mut(), 0) == 0 {
            let err_code = GetLastError();
            tracing::error!(
                target: "treesize::ntfs",
                "MFT 定位失败: offset={:#x}, total={}, error_code={}, {}",
                offset, total, err_code, vol.debug_lcn(),
            );
            return Err(NtfsError::MftSeekError {
                offset,
                lcn: vol.mft_start_lcn,
                cluster_size: vol.bytes_per_cluster(),
                code: err_code,
            });
        }
        if ReadFile(
            handle.raw(),
            buf.as_mut_ptr(),
            total as u32,
            &mut read,
            std::ptr::null_mut(),
        ) == 0
        {
            let err_code = GetLastError();
            tracing::error!(
                target: "treesize::ntfs",
                "MFT 读取失败: offset={:#x}, expected_bytes={}, actual_read={}, error_code={}, {}",
                offset, total, read, err_code, vol.debug_lcn(),
            );
            return Err(NtfsError::MftReadError {
                offset,
                expected: total,
                code: err_code,
            });
        }
    }
    buf.truncate(read as usize);
    Ok(buf)
}

// ─── MFT 记录解析 ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MftRecordHeader {
    pub record_number: u64,
    pub is_in_use: bool,
    pub is_directory: bool,
    pub sequence_number: u16,
    pub link_count: u16,
    pub bytes_in_use: u32,
    pub attribute_offset: u16,
    pub usa_offset: u16,
    pub usa_count: u16,
}

pub fn parse_record_header(data: &[u8]) -> NtfsResult<MftRecordHeader> {
    if data.len() < 48 {
        return Err(NtfsError::InsufficientData {
            needed: 48,
            actual: data.len(),
        });
    }
    if &data[0..4] != b"FILE" {
        return Err(NtfsError::InvalidMftMagic {
            record: u64::MAX,
            magic: [data[0], data[1], data[2], data[3]],
        });
    }
    let flags = u16::from_le_bytes([data[22], data[23]]);
    Ok(MftRecordHeader {
        usa_offset: u16::from_le_bytes([data[4], data[5]]),
        usa_count: u16::from_le_bytes([data[6], data[7]]),
        sequence_number: u16::from_le_bytes([data[16], data[17]]),
        link_count: u16::from_le_bytes([data[18], data[19]]),
        attribute_offset: u16::from_le_bytes([data[20], data[21]]),
        is_in_use: (flags & 0x01) != 0,
        is_directory: (flags & 0x02) != 0,
        bytes_in_use: u32::from_le_bytes([data[24], data[25], data[26], data[27]]),
        record_number: u32::from_le_bytes([data[44], data[45], data[46], data[47]]) as u64,
    })
}

/// 应用 Fixup（更新序列号修复）
pub fn apply_fixup(rec: &mut [u8], usa_off: u16, usa_cnt: u16, sector_size: u16) {
    if usa_cnt < 2 {
        return;
    }
    for i in 1..usa_cnt {
        let fix = (usa_off + i * 2) as usize;
        if fix + 2 > rec.len() {
            break;
        }
        let val = u16::from_le_bytes([rec[fix], rec[fix + 1]]);
        let se = (i as u16 * sector_size - 2) as usize;
        if se + 2 > rec.len() {
            break;
        }
        rec[se..se + 2].copy_from_slice(&val.to_le_bytes());
    }
}

// ─── MFT 属性解析 ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeType {
    StdInfo = 0x10,
    AttrList = 0x20,
    FileName = 0x30,
    Data = 0x80,
}
impl AttributeType {
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0x10 => Some(Self::StdInfo),
            0x20 => Some(Self::AttrList),
            0x30 => Some(Self::FileName),
            0x80 => Some(Self::Data),
            _ => None,
        }
    }
}

pub struct AttributeHeader {
    pub type_code: u32,
    pub total_length: u32,
    pub non_resident: bool,
    pub name_length: u8,
}

pub fn parse_attr_header(data: &[u8], off: usize) -> NtfsResult<Option<(AttributeHeader, usize)>> {
    if off + 4 > data.len() {
        return Ok(None);
    }
    let tc = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
    if tc == 0xFFFFFFFF {
        return Ok(None);
    }
    if off + 16 > data.len() {
        return Ok(None);
    }
    let tl = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap());
    if tl == 0 {
        return Ok(None);
    }
    Ok(Some((
        AttributeHeader {
            type_code: tc,
            total_length: tl,
            non_resident: data[off + 8] != 0,
            name_length: data[off + 9],
        },
        off + tl as usize,
    )))
}

fn get_resident_value<'a>(data: &'a [u8], attr_start: usize, hdr: &AttributeHeader) -> NtfsResult<&'a [u8]> {
    if hdr.total_length < 24 {
        return Err(NtfsError::InsufficientData {
            needed: 24,
            actual: hdr.total_length as usize,
        });
    }
    let vl = u32::from_le_bytes(data[attr_start + 16..attr_start + 20].try_into().unwrap()) as usize;
    let vo = u16::from_le_bytes(data[attr_start + 20..attr_start + 22].try_into().unwrap()) as usize;
    if attr_start + vo + vl > data.len() {
        return Err(NtfsError::InsufficientData {
            needed: attr_start + vo + vl,
            actual: data.len(),
        });
    }
    Ok(&data[attr_start + vo..attr_start + vo + vl])
}

#[derive(Debug, Clone)]
pub struct FileNameAttr {
    pub parent_record: u64,
    pub file_size: u64,
    pub name: String,
    pub name_namespace: u8,
}

pub fn parse_file_name(value: &[u8]) -> NtfsResult<FileNameAttr> {
    if value.len() < 66 {
        return Err(NtfsError::InsufficientData {
            needed: 66,
            actual: value.len(),
        });
    }
    let pref = u64::from_le_bytes(value[0..8].try_into().unwrap());
    let fn_len = value[64] as usize;
    let ns = value[65];
    let mut utf16 = Vec::with_capacity(fn_len);
    for i in 0..fn_len {
        utf16.push(u16::from_le_bytes([value[66 + i * 2], value[66 + i * 2 + 1]]));
    }
    let name = String::from_utf16(&utf16).map_err(|_| NtfsError::InvalidFileNameUtf16)?;
    let fsize = u64::from_le_bytes(value[48..56].try_into().unwrap());
    Ok(FileNameAttr {
        parent_record: pref & 0x0000_FFFF_FFFF_FFFF,
        file_size: fsize,
        name,
        name_namespace: ns,
    })
}

// ─── 完整 MFT 记录解析 ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ParsedMftRecord {
    pub header: MftRecordHeader,
    pub file_names: Vec<FileNameAttr>,
    pub data_real_size: Option<u64>,
}

pub fn parse_mft_record(data: &[u8]) -> NtfsResult<Option<ParsedMftRecord>> {
    let header = match parse_record_header(data) {
        Ok(h) => h,
        Err(NtfsError::InvalidMftMagic { .. }) => return Ok(None),
        Err(e) => return Err(e),
    };
    if !header.is_in_use {
        return Ok(None);
    }

    let mut record = data.to_vec();
    if header.usa_offset > 0 && header.usa_count > 1 {
        apply_fixup(&mut record, header.usa_offset, header.usa_count, 512);
    }
    let rec = record.as_slice();
    let mut result = ParsedMftRecord {
        header,
        file_names: Vec::new(),
        data_real_size: None,
    };

    let mut off = result.header.attribute_offset as usize;
    loop {
        let parsed = parse_attr_header(rec, off)?;
        let (ah, next) = match parsed {
            Some(p) => p,
            None => break,
        };
        if next <= off {
            break;
        }

        let attr_body_start = off;
        match AttributeType::from_code(ah.type_code) {
            Some(AttributeType::FileName) if !ah.non_resident => {
                if let Ok(val) = get_resident_value(rec, attr_body_start, &ah) {
                    if let Ok(fa) = parse_file_name(val) {
                        result.file_names.push(fa);
                    }
                }
            },
            Some(AttributeType::Data) if ah.non_resident => {
                if ah.total_length >= 64 {
                    let rs = u64::from_le_bytes(rec[attr_body_start + 48..attr_body_start + 56].try_into().unwrap());
                    result.data_real_size = Some(rs);
                }
            },
            Some(AttributeType::Data) => {
                if let Ok(val) = get_resident_value(rec, attr_body_start, &ah) {
                    result.data_real_size = Some(val.len() as u64);
                }
            },
            _ => {},
        }
        off = next;
    }
    Ok(Some(result))
}

/// 在 `&mut [u8]` 上原地解析单条 MFT 记录（避免 `to_vec()` 克隆）
///
/// 与 `parse_mft_record` 功能相同，但 fixup 直接在输入切片上执行，
/// 省去每次 1KB 的堆分配。被 `parse_mft_records` 内部调用。
fn parse_mft_record_mut(data: &mut [u8]) -> NtfsResult<Option<ParsedMftRecord>> {
    let header = match parse_record_header(data) {
        Ok(h) => h,
        Err(NtfsError::InvalidMftMagic { .. }) => return Ok(None),
        Err(e) => return Err(e),
    };
    if !header.is_in_use {
        return Ok(None);
    }

    // 原地应用 Fixup，无需克隆整个记录
    if header.usa_offset > 0 && header.usa_count > 1 {
        apply_fixup(data, header.usa_offset, header.usa_count, 512);
    }

    let mut result = ParsedMftRecord {
        header,
        file_names: Vec::new(),
        data_real_size: None,
    };

    let mut off = result.header.attribute_offset as usize;
    loop {
        let parsed = parse_attr_header(data, off)?;
        let (ah, next) = match parsed {
            Some(p) => p,
            None => break,
        };
        if next <= off {
            break;
        }

        let attr_body_start = off;
        match AttributeType::from_code(ah.type_code) {
            Some(AttributeType::FileName) if !ah.non_resident => {
                if let Ok(val) = get_resident_value(data, attr_body_start, &ah) {
                    if let Ok(fa) = parse_file_name(val) {
                        result.file_names.push(fa);
                    }
                }
            },
            Some(AttributeType::Data) if ah.non_resident => {
                if ah.total_length >= 64 {
                    let rs = u64::from_le_bytes(data[attr_body_start + 48..attr_body_start + 56].try_into().unwrap());
                    result.data_real_size = Some(rs);
                }
            },
            Some(AttributeType::Data) => {
                if let Ok(val) = get_resident_value(data, attr_body_start, &ah) {
                    result.data_real_size = Some(val.len() as u64);
                }
            },
            _ => {},
        }
        off = next;
    }
    Ok(Some(result))
}

/// 解析 MFT 原始数据为 `ParsedMftRecord` 列表
///
/// 使用 rayon 并行解析每条 MFT 记录。每条记录在 `par_chunks_mut` 分块中
/// 独立完成 fixup（原地）和属性解析，无需 `to_vec()` 克隆。
/// 对于百万级 MFT 记录，相比旧版本（逐条克隆 + 串行）可大幅减少分配和延迟。
pub fn parse_mft_records(raw: &mut [u8], bpr: u32) -> NtfsResult<Vec<ParsedMftRecord>> {
    let rs = bpr as usize;
    let n = raw.len() / rs;
    if n == 0 {
        return Ok(Vec::new());
    }

    let out: Vec<ParsedMftRecord> = raw
        .par_chunks_mut(rs)
        .take(n)
        .filter_map(|chunk| match parse_mft_record_mut(chunk) {
            Ok(Some(r)) => Some(r),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(target: "treesize::ntfs", "MFT 记录解析跳过：{e}");
                None
            },
        })
        .collect();

    Ok(out)
}

// ─── MFT 大小映射表（用于 USN 扫描器补充文件大小） ──────────────────────────

/// 从已解析的 MFT 记录构建 记录号→文件大小(u64) 的 HashMap
///
/// 用于 USN 扫描器：USN Journal 记录不包含文件大小信息，
/// 需要读取 MFT 来获取每个文件的实际大小。
/// key = MFT 记录号, value = 文件大小（字节）
pub fn build_size_map(parsed: &[ParsedMftRecord]) -> std::collections::HashMap<u64, u64> {
    parsed
        .iter()
        .map(|rec| (rec.header.record_number, rec.data_real_size.unwrap_or(0)))
        .collect()
}

// ─── MFT 条目与树构建 ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MftFileEntry {
    pub record_number: u64,
    pub is_directory: bool,
    pub name: String,
    pub parent_record: u64,
    pub size: u64,
}

pub fn build_tree(records: &[ParsedMftRecord]) -> NtfsResult<Vec<MftFileEntry>> {
    let mut entries = Vec::with_capacity(records.len());
    for rec in records {
        let best = rec.file_names.iter().max_by_key(|n| match n.name_namespace {
            1 => 2,
            3 => 3,
            2 => 1,
            _ => 0,
        });
        let name = best.map(|n| n.name.clone()).unwrap_or_default();
        let parent = best.map(|n| n.parent_record).unwrap_or(0);
        let sz = rec.data_real_size.unwrap_or(0);
        entries.push(MftFileEntry {
            record_number: rec.header.record_number,
            is_directory: rec.header.is_directory,
            name,
            parent_record: parent,
            size: sz,
        });
    }
    Ok(entries)
}

// ─── USN Journal ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UsnJournalState {
    pub usn_journal_id: u64,
    pub next_usn: i64,
    pub lowest_valid_usn: i64,
    pub max_usn: i64,
}

pub fn query_usn_journal(handle: &VolumeHandle) -> NtfsResult<UsnJournalState> {
    let mut buf = vec![0u8; 56];
    let mut ret: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle.raw(),
            FSCTL_QUERY_USN_JOURNAL,
            std::ptr::null(),
            0,
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error("FSCTL_QUERY_USN_JOURNAL 失败"));
    }
    buf.truncate(ret as usize);
    if buf.len() < 56 {
        return Err(NtfsError::InsufficientData {
            needed: 56,
            actual: buf.len(),
        });
    }
    Ok(UsnJournalState {
        usn_journal_id: u64::from_le_bytes(buf[0..8].try_into().unwrap()),
        next_usn: i64::from_le_bytes(buf[8..16].try_into().unwrap()),
        lowest_valid_usn: i64::from_le_bytes(buf[24..32].try_into().unwrap()),
        max_usn: i64::from_le_bytes(buf[32..40].try_into().unwrap()),
    })
}

#[derive(Debug, Clone)]
pub struct UsnRecord {
    pub file_reference_number: u64,
    pub parent_file_reference_number: u64,
    pub usn: i64,
    pub reason: u32,
    pub file_attributes: u32,
    pub file_name: String,
    pub is_directory: bool,
}

pub fn read_usn_records(
    handle: &VolumeHandle,
    journal_id: u64,
    since_usn: i64,
    max_records: u32,
) -> NtfsResult<Vec<UsnRecord>> {
    let mut input = vec![0u8; 40];
    input[0..8].copy_from_slice(&since_usn.to_le_bytes());
    input[8..12].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    input[12..16].copy_from_slice(&0u32.to_le_bytes());
    input[16..20].copy_from_slice(&0u32.to_le_bytes());
    input[20..24].copy_from_slice(&0u32.to_le_bytes());
    input[24..32].copy_from_slice(&journal_id.to_le_bytes());
    input[32..36].copy_from_slice(&2u32.to_le_bytes());
    input[36..40].copy_from_slice(&3u32.to_le_bytes());

    let out_size = max_records as usize * 128 + 4096;
    let mut output = vec![0u8; out_size];
    let mut ret: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle.raw(),
            FSCTL_READ_USN_JOURNAL,
            input.as_ptr() as *const _,
            input.len() as u32,
            output.as_mut_ptr() as *mut _,
            output.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error("FSCTL_READ_USN_JOURNAL 失败"));
    }
    output.truncate(ret as usize);
    if output.len() < 8 {
        return Ok(Vec::new());
    }

    let mut off = 8;
    let mut recs = Vec::new();
    while off + 4 <= output.len() {
        let rl = u32::from_le_bytes(output[off..off + 4].try_into().unwrap());
        if rl == 0 || off + rl as usize > output.len() {
            break;
        }
        let s = &output[off..off + rl as usize];
        let fattr = u32::from_le_bytes(s[52..56].try_into().unwrap());
        let fn_len = u16::from_le_bytes([s[56], s[57]]) as usize;
        let fn_off = u16::from_le_bytes([s[58], s[59]]) as usize;
        let name = if fn_off + fn_len * 2 <= s.len() {
            let mut u16v = Vec::with_capacity(fn_len);
            for i in 0..fn_len {
                u16v.push(u16::from_le_bytes([s[fn_off + i * 2], s[fn_off + i * 2 + 1]]));
            }
            String::from_utf16(&u16v).unwrap_or_default()
        } else {
            String::new()
        };

        recs.push(UsnRecord {
            file_reference_number: u64::from_le_bytes(s[8..16].try_into().unwrap()),
            parent_file_reference_number: u64::from_le_bytes(s[16..24].try_into().unwrap()),
            usn: i64::from_le_bytes(s[24..32].try_into().unwrap()),
            reason: u32::from_le_bytes(s[40..44].try_into().unwrap()),
            file_attributes: fattr,
            file_name: name,
            is_directory: (fattr & 0x10) != 0,
        });
        off += rl as usize;
    }
    Ok(recs)
}

// ─── NTFS 卷检测 ──────────────────────────────────────────────────────────────

/// 检测路径所在卷是否为 NTFS 文件系统
///
/// 通过尝试发送 FSCTL_GET_NTFS_VOLUME_DATA 判断，
/// 该 IOCTL 仅 NTFS 文件系统支持。
pub fn is_ntfs_volume(path: &Path) -> bool {
    // 用 GetVolumeInformationW 判定文件系统类型：它只需要普通路径权限，
    // 而读引导扇区/开卷设备都要管理员权限，不能用来做「是不是 NTFS」的探测。
    volume_filesystem(path).is_some_and(|fs| fs.eq_ignore_ascii_case("NTFS"))
}

/// 查询卷的文件系统名（如 "NTFS" / "exFAT" / "ReFS"）
pub fn volume_filesystem(path: &Path) -> Option<String> {
    let root = format!("{}:\\", volume_letter(path));
    let wide: Vec<u16> = OsString::from(&root).encode_wide().chain(std::iter::once(0)).collect();
    let mut name = vec![0u16; 32];
    let ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let len = name.iter().position(|&c| c == 0)?;
    Some(String::from_utf16_lossy(&name[..len]))
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_record_header() {
        let mut d = vec![0u8; 48];
        d[0..4].copy_from_slice(b"FILE");
        d[20..22].copy_from_slice(&[0x38, 0x00]);
        d[22..24].copy_from_slice(&[0x01, 0x00]);
        d[24..28].copy_from_slice(&1024u32.to_le_bytes());
        d[44..48].copy_from_slice(&5u32.to_le_bytes());
        let h = parse_record_header(&d).unwrap();
        assert!(h.is_in_use);
        assert_eq!(h.record_number, 5);
    }

    #[test]
    fn test_parse_file_name() {
        let mut d = vec![0u8; 74];
        d[0..8].copy_from_slice(&5u64.to_le_bytes());
        d[48..56].copy_from_slice(&500u64.to_le_bytes());
        d[64] = 4;
        d[65] = 1;
        d[66..74].copy_from_slice(&[b't', 0, b'e', 0, b's', 0, b't', 0]);
        let a = parse_file_name(&d).unwrap();
        assert_eq!(a.parent_record, 5);
        assert_eq!(a.name, "test");
    }

    // ── 引导扇区解析 ──

    /// 构造一份最小可用的 NTFS 引导扇区
    fn fake_boot(bps: u16, spc: u8, mft_lcn: u64, rec_code: i8) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[boot::OEM_ID..boot::OEM_ID + 8].copy_from_slice(b"NTFS    ");
        b[boot::BYTES_PER_SECTOR..boot::BYTES_PER_SECTOR + 2].copy_from_slice(&bps.to_le_bytes());
        b[boot::SECTORS_PER_CLUSTER] = spc;
        b[boot::MFT_START_LCN..boot::MFT_START_LCN + 8].copy_from_slice(&mft_lcn.to_le_bytes());
        b[boot::BYTES_PER_MFT_RECORD] = rec_code as u8;
        b
    }

    #[test]
    fn test_parse_boot_sector_standard_4k_cluster() {
        // NTFS 实际布局：512B/扇区、8扇区/簇、记录大小 2^10 = 1024
        let g = parse_boot_sector(&fake_boot(512, 8, 786_432, -10)).unwrap();
        assert_eq!(g.bytes_per_sector, 512);
        assert_eq!(g.sectors_per_cluster, 8);
        assert_eq!(g.bytes_per_cluster(), 4096);
        assert_eq!(g.mft_start_lcn, 786_432);
        assert_eq!(g.bytes_per_mft_record, 1024);
    }

    #[test]
    fn test_parse_boot_sector_large_cluster() {
        // 64 KB 簇（exFAT 风格偏移仅用于验证解码逻辑）
        let g = parse_boot_sector(&fake_boot(512, 128, 4, -10)).unwrap();
        assert_eq!(g.bytes_per_cluster(), 65536);
        assert_eq!(g.bytes_per_mft_record, 1024);
    }

    #[test]
    fn test_parse_boot_sector_record_size_as_clusters() {
        // 正数编码：每条记录占 2 簇 = 2 * 4096 = 8192 字节
        let g = parse_boot_sector(&fake_boot(512, 8, 4, 2)).unwrap();
        assert_eq!(g.bytes_per_mft_record, 8192);
    }

    #[test]
    fn test_parse_boot_sector_rejects_non_ntfs() {
        let mut b = fake_boot(512, 8, 4, -10);
        b[boot::OEM_ID..boot::OEM_ID + 8].copy_from_slice(b"MSDOS5.0");
        assert!(matches!(parse_boot_sector(&b), Err(NtfsError::NotNtfs)));
    }

    #[test]
    fn test_parse_boot_sector_rejects_bad_geometry() {
        // 每扇区字节数不是 2 的幂
        assert!(matches!(
            parse_boot_sector(&fake_boot(777, 8, 4, -10)),
            Err(NtfsError::InvalidBootGeometry { .. })
        ));
        // 每簇扇区数为 0
        assert!(matches!(
            parse_boot_sector(&fake_boot(512, 0, 4, -10)),
            Err(NtfsError::InvalidBootGeometry { .. })
        ));
        // 记录大小编码为 0（非法）
        assert!(matches!(
            parse_boot_sector(&fake_boot(512, 8, 4, 0)),
            Err(NtfsError::InvalidBootGeometry { .. })
        ));
        // 记录大小解出 2^30，远超 64 KB 上限
        assert!(matches!(
            parse_boot_sector(&fake_boot(512, 8, 4, -30)),
            Err(NtfsError::InvalidBootGeometry { .. })
        ));
    }

    #[test]
    fn test_parse_boot_sector_rejects_short_buffer() {
        assert!(matches!(
            parse_boot_sector(&[0u8; 0x20]),
            Err(NtfsError::InsufficientData { .. })
        ));
    }

    #[test]
    fn test_decode_record_size_bounds() {
        // -128 取负不得溢出
        assert_eq!(BootGeometry::decode_record_size(-128, 4096), None);
        assert_eq!(BootGeometry::decode_record_size(-9, 4096), Some(512));
        assert_eq!(BootGeometry::decode_record_size(-16, 4096), Some(65536));
        // 正数编码下溢保护
        assert_eq!(BootGeometry::decode_record_size(127, 4096), None);
    }
}
