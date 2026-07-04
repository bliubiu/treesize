//! 分类统计服务
//!
//! 按文件大类、扩展名、目录深度等维度聚合统计。

use std::collections::HashMap;

use serde::Serialize;

use crate::domain::file_node::FileNode;
use crate::domain::value_objects::{ByteSize, FileCategory};

/// 单个分类统计条目
#[derive(Debug, Serialize)]
pub struct CategoryStat {
    pub category: FileCategory,
    pub label: String,
    pub file_count: u64,
    pub total_size: ByteSize,
    pub percent: f64,
}

/// 扩展名统计条目
#[derive(Debug, Serialize)]
pub struct ExtensionStat {
    pub extension: String,
    pub file_count: u64,
    pub total_size: ByteSize,
    pub percent: f64,
}

/// 分类统计报表
#[derive(Debug, Serialize)]
pub struct ClassifyReport {
    pub by_category: Vec<CategoryStat>,
    pub by_extension: Vec<ExtensionStat>,
    pub total_files: u64,
    pub total_size: ByteSize,
}

/// 分类统计服务
pub struct ClassifyService;

impl ClassifyService {
    /// 生成分类统计报表
    pub fn analyze(root: &FileNode) -> ClassifyReport {
        let mut cat_files: HashMap<FileCategory, u64> = HashMap::new();
        let mut cat_size: HashMap<FileCategory, u64> = HashMap::new();
        let mut ext_files: HashMap<String, u64> = HashMap::new();
        let mut ext_size: HashMap<String, u64> = HashMap::new();

        let mut total_files: u64 = 0;
        let mut total_size: u64 = 0;

        for node in root.iter_all() {
            if !node.is_file() {
                continue;
            }
            total_files += 1;
            total_size = total_size.saturating_add(node.size.0);

            *cat_files.entry(node.category).or_insert(0) += 1;
            *cat_size.entry(node.category).or_insert(0) += node.size.0;

            let ext = if node.extension.is_empty() {
                "(无扩展名)".to_string()
            } else {
                node.extension.clone()
            };
            *ext_files.entry(ext.clone()).or_insert(0) += 1;
            *ext_size.entry(ext.clone()).or_insert(0) += node.size.0;
        }

        let mut by_category: Vec<CategoryStat> = cat_files
            .into_iter()
            .map(|(cat, count)| CategoryStat {
                category: cat,
                label: cat.label().to_string(),
                file_count: count,
                total_size: ByteSize(*cat_size.get(&cat).unwrap_or(&0)),
                percent: percent(*cat_size.get(&cat).unwrap_or(&0), total_size),
            })
            .collect();
        by_category.sort_by(|a, b| b.total_size.cmp(&a.total_size));

        let mut by_extension: Vec<ExtensionStat> = ext_files
            .into_iter()
            .map(|(ext, count)| ExtensionStat {
                total_size: ByteSize(*ext_size.get(&ext).unwrap_or(&0)),
                file_count: count,
                percent: percent(*ext_size.get(&ext).unwrap_or(&0), total_size),
                extension: ext,
            })
            .collect();
        by_extension.sort_by(|a, b| b.total_size.cmp(&a.total_size));

        ClassifyReport {
            by_category,
            by_extension,
            total_files,
            total_size: ByteSize(total_size),
        }
    }
}

fn percent(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn build_tree() -> FileNode {
        let mut root = FileNode::new_dir(PathBuf::from("/root"), None);
        root.children
            .push(FileNode::new_file(PathBuf::from("/root/a.txt"), 100, None));
        root.children
            .push(FileNode::new_file(PathBuf::from("/root/b.mp4"), 300, None));
        root.children
            .push(FileNode::new_file(PathBuf::from("/root/c.txt"), 50, None));
        root.aggregate();
        root
    }

    #[test]
    fn analyze_categories() {
        let root = build_tree();
        let report = ClassifyService::analyze(&root);

        assert_eq!(report.total_files, 3);
        assert_eq!(report.total_size.0, 450);

        let video = report
            .by_category
            .iter()
            .find(|c| c.category == FileCategory::Video)
            .unwrap();
        assert_eq!(video.file_count, 1);
        assert_eq!(video.total_size.0, 300);

        let doc = report
            .by_category
            .iter()
            .find(|c| c.category == FileCategory::Document)
            .unwrap();
        assert_eq!(doc.file_count, 2);
        assert_eq!(doc.total_size.0, 150);
    }

    #[test]
    fn analyze_extensions() {
        let root = build_tree();
        let report = ClassifyService::analyze(&root);

        let txt = report
            .by_extension
            .iter()
            .find(|e| e.extension == "txt")
            .unwrap();
        assert_eq!(txt.file_count, 2);
        assert_eq!(txt.total_size.0, 150);

        let mp4 = report
            .by_extension
            .iter()
            .find(|e| e.extension == "mp4")
            .unwrap();
        assert_eq!(mp4.file_count, 1);
    }
}
