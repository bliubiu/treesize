//! 扫描控制子模块
//!
//! 从 `app.rs` 拆出扫描生命周期：`start_scan` / `cancel_scan` 及扫描引擎工厂。
//! 本模块是 `gui::app` 的子模块，因此可访问 `TreeSizeApp` 的私有字段。
//! 方法以 `pub(super)` 暴露给父模块 `app.rs` 的工具栏与快捷键调用；逻辑与原实现一致。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use super::TreeSizeApp;
use crate::application::ScanService;
use crate::domain::scan_engine::{CancelToken, ScanOptions, ScanProgress};
use crate::infrastructure::{detect_best_engine, FsScanEngine};
#[cfg(target_os = "windows")]
use crate::infrastructure::{MftScanEngine, UsnScanEngine};

impl TreeSizeApp {
    pub(super) fn start_scan(&mut self) {
        let path_str = self.path_input.trim().to_string();
        let path = PathBuf::from(&path_str);
        tracing::info!(target: "treesize::gui", "start_scan 被调用，路径：{}", path.display());

        if !path.exists() {
            self.state.lock().unwrap().error = Some(format!("路径不存在：{}", path.display()));
            tracing::warn!(target: "treesize::gui", "路径不存在：{}", path.display());
            return;
        }

        if !self.path_history.contains(&path_str) {
            self.path_history.push_back(path_str.clone());
            if self.path_history.len() > 20 {
                self.path_history.pop_front();
            }
        }

        let state = self.state.clone();
        let mut options = self.options.clone();
        options.include_hidden = self.show_hidden;

        let generation = {
            let mut s = state.lock().unwrap();
            s.cancel.cancel();
            s.cancel = CancelToken::new();
            s.running = true;
            s.error = None;
            s.node = None;
            s.stats = None;
            s.progress = ScanProgress::default();
            s.started_at = Some(SystemTime::now());
            s.reconciliation = None;
            s.generation += 1;
            s.generation
        };

        let cancel_token = {
            let s = state.lock().unwrap();
            s.cancel.clone()
        };

        let path_clone = path.clone();
        #[cfg(target_os = "windows")]
        let watch = self.watch.clone();
        tracing::info!(target: "treesize::gui", "即将启动扫描线程，代次：{}", generation);

        std::thread::Builder::new()
            .name("treesize-scan".to_string())
            .stack_size(256 * 1024 * 1024)
            .spawn(move || {
                tracing::info!(target: "treesize::gui", "扫描线程已启动，路径：{}", path_clone.display());

                let mut current_options = options.clone();
                let mut fallback_to_fs = false;

                let result = loop {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        tracing::info!(target: "treesize::gui", "创建扫描引擎...");
                        let engine = create_scan_engine(&current_options, &path_clone);
                        tracing::info!(target: "treesize::gui", "创建扫描服务...");
                        let service = ScanService::new(engine);

                        let state_clone = state.clone();
                        let progress_cb = move |p: &ScanProgress| {
                            if let Ok(mut s) = state_clone.lock() {
                                if s.generation != generation {
                                    return;
                                }
                                s.progress = p.clone();
                            }
                        };

                        tracing::info!(target: "treesize::gui", "开始执行扫描...");
                        service.scan(path.clone(), &current_options, Some(&progress_cb), Some(&cancel_token))
                    }));

                    if let Ok(Err(e)) = &result {
                        let err_str = e.to_string();
                        if !fallback_to_fs
                            && current_options.engine == crate::domain::scan_engine::ScanEngineType::Mft
                            && (err_str.contains("MFT") || err_str.contains("NTFS") || err_str.contains("LCN"))
                        {
                            tracing::warn!(target: "treesize::gui", "MFT 扫描失败，自动降级到 Fs 引擎重试");
                            fallback_to_fs = true;
                            current_options.engine = crate::domain::scan_engine::ScanEngineType::Fs;
                            continue;
                        }
                    }

                    break result;
                };

                tracing::info!(target: "treesize::gui", "扫描线程执行完毕");

                let (result_node, result_stats, result_error) = match result {
                    Ok(Ok((node, stats))) => {
                        tracing::info!(target: "treesize::gui", "GUI 扫描完成");
                        (Some(node), Some(stats), None)
                    },
                    Ok(Err(e)) => {
                        tracing::error!(target: "treesize::gui", "GUI 扫描失败：{e}");
                        (None, None, Some(e.to_string()))
                    },
                    Err(panic_payload) => {
                        let msg = panic_payload
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| panic_payload.downcast_ref::<String>().map(|s| s.to_string()))
                            .unwrap_or_else(|| "扫描线程内部错误".to_string());
                        tracing::error!(target: "treesize::gui", "扫描线程 panic：{msg}");
                        (None, None, Some(msg))
                    },
                };

                // 卷级对账只在卷根扫描时有意义；在扫描线程里算一次并缓存，
                // 避免 Dashboard 每帧调用 sysinfo::Disks（有系统调用开销）。
                let reconciliation = result_stats.as_ref().and_then(|st| {
                    crate::infrastructure::reconcile(&path, st.total_size.bytes(), current_options.apparent_size)
                });

                let scan_succeeded = result_node.is_some();

                if let Ok(mut s) = state.lock() {
                    if s.generation != generation {
                        tracing::info!(target: "treesize::gui", "跳过过期扫描结果（代次不匹配）");
                        return;
                    }
                    s.running = false;
                    s.error = result_error;
                    s.reconciliation = reconciliation;
                    if let (Some(n), Some(st)) = (result_node, result_stats) {
                        s.node = Some(Arc::new(n));
                        s.stats = Some(st);
                    }
                }

                // 扫描成功完成后以当前卷 USN 作为变化感知基线。
                #[cfg(target_os = "windows")]
                if scan_succeeded {
                    if let Some(w) = &watch {
                        w.set_baseline_from_path(&path_clone);
                    }
                }
            })
            .expect("创建扫描线程失败");
    }

    pub(super) fn cancel_scan(&mut self) {
        let s = self.state.lock().unwrap();
        s.cancel.cancel();
    }
}

fn create_scan_engine(
    options: &ScanOptions,
    root_path: &std::path::Path,
) -> Arc<dyn crate::domain::scan_engine::ScanEngine> {
    // 解析 Auto 引擎类型
    let engine_type = if options.engine == crate::domain::scan_engine::ScanEngineType::Auto {
        let detected = detect_best_engine(root_path);
        tracing::info!(target: "treesize::gui", "引擎自动选择结果：{}", detected);
        detected
    } else {
        options.engine
    };

    #[cfg(target_os = "windows")]
    {
        match engine_type {
            crate::domain::scan_engine::ScanEngineType::Mft => {
                tracing::info!("GUI 使用 MFT 直接读取引擎（极速）");
                Arc::new(MftScanEngine::new())
            },
            crate::domain::scan_engine::ScanEngineType::Usn => {
                tracing::info!("GUI 使用 USN Journal 增量引擎");
                Arc::new(UsnScanEngine::new())
            },
            _ => {
                tracing::info!("GUI 使用 FsScanEngine（文件系统遍历）");
                Arc::new(FsScanEngine::new())
            },
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = engine_type; // 非 Windows 始终用 Fs
        tracing::info!("GUI 使用 FsScanEngine（文件系统遍历）");
        Arc::new(FsScanEngine::new())
    }
}
