//! 卷级对账：解释「扫描结果」与「磁盘属性里的已用空间」为什么对不上。
//!
//! 二者本来就不是同一个量：
//! - **卷已用空间** = 文件系统分配出去的全部空间，包含元数据、卷影副本/还原点、
//!   独占打开的系统文件（pagefile/hiberfil）、权限拒绝而读不到的目录，
//!   以及压缩/稀疏文件与硬链接在磁盘上的实际占用；
//! - **本次计入** = 遍历到的文件节点按当前口径（分配大小 / 表观大小）累加。
//!
//! 因此 C 盘常见「已用 135 GiB / 扫描 112 GiB」（不可读系统目录），
//! D 盘常见「已用 249 GiB / 扫描 257 GiB」（硬链接重复计数）。
//! 本模块只把差额**摆出来并解释**，不修正、不篡改任何扫描数值。

use std::path::Path;

use sysinfo::Disks;

/// 扫描中读不到的目录（路径 + 失败原因）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableDir {
    pub path: String,
    pub reason: String,
}

/// 一次卷根扫描的对账结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeReconciliation {
    /// 卷挂载点（规范化字符串，如 `C:` / `/`）
    pub volume_root: String,
    /// 卷总容量
    pub total_bytes: u64,
    /// 卷已用空间（总容量 − 当前用户可用空间）
    pub used_bytes: u64,
    /// 卷空闲空间
    pub free_bytes: u64,
    /// 本次扫描计入的大小
    pub accounted_bytes: u64,
    /// `used − accounted`：正 = 卷上占用但本次未计入，负 = 本次计入超过卷已用
    pub gap_bytes: i64,
    /// 大小口径：true = 表观大小，false = 分配大小
    pub apparent_size: bool,
    /// 不可读目录明细（最多 2000 条）
    pub unreadable: Vec<UnreadableDir>,
    /// 不可读目录总数（可能大于 `unreadable.len()`）
    pub total_unreadable: usize,
}

impl VolumeReconciliation {
    /// 差额绝对值
    pub fn gap_abs(&self) -> u64 { self.gap_bytes.unsigned_abs() }

    /// 本次计入占卷已用空间的百分比
    pub fn coverage_percent(&self) -> f64 {
        if self.used_bytes == 0 {
            return 0.0;
        }
        (self.accounted_bytes as f64 / self.used_bytes as f64) * 100.0
    }

    /// 差额方向的中文标签
    pub fn gap_label(&self) -> &'static str {
        if self.gap_bytes > 0 {
            "未计入"
        } else if self.gap_bytes < 0 {
            "多算"
        } else {
            "一致"
        }
    }

    /// 当前大小口径的中文标签
    pub fn basis_label(&self) -> &'static str {
        if self.apparent_size {
            "表观大小（逻辑长度）"
        } else {
            "分配大小（簇对齐）"
        }
    }
}

/// 路径规范化：统一分隔符、去掉尾部斜杠、Windows 盘符大写。
///
/// `C:\`、`C:/`、`c:` 归一化后都是 `C:`；根目录 `/` 特殊保留，
/// 否则 `trim_end_matches('/')` 会把它变成空串而无法参与前缀匹配。
pub(crate) fn normalize_path(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let trimmed = raw.trim_end_matches('/');
    let normalized = if trimmed.is_empty() && raw.starts_with('/') {
        "/"
    } else {
        trimmed
    };
    if cfg!(target_os = "windows") {
        let mut chars = normalized.chars();
        match chars.next() {
            Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
            None => String::new(),
        }
    } else {
        normalized.to_string()
    }
}

struct VolumeInfo {
    root: String,
    total: u64,
    free: u64,
    used: u64,
}

/// 找出 `path` 所在卷；多个挂载点嵌套时取最长前缀（卷挂载点优先于盘符根）。
fn volume_of(path: &Path) -> Option<VolumeInfo> {
    let target = normalize_path(path);
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<VolumeInfo> = None;

    for disk in disks.list() {
        let mount = normalize_path(disk.mount_point());
        let total = disk.total_space();
        // 空光驱 / 未就绪卷会报 0，跳过避免误匹配
        if total == 0 || mount.is_empty() || !is_same_or_child(&target, &mount) {
            continue;
        }
        let free = disk.available_space().min(total);
        if best.as_ref().map_or(true, |b| mount.len() > b.root.len()) {
            best = Some(VolumeInfo {
                root: mount,
                total,
                free,
                used: total - free,
            });
        }
    }
    best
}

/// `path` 是否等于挂载点或位于其下（按 `/` 边界比较，避免 `/mnt` 误配 `/mnt2`）
fn is_same_or_child(path: &str, mount: &str) -> bool {
    if mount == "/" {
        return path.starts_with('/');
    }
    if path == mount {
        return true;
    }
    path.strip_prefix(mount).is_some_and(|rest| rest.starts_with('/'))
}

/// `path` 是否为一个可识别的卷根
pub fn is_volume_root(path: &Path) -> bool { volume_of(path).is_some_and(|v| v.root == normalize_path(path)) }

/// 构造卷级对账；仅当 `path` 是卷根且能读到卷信息时返回 `Some`。
///
/// `accounted_bytes` 取扫描结果的总大小，`apparent_size` 为本次扫描的口径。
/// 不可读目录清单只有在诊断记录的根路径与本次一致时才附上，
/// 避免 MFT/USN 引擎（不走目录枚举）误用上一次 Fs 扫描的残留数据。
pub fn reconcile(path: &Path, accounted_bytes: u64, apparent_size: bool) -> Option<VolumeReconciliation> {
    use super::scan_diagnostics;

    let volume = volume_of(path)?;
    if volume.root != normalize_path(path) {
        return None;
    }

    let gap = if volume.used >= accounted_bytes {
        (volume.used - accounted_bytes).min(i64::MAX as u64) as i64
    } else {
        -((accounted_bytes - volume.used).min(i64::MAX as u64) as i64)
    };

    let diag = scan_diagnostics::snapshot();
    let matches = diag.root == volume.root;

    Some(VolumeReconciliation {
        volume_root: volume.root,
        total_bytes: volume.total,
        used_bytes: volume.used,
        free_bytes: volume.free,
        accounted_bytes,
        gap_bytes: gap,
        apparent_size,
        unreadable: if matches { diag.unreadable } else { Vec::new() },
        total_unreadable: if matches { diag.total_unreadable } else { 0 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_unifies_separators_and_trailing_slash() {
        assert_eq!(normalize_path(Path::new("C:\\")), "C:");
        assert_eq!(normalize_path(Path::new("C:/")), "C:");
        assert_eq!(normalize_path(Path::new("/")), "/");
        assert_eq!(normalize_path(Path::new("/mnt/data/")), "/mnt/data");
    }

    #[test]
    fn prefix_matching_respects_component_boundary() {
        assert!(is_same_or_child("/mnt/data", "/mnt"));
        assert!(!is_same_or_child("/mnt2/data", "/mnt"));
        assert!(is_same_or_child("/mnt", "/mnt"));
        assert!(is_same_or_child("/anything", "/"));
        assert!(is_same_or_child("C:/Windows", "C:"));
        assert!(!is_same_or_child("D:/Windows", "C:"));
    }

    fn fixture(gap: i64) -> VolumeReconciliation {
        VolumeReconciliation {
            volume_root: "C:".into(),
            total_bytes: 200,
            used_bytes: 100,
            free_bytes: 100,
            accounted_bytes: (100 - gap) as u64,
            gap_bytes: gap,
            apparent_size: false,
            unreadable: Vec::new(),
            total_unreadable: 0,
        }
    }

    #[test]
    fn gap_direction_and_labels() {
        let short = fixture(20);
        assert_eq!(short.gap_label(), "未计入");
        assert_eq!(short.gap_abs(), 20);
        assert_eq!(short.basis_label(), "分配大小（簇对齐）");
        assert!((short.coverage_percent() - 80.0).abs() < f64::EPSILON);

        let over = fixture(-20);
        assert_eq!(over.gap_label(), "多算");
        assert_eq!(over.gap_abs(), 20);
        assert!((over.coverage_percent() - 120.0).abs() < f64::EPSILON);

        let mut exact = fixture(0);
        assert_eq!(exact.gap_label(), "一致");
        exact.apparent_size = true;
        assert_eq!(exact.basis_label(), "表观大小（逻辑长度）");
    }

    #[test]
    fn non_volume_root_never_reconciles() {
        // 子目录不是卷根，对账无意义（卷已用空间包含该目录之外的所有内容）
        assert!(reconcile(Path::new("/definitely/not/a/volume/root"), 42, false).is_none());
        assert!(reconcile(Path::new("C:/Windows/System32"), 42, false).is_none());
    }
}
