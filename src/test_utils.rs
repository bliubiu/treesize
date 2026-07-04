//! 测试辅助工具
//!
//! 提供在单元测试中构造 `FileNode` 树的共享函数。
//! 仅在 `cfg(test)` 模式下编译。

use std::fs;
use std::path::Path;

use crate::domain::file_node::FileNode;

/// 从临时目录递归构建 `FileNode` 树
///
/// 读取目录内容，识别文件和子目录，递归聚合大小和计数。
/// 用于 `duplicate_service` 和 `waste_service` 等需要真实文件系统结构的测试。
pub fn build_tree_from_dir(root: &Path) -> FileNode {
    let mut node = FileNode::new_dir(root.to_path_buf(), None);
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let meta = fs::metadata(&path).unwrap();
        if meta.is_dir() {
            let mut child = build_tree_from_dir(&path);
            child.aggregate();
            node.children.push(child);
        } else {
            node.children
                .push(FileNode::new_file(path, meta.len(), meta.modified().ok()));
        }
    }
    node.aggregate();
    node
}
