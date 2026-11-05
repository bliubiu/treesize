//! 扫描诊断：记录本次扫描中「读不到的目录」。
//!
//! 这类信息不属于文件树本身——不可读目录既没有子节点，也不该影响大小统计——
//! 但它是解释「扫描计入量 < 卷已用空间」最直接的证据。因此放在进程级共享槽里：
//! `FsScanEngine` 在扫描开始时清空、遍历失败时追加，展示层按需取快照。
//!
//! 之所以不挂到 `FileNode` / `ScanStats` 上：那会污染领域模型与历史持久化结构，
//! 而诊断数据只对「刚扫完的这一次」有意义，是一次性的旁路边信息。

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::volume_usage::{normalize_path, UnreadableDir};

/// 最多保留的明细条数；超过后仍累计总数，避免大盘扫描把内存吃满
const MAX_DETAILS: usize = 2000;

#[derive(Debug, Clone, Default)]
pub struct ScanDiagnostics {
    /// 本次扫描的根（规范化字符串），展示层据此防止误用上一次扫描的数据
    pub root: String,
    /// 不可读目录明细
    pub unreadable: Vec<UnreadableDir>,
    /// 去重后的不可读目录总数（可能大于 `unreadable.len()`）
    pub total_unreadable: usize,
    /// 已记录路径集合：超出明细上限后仍能正确去重
    seen: HashSet<String>,
}

fn slot() -> &'static Mutex<ScanDiagnostics> {
    static SLOT: OnceLock<Mutex<ScanDiagnostics>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(ScanDiagnostics::default()))
}

fn lock() -> std::sync::MutexGuard<'static, ScanDiagnostics> {
    slot().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 开始一次新扫描：清空上一次的诊断数据并登记根路径
pub fn begin(root: &Path) {
    let mut d = lock();
    d.root = normalize_path(root);
    d.unreadable.clear();
    d.total_unreadable = 0;
    d.seen.clear();
}

/// 记录一个读不到的目录；同一路径只记一次，原因取第一次
pub fn record_unreadable(dir: &Path, reason: &str) {
    let path = normalize_path(dir);
    let mut d = lock();
    if !d.seen.insert(path.clone()) {
        return;
    }
    d.total_unreadable += 1;
    if d.unreadable.len() < MAX_DETAILS {
        d.unreadable.push(UnreadableDir {
            path,
            reason: reason.to_string(),
        });
    }
}

/// 取当前诊断快照（克隆，展示层可安全持有）
pub fn snapshot() -> ScanDiagnostics { lock().clone() }

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局槽是共享状态，所有断言放在同一个测试里串行执行，避免并行测试互相清空
    #[test]
    fn begin_record_dedup_and_snapshot() {
        begin(Path::new("C:/"));
        let empty = snapshot();
        assert_eq!(empty.root, "C:");
        assert_eq!(empty.total_unreadable, 0);
        assert!(empty.unreadable.is_empty());

        record_unreadable(Path::new("C:\\System Volume Information"), "拒绝访问 (os error 5)");
        record_unreadable(Path::new("C:/System Volume Information"), "重复失败应被去重");
        record_unreadable(Path::new("C:/Recovery"), "权限不足");

        let snap = snapshot();
        assert_eq!(snap.total_unreadable, 2, "同一目录的两种路径写法应被去重");
        assert_eq!(snap.unreadable.len(), 2);
        assert_eq!(snap.unreadable[0].path, "C:/System Volume Information");
        assert!(snap.unreadable[0].reason.contains("拒绝访问"));
        assert_eq!(snap.unreadable[1].path, "C:/Recovery");

        // 换一个扫描根会清空旧数据，防止串味
        begin(Path::new("D:/"));
        let after = snapshot();
        assert_eq!(after.root, "D:");
        assert_eq!(after.total_unreadable, 0);
        assert!(after.unreadable.is_empty());
    }
}
