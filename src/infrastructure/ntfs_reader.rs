//! NTFS 底层读取模块（仅 Windows）
//!
//! 提供 MFT 直接读取、MFT 记录解析、USN Journal 查询等底层函数。
//! 本模块自行定义 windows-sys 0.59 中缺失的 FSCTL 常量和外联函数。

#![cfg(target_os = "windows")]

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use thiserror::Error;

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, INVALID_HANDLE_VALUE,
};

// ─── 手动定义的 IOCTL／常量 ──────────────────────────────────────────────

const FSCTL_GET_NTFS_VOLUME_DATA: u32 = 0x00090064;
const FSCTL_QUERY_USN_JOURNAL: u32 = 0x000900ec;
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
        hfile: HANDLE, lpbuffer: *mut u8, nnumberofbytestoread: u32,
        lpnumberofbytesread: *mut u32, lpoverlapped: *mut std::ffi::c_void,
    ) -> i32;
    fn SetFilePointerEx(
        hfile: HANDLE, lidistancetomove: i64, lpnewfilepointer: *mut i64, dwmovemethod: u32,
    ) -> i32;
    fn DeviceIoControl(
        hdevice: HANDLE, dwiocontrolcode: u32,
        lpinbuffer: *const std::ffi::c_void, ninbuffersize: u32,
        lpoutbuffer: *mut std::ffi::c_void, noutbuffersize: u32,
        lpbytesreturned: *mut u32, lpoverlapped: *mut std::ffi::c_void,
    ) -> i32;
    fn GetLastError() -> u32;
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
    #[error("USN Journal 不存在")]
    NoUsnJournal,
    #[error("文件名包含无效 UTF-16")]
    InvalidFileNameUtf16,
    #[error("NTFS I/O 错误：{0}")]
    Io(#[from] std::io::Error),
    /// MFT 起始 LCN 计算出的字节偏移为 0（lcn 负值/乘法溢出/元数据损坏）
    #[error(
        "MFT 字节偏移无效: lcn={lcn}, cluster_size={cluster_size}, byte_offset={byte_offset:#x}"
    )]
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
    MftReadError {
        offset: u64,
        expected: u64,
        code: u32,
    },
}

pub type NtfsResult<T> = Result<T, NtfsError>;

// ─── 卷设备操作 ────────────────────────────────────────────────────────────

/// 打开卷设备（如 `\\?\C:\`）
pub fn open_volume(path: &Path) -> NtfsResult<HANDLE> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let h = unsafe {
        CreateFileW(wide.as_ptr(), FILE_READ_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut())
    };
    if h == INVALID_HANDLE_VALUE { Err(last_error(format!("打开卷失败：{}", path.display()))) }
    else { Ok(h) }
}

/// 关闭句柄
pub fn close_handle(handle: HANDLE) {
    if handle != INVALID_HANDLE_VALUE { unsafe { CloseHandle(handle); } }
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

/// 获取 NTFS 卷信息（FSCTL_GET_NTFS_VOLUME_DATA）
///
/// NTFS_VOLUME_DATA_BUFFER 结构布局：
/// - VolumeSerialNumber: 0 (i64)
/// - NumberSectors: 8 (i64)
/// - SectorsPerCluster: 16 (i64)
/// - BytesPerSector: 24 (i64)
/// - BytesPerCluster: 32 (i64)
/// - BytesPerFileRecordSegment: 40 (i64)
/// - ClustersPerFileRecordSegment: 48 (i64)
/// - MftValidDataLength: 56 (i64)
/// - MftStartLcn: 64 (i64)
/// - Mft2StartLcn: 72 (i64)
/// - MftZoneStart: 80 (i64)
/// - MftZoneEnd: 88 (i64)
pub fn get_volume_info(handle: HANDLE) -> NtfsResult<VolumeInfo> {
    let mut buf = vec![0u8; 104];
    let mut ret: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(handle, FSCTL_GET_NTFS_VOLUME_DATA,
            std::ptr::null(), 0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut ret, std::ptr::null_mut())
    };
    if ok == 0 { return Err(last_error("FSCTL_GET_NTFS_VOLUME_DATA 失败")); }
    buf.truncate(ret as usize);
    if buf.len() < 100 { return Err(NtfsError::InsufficientData { needed: 100, actual: buf.len() }); }

    let r = |off: usize| -> i64 { i64::from_le_bytes(buf[off..off+8].try_into().unwrap()) };

    Ok(VolumeInfo {
        mft_start_lcn: r(64),
        mft_valid_data_length: r(56),
        volume_serial_number: r(0),
        bytes_per_sector: r(24) as u32,
        sectors_per_cluster: r(16) as u32,
        bytes_per_record: r(40) as u32,
    })
}

/// 卷路径（如 "C:\Users" → "\\?\C:\"）
pub fn path_to_volume_path(path: &Path) -> OsString {
    let s = path.to_string_lossy();
    let letter = if s.len() >= 2 && s.as_bytes()[1] == b':' { s[..1].to_uppercase() } else { "C".to_string() };
    OsString::from(format!("\\\\?\\{}:\\", letter))
}

// ─── MFT 原始数据读取 ──────────────────────────────────────────────────────

pub fn read_mft_raw(handle: HANDLE, vol: &VolumeInfo, max_records: u64) -> NtfsResult<Vec<u8>> {
    let total = (max_records * vol.bytes_per_record as u64).min(vol.mft_max_bytes());
    if total == 0 { return Ok(Vec::new()); }
    
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
        if SetFilePointerEx(handle, offset as i64, std::ptr::null_mut(), 0) == 0 {
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
        if ReadFile(handle, buf.as_mut_ptr(), total as u32, &mut read, std::ptr::null_mut()) == 0 {
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
    if data.len() < 48 { return Err(NtfsError::InsufficientData { needed: 48, actual: data.len() }); }
    if &data[0..4] != b"FILE" {
        return Err(NtfsError::InvalidMftMagic { record: u64::MAX, magic: [data[0], data[1], data[2], data[3]] });
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
    if usa_cnt < 2 { return; }
    for i in 1..usa_cnt {
        let fix = (usa_off + i * 2) as usize;
        if fix + 2 > rec.len() { break; }
        let val = u16::from_le_bytes([rec[fix], rec[fix + 1]]);
        let se = (i as u16 * sector_size - 2) as usize;
        if se + 2 > rec.len() { break; }
        rec[se..se+2].copy_from_slice(&val.to_le_bytes());
    }
}

// ─── MFT 属性解析 ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeType { StdInfo = 0x10, AttrList = 0x20, FileName = 0x30, Data = 0x80 }
impl AttributeType {
    pub fn from_code(code: u32) -> Option<Self> {
        match code { 0x10 => Some(Self::StdInfo), 0x20 => Some(Self::AttrList), 0x30 => Some(Self::FileName), 0x80 => Some(Self::Data), _ => None }
    }
}

pub struct AttributeHeader { pub type_code: u32, pub total_length: u32, pub non_resident: bool, pub name_length: u8 }

pub fn parse_attr_header(data: &[u8], off: usize) -> NtfsResult<Option<(AttributeHeader, usize)>> {
    if off + 4 > data.len() { return Ok(None); }
    let tc = u32::from_le_bytes(data[off..off+4].try_into().unwrap());
    if tc == 0xFFFFFFFF { return Ok(None); }
    if off + 16 > data.len() { return Ok(None); }
    let tl = u32::from_le_bytes(data[off+4..off+8].try_into().unwrap());
    if tl == 0 { return Ok(None); }
    Ok(Some((AttributeHeader { type_code: tc, total_length: tl, non_resident: data[off+8] != 0, name_length: data[off+9] }, off + tl as usize)))
}

fn get_resident_value<'a>(data: &'a [u8], attr_start: usize, hdr: &AttributeHeader) -> NtfsResult<&'a [u8]> {
    if hdr.total_length < 24 { return Err(NtfsError::InsufficientData { needed: 24, actual: hdr.total_length as usize }); }
    let vl = u32::from_le_bytes(data[attr_start+16..attr_start+20].try_into().unwrap()) as usize;
    let vo = u16::from_le_bytes(data[attr_start+20..attr_start+22].try_into().unwrap()) as usize;
    if attr_start + vo + vl > data.len() { return Err(NtfsError::InsufficientData { needed: attr_start+vo+vl, actual: data.len() }); }
    Ok(&data[attr_start+vo..attr_start+vo+vl])
}

#[derive(Debug, Clone)]
pub struct FileNameAttr {
    pub parent_record: u64,
    pub file_size: u64,
    pub name: String,
    pub name_namespace: u8,
}

pub fn parse_file_name(value: &[u8]) -> NtfsResult<FileNameAttr> {
    if value.len() < 66 { return Err(NtfsError::InsufficientData { needed: 66, actual: value.len() }); }
    let pref = u64::from_le_bytes(value[0..8].try_into().unwrap());
    let fn_len = value[64] as usize;
    let ns = value[65];
    let mut utf16 = Vec::with_capacity(fn_len);
    for i in 0..fn_len { utf16.push(u16::from_le_bytes([value[66+i*2], value[66+i*2+1]])); }
    let name = String::from_utf16(&utf16).map_err(|_| NtfsError::InvalidFileNameUtf16)?;
    let fsize = u64::from_le_bytes(value[48..56].try_into().unwrap());
    Ok(FileNameAttr { parent_record: pref & 0x0000_FFFF_FFFF_FFFF, file_size: fsize, name, name_namespace: ns })
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
    if !header.is_in_use { return Ok(None); }

    let mut record = data.to_vec();
    if header.usa_offset > 0 && header.usa_count > 1 {
        apply_fixup(&mut record, header.usa_offset, header.usa_count, 512);
    }
    let rec = record.as_slice();
    let mut result = ParsedMftRecord { header, file_names: Vec::new(), data_real_size: None };

    let mut off = result.header.attribute_offset as usize;
    loop {
        let parsed = parse_attr_header(rec, off)?;
        let (ah, next) = match parsed { Some(p) => p, None => break };
        if next <= off { break; }

        let attr_body_start = off;
        match AttributeType::from_code(ah.type_code) {
            Some(AttributeType::FileName) if !ah.non_resident => {
                if let Ok(val) = get_resident_value(rec, attr_body_start, &ah) {
                    if let Ok(fa) = parse_file_name(val) { result.file_names.push(fa); }
                }
            }
            Some(AttributeType::Data) if ah.non_resident => {
                if ah.total_length >= 64 {
                    let rs = u64::from_le_bytes(rec[attr_body_start+48..attr_body_start+56].try_into().unwrap());
                    result.data_real_size = Some(rs);
                }
            }
            Some(AttributeType::Data) => {
                if let Ok(val) = get_resident_value(rec, attr_body_start, &ah) {
                    result.data_real_size = Some(val.len() as u64);
                }
            }
            _ => {}
        }
        off = next;
    }
    Ok(Some(result))
}

pub fn parse_mft_records(raw: &[u8], bpr: u32) -> NtfsResult<Vec<ParsedMftRecord>> {
    let rs = bpr as usize;
    let n = raw.len() / rs;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let s = i * rs;
        if s + rs > raw.len() { break; }
        match parse_mft_record(&raw[s..s+rs]) {
            Ok(Some(r)) => out.push(r),
            Ok(None) => {},
            Err(e) => tracing::warn!(
                target: "treesize::ntfs",
                "MFT #{i} (byte_offset={:#x}) 跳过：{e}",
                s,
            ),
        }
    }
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
            1 => 2, 3 => 3, 2 => 1, _ => 0
        });
        let name = best.map(|n| n.name.clone()).unwrap_or_default();
        let parent = best.map(|n| n.parent_record).unwrap_or(0);
        let sz = rec.data_real_size.unwrap_or(0);
        entries.push(MftFileEntry {
            record_number: rec.header.record_number,
            is_directory: rec.header.is_directory,
            name, parent_record: parent, size: sz,
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

pub fn query_usn_journal(handle: HANDLE) -> NtfsResult<UsnJournalState> {
    let mut buf = vec![0u8; 56];
    let mut ret: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(handle, FSCTL_QUERY_USN_JOURNAL, std::ptr::null(), 0,
            buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut ret, std::ptr::null_mut())
    };
    if ok == 0 { return Err(last_error("FSCTL_QUERY_USN_JOURNAL 失败")); }
    buf.truncate(ret as usize);
    if buf.len() < 56 { return Err(NtfsError::InsufficientData { needed: 56, actual: buf.len() }); }
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

pub fn read_usn_records(handle: HANDLE, journal_id: u64, since_usn: i64, max_records: u32) -> NtfsResult<Vec<UsnRecord>> {
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
        DeviceIoControl(handle, FSCTL_READ_USN_JOURNAL,
            input.as_ptr() as *const _, input.len() as u32,
            output.as_mut_ptr() as *mut _, output.len() as u32, &mut ret, std::ptr::null_mut())
    };
    if ok == 0 { return Err(last_error("FSCTL_READ_USN_JOURNAL 失败")); }
    output.truncate(ret as usize);
    if output.len() < 8 { return Ok(Vec::new()); }

    let mut off = 8;
    let mut recs = Vec::new();
    while off + 4 <= output.len() {
        let rl = u32::from_le_bytes(output[off..off+4].try_into().unwrap());
        if rl == 0 || off + rl as usize > output.len() { break; }
        let s = &output[off..off + rl as usize];
        let fattr = u32::from_le_bytes(s[52..56].try_into().unwrap());
        let fn_len = u16::from_le_bytes([s[56], s[57]]) as usize;
        let fn_off = u16::from_le_bytes([s[58], s[59]]) as usize;
        let name = if fn_off + fn_len * 2 <= s.len() {
            let mut u16v = Vec::with_capacity(fn_len);
            for i in 0..fn_len { u16v.push(u16::from_le_bytes([s[fn_off+i*2], s[fn_off+i*2+1]])); }
            String::from_utf16(&u16v).unwrap_or_default()
        } else { String::new() };

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

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_record_header() {
        let mut d = vec![0u8; 48]; d[0..4].copy_from_slice(b"FILE");
        d[20..22].copy_from_slice(&[0x38, 0x00]);
        d[22..24].copy_from_slice(&[0x01, 0x00]);
        d[24..28].copy_from_slice(&1024u32.to_le_bytes());
        d[44..48].copy_from_slice(&5u32.to_le_bytes());
        let h = parse_record_header(&d).unwrap();
        assert!(h.is_in_use); assert_eq!(h.record_number, 5);
    }

    #[test]
    fn test_parse_file_name() {
        let mut d = vec![0u8; 74];
        d[0..8].copy_from_slice(&5u64.to_le_bytes());
        d[48..56].copy_from_slice(&500u64.to_le_bytes());
        d[64] = 4; d[65] = 1;
        d[66..74].copy_from_slice(&[b't',0,b'e',0,b's',0,b't',0]);
        let a = parse_file_name(&d).unwrap();
        assert_eq!(a.parent_record, 5);
        assert_eq!(a.name, "test");
    }
}
