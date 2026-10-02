//! 系统资源监控
//!
//! 提供内存使用监控功能，防止扫描过程中内存耗尽。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sysinfo::{Pid, System};

/// 内存监控器
pub struct MemoryMonitor {
    max_memory_mb: u64,
    should_stop: AtomicBool,
    current_usage_mb: AtomicU64,
    system: System,
    pid: Pid,
}

impl MemoryMonitor {
    /// 创建内存监控器
    /// `max_memory_mb` 为最大允许内存使用量（MB），0 表示不限制
    pub fn new(max_memory_mb: u64) -> Self {
        let pid = Pid::from(std::process::id() as usize);
        Self {
            max_memory_mb,
            should_stop: AtomicBool::new(false),
            current_usage_mb: AtomicU64::new(0),
            system: System::new(),
            pid,
        }
    }

    /// 检查是否应该停止扫描
    pub fn should_stop(&self) -> bool {
        self.should_stop.load(Ordering::Relaxed)
    }

    /// 获取当前内存使用量（MB）
    pub fn current_usage(&self) -> u64 {
        self.current_usage_mb.load(Ordering::Relaxed)
    }

    /// 更新内存使用状态，返回是否应该继续
    pub fn check_memory(&mut self) -> bool {
        if self.max_memory_mb == 0 {
            return true;
        }

        self.system.refresh_memory();
        self.system
            .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[self.pid]), false);

        // 获取当前进程内存使用量（字节），转换为 MB
        let used_mb = self
            .system
            .process(self.pid)
            .map(|p| p.memory() / 1024 / 1024)
            .unwrap_or(0);
        self.current_usage_mb.store(used_mb, Ordering::Relaxed);

        if used_mb >= self.max_memory_mb {
            self.should_stop.store(true, Ordering::Relaxed);
            tracing::warn!(target: "treesize::memory", "进程内存使用超过限制：{} MB / {} MB", used_mb, self.max_memory_mb);
            false
        } else {
            true
        }
    }

    /// 估算扫描任务的内存需求
    /// 返回 (estimated_mb, warning: bool)
    pub fn estimate_memory(files_count: u64) -> (u64, bool) {
        // 每个文件节点约 200 字节（保守估算），使用浮点运算避免整数除法归零
        let estimated_mb = ((files_count as f64 * 200.0) / 1024.0 / 1024.0).ceil() as u64;

        // 系统可用内存（使用新实例，因是静态方法）
        let mut sys = System::new();
        sys.refresh_memory();
        let available_mb = sys.available_memory() / 1024 / 1024;

        // 安全阈值：使用不超过可用内存的 80%
        let safe_limit = (available_mb as f64 * 0.8) as u64;
        let warning = estimated_mb > safe_limit;

        if warning {
            tracing::warn!(target: "treesize::memory", "预计内存使用可能超出安全范围：预计 {} MB，安全上限 {} MB", estimated_mb, safe_limit);
        }

        (estimated_mb, warning)
    }
}
