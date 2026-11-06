//! 磁盘内容变化感知子模块（仅 Windows NTFS）
//!
//! 复用 NTFS 的 USN Journal：后台线程低频轮询当前扫描根所在卷的
//! `NextUsn`，一旦增长或 Journal ID 变化即视为磁盘内容发生变更，
//! 置位脏标志并请求 UI 重绘。GUI 据此显示「数据可能已过期」提示条，
//! 由用户决定何时重新扫描。
//!
//! 说明：
//! - 选择 USN 而非目录监听，是因为一次查询即覆盖整卷，且由内核维护，
//!   不会像递归 `ReadDirectoryChangesW` 那样耗尽句柄或丢事件。
//! - 读取卷设备需要权限；普通用户打开 `\\.\X:` 会失败。此时不建立基线，
//!   自动感知能力关闭，不影响正常扫描。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;

use super::TreeSizeApp;
use crate::infrastructure::ntfs_reader::{path_to_volume_device, query_usn_journal, VolumeHandle};

/// 轮询间隔：3 秒足够及时发现变化，又不会带来可感知的额外开销。
const POLL_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Clone)]
struct Baseline {
    volume_device: PathBuf,
    journal_id: u64,
    next_usn: i64,
}

/// 磁盘变化监视器：后台线程 + 共享脏标志。
pub(super) struct ChangeWatch {
    dirty: Arc<AtomicBool>,
    baseline: Arc<Mutex<Option<Baseline>>>,
    stop: Arc<AtomicBool>,
}

impl ChangeWatch {
    pub(super) fn new(ctx: egui::Context) -> Self {
        let dirty = Arc::new(AtomicBool::new(false));
        let baseline = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));

        {
            let dirty = dirty.clone();
            let baseline = baseline.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("treesize-watch".to_string())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let current = baseline.lock().unwrap().clone();
                        if let Some(b) = current {
                            if volume_changed(&b) && !dirty.swap(true, Ordering::SeqCst) {
                                tracing::info!(
                                    target: "treesize::watch",
                                    "检测到磁盘内容变化：{}",
                                    b.volume_device.display(),
                                );
                                // 唤醒空闲的 UI，让提示条尽快出现。
                                ctx.request_repaint();
                            }
                        }
                        std::thread::sleep(POLL_INTERVAL);
                    }
                })
                .ok();
        }

        Self { dirty, baseline, stop }
    }

    /// 以给定路径所在卷的当前 USN 作为基线（扫描完成后调用）。
    pub(super) fn set_baseline_from_path(&self, path: &Path) {
        let device = PathBuf::from(path_to_volume_device(path));
        let journal = VolumeHandle::open(&device)
            .ok()
            .and_then(|h| query_usn_journal(&h).ok());

        match journal {
            Some(j) => {
                *self.baseline.lock().unwrap() = Some(Baseline {
                    volume_device: device,
                    journal_id: j.usn_journal_id,
                    next_usn: j.next_usn,
                });
                self.dirty.store(false, Ordering::SeqCst);
            },
            None => {
                // 无权限或非 NTFS：关闭自动感知，避免误报。
                *self.baseline.lock().unwrap() = None;
                self.dirty.store(false, Ordering::SeqCst);
                tracing::debug!(
                    target: "treesize::watch",
                    "无法读取卷 USN Journal，自动感知已关闭：{}",
                    device.display(),
                );
            },
        }
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }
}

impl Drop for ChangeWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// 查询卷当前 USN，判断相对基线是否发生变化。
/// 查询失败（权限/卷卸载）时按「无变化」处理，避免噪声式误报。
fn volume_changed(b: &Baseline) -> bool {
    match VolumeHandle::open(&b.volume_device) {
        Ok(h) => match query_usn_journal(&h) {
            Ok(j) => usn_changed(b.journal_id, b.next_usn, j.usn_journal_id, j.next_usn),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// 判断 USN 状态是否相对基线发生变化。
///
/// - Journal ID 变化 = Journal 被重置（视为全量变化）
/// - `NextUsn` 增长 = 有文件系统变更；减少（回绕）不提示
fn usn_changed(
    baseline_journal_id: u64,
    baseline_next_usn: i64,
    current_journal_id: u64,
    current_next_usn: i64,
) -> bool {
    current_journal_id != baseline_journal_id || current_next_usn > baseline_next_usn
}

impl TreeSizeApp {
    /// 渲染顶部「数据已过期」提示条。
    pub(super) fn render_change_hint(&mut self, ui: &mut egui::Ui) {
        let dirty = self.watch.as_ref().map(|w| w.is_dirty()).unwrap_or(false);
        if !dirty {
            return;
        }
        // 扫描进行中无需提示：完成后基线会更新并清除脏标志。
        if self.state.try_lock().map(|s| s.running).unwrap_or(false) {
            return;
        }

        egui::Frame::none()
            .fill(self.theme.bg_card)
            .stroke(egui::Stroke::new(1.0_f32, self.theme.accent))
            .inner_margin(egui::Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("检测到磁盘内容变化，当前数据可能已过期")
                            .color(self.theme.text_primary),
                    );
                    if ui.button("重新扫描").clicked() {
                        self.start_scan();
                    }
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::usn_changed;

    #[test]
    fn unchanged_journal_reports_no_change() {
        assert!(!usn_changed(1, 100, 1, 100));
    }

    #[test]
    fn growing_next_usn_reports_change() {
        assert!(usn_changed(1, 100, 1, 101));
    }

    #[test]
    fn shrinking_next_usn_reports_no_change() {
        assert!(!usn_changed(1, 100, 1, 99));
    }

    #[test]
    fn journal_id_change_always_reports_change() {
        assert!(usn_changed(1, 100, 2, 100));
    }
}
