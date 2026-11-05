//! 聚合计算缓存控制子模块
//!
//! 从 `app.rs` 拆出缓存失效与后台聚合计算刷新：`invalidate_cache_if_needed` / `refresh_cache`。
//! 本模块是 `gui::app` 的子模块，因此可访问 `TreeSizeApp` 的私有字段。
//! 方法以 `pub(super)` 暴露给父模块 `app.rs` 的 `update()` 调用；逻辑与原实现一致。

use super::{ComputeCache, Snapshot, Tab, TreeSizeApp};
use crate::application::{ClassifyService, DuplicateService, ReportService, WasteService};

/// 后台聚合计划：返回 `(是否需要快速聚合, 是否需要重复文件哈希)`。
///
/// 设计约束（避免重复文件哈希拖垮主界面）：
/// - 快速聚合（TopN 文件/目录、分类、大文件列表、可清理）走纯内存遍历，毫秒级；
/// - 重复文件哈希（全量内容哈希，需读盘、整卷可能耗时数分钟）**仅由「重复文件」页触发**，
///   Dashboard 不再自动计算。理由：整卷扫描后 Dashboard 若自动跑哈希，会因
///   `cache_computing` 恒真而退化为高频轮询空转（空闲 CPU 居高不下），且其余毫秒级
///   洞察会被慢哈希阻塞、一直显示「计算中…」。
///
/// `needs_dup` 的触发条件若被改回包含 Dashboard，本单元测试会立即失败。
pub(super) fn compute_cache_plan(
    tab: Tab,
    cached_top_files: bool,
    cached_top_dirs: bool,
    cached_classify: bool,
    cached_all_files: bool,
    cached_duplicates: bool,
    cached_waste: bool,
) -> (bool, bool) {
    let needs_fast = (tab == Tab::Dashboard && (!cached_top_files || !cached_top_dirs || !cached_classify))
        || (tab == Tab::TopFiles && !cached_all_files)
        || (tab == Tab::Classify && !cached_classify)
        || (tab == Tab::Waste && !cached_waste);

    // 重复文件哈希仅由「重复文件」页触发，Dashboard 从不自动算
    let needs_dup = tab == Tab::Duplicates && !cached_duplicates;

    (needs_fast, needs_dup)
}

impl TreeSizeApp {
    pub(super) fn invalidate_cache_if_needed(&mut self, snapshot: &Snapshot) {
        let cache_invalid = snapshot.generation != self.cache_generation || self.cache_top_n != self.top_n;
        if cache_invalid {
            self.cache_generation = snapshot.generation;
            self.cache_top_n = self.top_n;
            self.cached_top_files = None;
            self.cached_top_dirs = None;
            self.cached_classify = None;
            self.cached_duplicates = None;
            self.cached_waste = None;
            self.cached_all_files = None;
            // 重复文件哈希线程较慢，代次变更时整批作废，避免过期结果污染新树
            self.dup_computing = false;
            self.dup_generation = 0;
            *self.compute_cache.lock().unwrap() = ComputeCache::default();
            *self.dup_cache.lock().unwrap() = None;
        }
    }

    pub(super) fn refresh_cache(&mut self, snapshot: &Snapshot) {
        // 扫描运行期间不启动缓存计算：
        // 1) 计算基于旧树，扫描完成后代次变化即作废，纯浪费 CPU/磁盘；
        // 2) 重复文件服务的哈希与扫描争抢全局 rayon 线程池与磁盘，
        //    曾把 MFT 解析从 0.4s 拖慢到 25.6s。
        if snapshot.node.is_none() || snapshot.running {
            return;
        }

        // 快速聚合（纯内存遍历，毫秒级）：TopN 文件/目录 + 分类 + 大文件列表 + 可清理。
        // 注意 Dashboard 不再在此等待重复文件哈希，避免「最大文件/目录/主要类型」被慢计算阻塞。
        let (needs_fast, needs_dup) = compute_cache_plan(
            self.selected_tab,
            self.cached_top_files.is_some(),
            self.cached_top_dirs.is_some(),
            self.cached_classify.is_some(),
            self.cached_all_files.is_some(),
            self.cached_duplicates.is_some(),
            self.cached_waste.is_some(),
        );

        if needs_fast && !self.cache_computing {
            self.cache_computing = true;
            self.cache_generation_computing = snapshot.generation;

            let state_clone = self.state.clone();
            let compute_cache = self.compute_cache.clone();
            let selected_tab = self.selected_tab;
            let top_n = self.top_n;
            let generation = snapshot.generation;

            std::thread::spawn(move || {
                let s = state_clone.lock().unwrap();
                if let Some(node) = s.node.as_ref() {
                    if s.generation != generation {
                        return;
                    }

                    let node_clone = node.clone();
                    drop(s); // ⚡ 尽早释放 state 锁

                    // ⚡ 先计算，不持有任何锁
                    // Dashboard 分支有意不包含重复文件（交由下方 dup 线程），保证快速洞察即时显示
                    let (top_files, top_dirs, classify, waste, all_files) = match selected_tab {
                        Tab::Dashboard => {
                            let tf = ReportService::top_n_files_report(&node_clone, top_n);
                            let td = ReportService::top_n_dirs_report(&node_clone, top_n);
                            let cl = ClassifyService::analyze(&node_clone);
                            (Some(tf), Some(td), Some(cl), None, None)
                        },
                        Tab::TopFiles => {
                            let af = ReportService::top_n_files_report(&node_clone, top_n.max(200));
                            (None, None, None, None, Some(af))
                        },
                        Tab::Classify => {
                            let cl = ClassifyService::analyze(&node_clone);
                            (None, None, Some(cl), None, None)
                        },
                        Tab::Waste => {
                            let wa = WasteService::scan(&node_clone);
                            (None, None, None, Some(wa), None)
                        },
                        _ => (None, None, None, None, None),
                    };

                    // ⚡ 计算完毕，短时间获取锁写入结果
                    let mut cache = compute_cache.lock().unwrap();
                    cache.generation = generation;
                    if let Some(v) = top_files {
                        cache.top_files = Some(v);
                    }
                    if let Some(v) = top_dirs {
                        cache.top_dirs = Some(v);
                    }
                    if let Some(v) = classify {
                        cache.classify = Some(v);
                    }
                    if let Some(v) = waste {
                        cache.waste = Some(v);
                    }
                    if let Some(v) = all_files {
                        cache.all_files = Some(v);
                    }
                }
            });
        }

        if needs_dup && !self.dup_computing {
            self.dup_computing = true;
            self.dup_generation = snapshot.generation;

            let state_clone = self.state.clone();
            let dup_cache = self.dup_cache.clone();
            let generation = snapshot.generation;

            std::thread::spawn(move || {
                let node_clone = {
                    let s = state_clone.lock().unwrap();
                    match s.node.as_ref() {
                        // ⚡ 代次不符直接放弃，避免基于旧树的过期哈希污染新结果
                        Some(n) if s.generation == generation => n.clone(),
                        _ => return,
                    }
                }; // ⚡ 释放 state 锁后再读盘
                let dd = DuplicateService::scan(&node_clone, 1024);
                *dup_cache.lock().unwrap() = Some(dd);
            });
        }

        if self.cache_computing {
            let mut cache = self.compute_cache.lock().unwrap();
            if cache.generation == snapshot.generation {
                if let Some(tf) = cache.top_files.take() {
                    self.cached_top_files = Some(tf);
                }
                if let Some(td) = cache.top_dirs.take() {
                    self.cached_top_dirs = Some(td);
                }
                if let Some(cl) = cache.classify.take() {
                    self.cached_classify = Some(cl);
                }
                // 注意：快速聚合不再产出重复文件，重复结果由 dup_cache 单独回收
                if let Some(wa) = cache.waste.take() {
                    self.cached_waste = Some(wa);
                }
                if let Some(af) = cache.all_files.take() {
                    self.cached_all_files = Some(af);
                }
                self.cache_computing = false;
            }
        }

        if self.dup_computing {
            // 代次已变更（新一轮扫描）：丢弃过期结果，等待下一轮重新计算
            if self.dup_generation != snapshot.generation {
                *self.dup_cache.lock().unwrap() = None;
                self.dup_computing = false;
            } else if let Some(dd) = self.dup_cache.lock().unwrap().take() {
                self.cached_duplicates = Some(dd);
                self.dup_computing = false;
            }
        }
    }
}
