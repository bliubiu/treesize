//! 拖拽收集器领域模型
//!
//! 支持用户拖拽文件/目录到应用中进行收集和删除。

use std::path::PathBuf;

/// 拖拽收集项
#[derive(Debug, Clone)]
pub struct CollectedItem {
    /// 文件/目录路径
    pub path: PathBuf,
    /// 名称
    pub name: String,
    /// 大小（字节）
    pub size: u64,
    /// 是否为目录
    pub is_dir: bool,
    /// 是否已标记删除
    pub marked_for_deletion: bool,
}

impl CollectedItem {
    /// 创建新的收集项
    pub fn new(path: PathBuf) -> Self {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();

        Self {
            path,
            name,
            size: 0,
            is_dir: false,
            marked_for_deletion: false,
        }
    }

    /// 设置大小
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    /// 设置为目录
    pub fn as_dir(mut self) -> Self {
        self.is_dir = true;
        self
    }
}

/// 拖拽收集器
#[derive(Debug, Default)]
pub struct DragCollector {
    /// 收集的项目列表
    pub items: Vec<CollectedItem>,
    /// 总大小
    pub total_size: u64,
}

impl DragCollector {
    /// 创建新的收集器
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加项目
    pub fn add(&mut self, item: CollectedItem) {
        // 检查是否已存在
        if !self.items.iter().any(|i| i.path == item.path) {
            self.total_size += item.size;
            self.items.push(item);
        }
    }

    /// 移除项目
    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.total_size -= self.items[index].size;
            self.items.remove(index);
        }
    }

    /// 标记项目为待删除
    pub fn mark_for_deletion(&mut self, index: usize) {
        if index < self.items.len() {
            self.items[index].marked_for_deletion = true;
        }
    }

    /// 取消标记
    pub fn unmark_for_deletion(&mut self, index: usize) {
        if index < self.items.len() {
            self.items[index].marked_for_deletion = false;
        }
    }

    /// 清空收集器
    pub fn clear(&mut self) {
        self.items.clear();
        self.total_size = 0;
    }

    /// 获取待删除项目数量
    pub fn marked_count(&self) -> usize {
        self.items.iter().filter(|i| i.marked_for_deletion).count()
    }

    /// 获取待删除项目总大小
    pub fn marked_size(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| i.marked_for_deletion)
            .map(|i| i.size)
            .sum()
    }
}
