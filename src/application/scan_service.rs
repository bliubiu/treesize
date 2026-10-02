//! 扫描服务
//!
//! 编排 `ScanEngine` 完成扫描用例，封装统计信息收集。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use crate::domain::error::Result;
use crate::domain::file_node::FileNode;
use crate::domain::scan_engine::{CancelToken, ScanEngine, ScanOptions, ScanProgress, ScanStats};

/// 扫描服务
pub struct ScanService {
    engine: Arc<dyn ScanEngine>,
}

impl ScanService {
    pub fn new(engine: Arc<dyn ScanEngine>) -> Self {
        Self { engine }
    }

    /// 执行扫描
    ///
    /// `progress` 回调用于上报进度，`cancel` 用于中止。
    pub fn scan(
        &self,
        root: PathBuf,
        options: &ScanOptions,
        progress: Option<&(dyn Fn(&ScanProgress) + Send + Sync)>,
        cancel: Option<&CancelToken>,
    ) -> Result<(FileNode, ScanStats)> {
        let start = SystemTime::now();
        let node = self.engine.scan(root, options, progress, cancel)?;

        let elapsed_ms = SystemTime::now()
            .duration_since(start)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let stats = ScanStats {
            total_files: node.file_count,
            total_dirs: node.dir_count,
            total_size: node.size,
            error_count: count_errors(&node),
            elapsed_ms,
        };

        Ok((node, stats))
    }
}

/// 递归统计错误数
fn count_errors(node: &FileNode) -> u64 {
    let mut n = node.errors.len() as u64;
    for c in &node.children {
        n += count_errors(c);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::file_node::FileNode;
    use crate::domain::scan_engine::testing::MockScanEngine;
    use std::path::PathBuf;

    #[test]
    fn scan_service_returns_stats() {
        let mut root = FileNode::new_dir(PathBuf::from("/root"), None);
        root.children
            .push(FileNode::new_file(PathBuf::from("/root/a.txt"), 100, None));
        root.aggregate();

        let engine = Arc::new(MockScanEngine::new(root));
        let svc = ScanService::new(engine);

        let (node, stats) = svc
            .scan(PathBuf::from("/root"), &ScanOptions::default(), None, None)
            .unwrap();

        assert_eq!(node.file_count, 1);
        assert_eq!(stats.total_files, 1);
        assert_eq!(stats.total_size.bytes(), 100);
    }
}
