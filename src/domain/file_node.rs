//! 文件节点实体
//!
//! `FileNode` 是磁盘扫描的核心实体，构成树形结构。
//! 目录节点的 `size` 为递归累计值，由扫描器在回溯阶段填充。

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::value_objects::{file_name, normalize_extension, ByteSize, FileCategory, FileType};

/// 文件节点实体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    /// 完整路径
    pub path: PathBuf,
    /// 文件名（不含目录）
    pub name: String,
    /// 大小（字节）。目录为递归累计值
    pub size: ByteSize,
    /// 节点类型
    pub file_type: FileType,
    /// 规范化扩展名（小写、无点），目录为空
    pub extension: String,
    /// 文件大类
    pub category: FileCategory,
    /// 最后修改时间
    pub modified: Option<SystemTime>,
    /// 子节点（仅目录有）
    pub children: Vec<FileNode>,
    /// 递归文件数（不含自身）
    pub file_count: u64,
    /// 递归子目录数（不含自身）
    pub dir_count: u64,
    /// 该节点扫描时遇到的错误信息
    pub errors: Vec<String>,
}

impl FileNode {
    /// 构造文件叶节点
    pub fn new_file(path: PathBuf, size: u64, modified: Option<SystemTime>) -> Self {
        let name = file_name(&path);
        let extension = normalize_extension(&path);
        let category = FileCategory::from_extension(&extension);
        Self {
            path,
            name,
            size: ByteSize(size),
            file_type: FileType::File,
            extension,
            category,
            modified,
            children: Vec::new(),
            file_count: 0,
            dir_count: 0,
            errors: Vec::new(),
        }
    }

    /// 构造目录节点（size 待回溯填充）
    pub fn new_dir(path: PathBuf, modified: Option<SystemTime>) -> Self {
        let name = file_name(&path);
        Self {
            path,
            name,
            size: ByteSize(0),
            file_type: FileType::Directory,
            extension: String::new(),
            category: FileCategory::Other,
            modified,
            children: Vec::new(),
            file_count: 0,
            dir_count: 0,
            errors: Vec::new(),
        }
    }

    pub fn is_dir(&self) -> bool {
        self.file_type == FileType::Directory
    }

    pub fn is_file(&self) -> bool {
        self.file_type == FileType::File
    }

    /// 回溯累计子节点大小与计数。在扫描器完成子树后调用。
    pub fn aggregate(&mut self) {
        if self.is_file() {
            return;
        }
        let mut total: u64 = 0;
        let mut files: u64 = 0;
        let mut dirs: u64 = 0;
        for child in &mut self.children {
            child.aggregate();
            total = total.saturating_add(child.size.0);
            files = files.saturating_add(child.file_count);
            dirs = dirs.saturating_add(child.dir_count);
            // 累加子节点后，再计入子节点自身（目录加 1，文件已在 child.file_count=0 时处理）
            match child.file_type {
                FileType::File => files = files.saturating_add(1),
                FileType::Directory => dirs = dirs.saturating_add(1),
            }
        }
        self.size = ByteSize(total);
        self.file_count = files;
        self.dir_count = dirs;
    }

    /// 按大小降序排序子节点（目录优先）
    pub fn sort_by_size_desc(&mut self) {
        self.children
            .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        for c in &mut self.children {
            c.sort_by_size_desc();
        }
    }

    /// 递归遍历所有节点（深度优先，包含自身）
    pub fn iter_all(&self) -> Box<dyn Iterator<Item = &FileNode> + '_> {
        Box::new(std::iter::once(self).chain(self.children.iter().flat_map(|c| c.iter_all())))
    }

    /// 统计所有文件叶节点
    pub fn count_files(&self) -> usize {
        if self.is_file() {
            1
        } else {
            self.children.iter().map(|c| c.count_files()).sum()
        }
    }

    /// 增量删除：移除指定路径的节点，并回溯更新聚合值
    /// 返回被删除节点的大小和文件/目录计数，用于回溯更新
    pub fn remove_path(&mut self, target: &std::path::Path) -> Option<(u64, u64, u64)> {
        // 在子节点中查找目标
        let mut found_idx = None;
        for (i, child) in self.children.iter().enumerate() {
            if child.path == target {
                found_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = found_idx {
            // 找到目标，移除并返回其统计信息
            let removed = self.children.remove(idx);
            let removed_size = removed.size.0;
            let removed_files = removed.file_count + if removed.is_file() { 1 } else { 0 };
            let removed_dirs = removed.dir_count + if removed.is_dir() { 1 } else { 0 };
            // 回溯更新当前节点的聚合值
            self.size = ByteSize(self.size.0.saturating_sub(removed_size));
            self.file_count = self.file_count.saturating_sub(removed_files);
            self.dir_count = self.dir_count.saturating_sub(removed_dirs);
            Some((removed_size, removed_files, removed_dirs))
        } else {
            // 未找到，递归在子目录中查找
            for child in &mut self.children {
                if child.is_dir() && target.starts_with(&child.path) {
                    if let Some((size, files, dirs)) = child.remove_path(target) {
                        // 回溯更新当前节点的聚合值
                        self.size = ByteSize(self.size.0.saturating_sub(size));
                        self.file_count = self.file_count.saturating_sub(files);
                        self.dir_count = self.dir_count.saturating_sub(dirs);
                        return Some((size, files, dirs));
                    }
                }
            }
            None
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file(name: &str, size: u64) -> FileNode {
        FileNode::new_file(PathBuf::from(name), size, None)
    }

    fn make_dir(name: &str) -> FileNode {
        FileNode::new_dir(PathBuf::from(name), None)
    }

    #[test]
    fn aggregate_dir_size() {
        let mut root = make_dir("/root");
        root.children.push(make_file("/root/a.txt", 100));
        root.children.push(make_file("/root/b.txt", 200));
        let mut sub = make_dir("/root/sub");
        sub.children.push(make_file("/root/sub/c.txt", 300));
        root.children.push(sub);

        root.aggregate();

        assert_eq!(root.size.0, 600);
        assert_eq!(root.file_count, 3);
        assert_eq!(root.dir_count, 1);
    }

    #[test]
    fn sort_desc() {
        let mut root = make_dir("/root");
        root.children.push(make_file("/root/a.txt", 100));
        root.children.push(make_file("/root/b.txt", 300));
        root.children.push(make_file("/root/c.txt", 200));
        root.sort_by_size_desc();

        assert_eq!(root.children[0].name, "b.txt");
        assert_eq!(root.children[1].name, "c.txt");
        assert_eq!(root.children[2].name, "a.txt");
    }

    #[test]
    fn iter_all_count() {
        let mut root = make_dir("/root");
        root.children.push(make_file("/root/a.txt", 100));
        let mut sub = make_dir("/root/sub");
        sub.children.push(make_file("/root/sub/c.txt", 300));
        root.children.push(sub);

        assert_eq!(root.iter_all().count(), 4);
    }
}
