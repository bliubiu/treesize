//! 空间浪费类型定义
//!
//! 定义各种可检测的空间浪费类型，用于磁盘清理建议。

use serde::{Deserialize, Serialize};

/// 空间浪费类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WasteType {
    /// 空目录（无任何文件或子目录）
    EmptyDirectory,
    /// 零字节文件（大小为 0 的文件）
    ZeroByteFile,
    /// 临时文件（.tmp, .temp, .bak 等）
    TemporaryFile,
    /// 过期文件（超过指定天数未修改）
    StaleFile,
    /// 日志文件（.log 文件，可能占用大量空间）
    LogFile,
    /// 缓存文件（node_modules, .cache, __pycache__ 等）
    CacheFile,
    /// 重复文件（内容完全相同的文件）
    DuplicateFile,
    /// 孤立文件（无主程序关联的残留文件）
    OrphanFile,
    /// 长路径文件（路径深度过深，可能影响性能）
    DeepPathFile,
    /// 锁定文件（.lock 文件，可能表示进程异常）
    LockFile,
    /// 压缩归档（可解压后删除原文件的场景）
    RedundantArchive,
    /// 系统残留（卸载后遗留的配置/数据文件）
    SystemResidue,
}

impl WasteType {
    /// 浪费类型的中文标签
    pub fn label(&self) -> &'static str {
        match self {
            Self::EmptyDirectory => "空目录",
            Self::ZeroByteFile => "零字节文件",
            Self::TemporaryFile => "临时文件",
            Self::StaleFile => "过期文件",
            Self::LogFile => "日志文件",
            Self::CacheFile => "缓存文件",
            Self::DuplicateFile => "重复文件",
            Self::OrphanFile => "孤立文件",
            Self::DeepPathFile => "长路径文件",
            Self::LockFile => "锁定文件",
            Self::RedundantArchive => "冗余归档",
            Self::SystemResidue => "系统残留",
        }
    }

    /// 浪费类型的简短描述
    pub fn description(&self) -> &'static str {
        match self {
            Self::EmptyDirectory => "不包含任何内容的空目录",
            Self::ZeroByteFile => "大小为 0 字节，无实际内容",
            Self::TemporaryFile => "临时生成的文件，通常可安全删除",
            Self::StaleFile => "长时间未访问或修改的文件",
            Self::LogFile => "日志文件，可能累积占用大量空间",
            Self::CacheFile => "缓存目录或文件，可重新生成",
            Self::DuplicateFile => "与其他文件内容完全相同",
            Self::OrphanFile => "无主程序关联的残留文件",
            Self::DeepPathFile => "路径层级过深，可能影响性能",
            Self::LockFile => "锁定文件，可能表示进程异常终止",
            Self::RedundantArchive => "压缩包与解压内容同时存在",
            Self::SystemResidue => "软件卸载后遗留的配置或数据",
        }
    }

    /// 建议的清理优先级（1-5，5 为最高）
    pub fn cleanup_priority(&self) -> u8 {
        match self {
            Self::EmptyDirectory => 3,
            Self::ZeroByteFile => 2,
            Self::TemporaryFile => 4,
            Self::StaleFile => 3,
            Self::LogFile => 4,
            Self::CacheFile => 5,
            Self::DuplicateFile => 4,
            Self::OrphanFile => 3,
            Self::DeepPathFile => 2,
            Self::LockFile => 3,
            Self::RedundantArchive => 3,
            Self::SystemResidue => 4,
        }
    }

    /// 是否建议自动清理（true 表示相对安全）
    pub fn safe_to_auto_clean(&self) -> bool {
        match self {
            Self::EmptyDirectory => true,
            Self::ZeroByteFile => true,
            Self::TemporaryFile => true,
            Self::LogFile => true,
            Self::CacheFile => true,
            Self::LockFile => true,
            Self::StaleFile => false,
            Self::DuplicateFile => false,
            Self::OrphanFile => false,
            Self::DeepPathFile => false,
            Self::RedundantArchive => false,
            Self::SystemResidue => false,
        }
    }

    /// 浪费类型对应的颜色（RGB）
    pub fn color(&self) -> [u8; 3] {
        match self {
            Self::EmptyDirectory => [189, 195, 199],  // 灰色
            Self::ZeroByteFile => [149, 165, 166],    // 浅灰
            Self::TemporaryFile => [241, 196, 15],    // 黄色
            Self::StaleFile => [230, 126, 34],        // 橙色
            Self::LogFile => [231, 76, 60],           // 红色
            Self::CacheFile => [155, 89, 182],        // 紫色
            Self::DuplicateFile => [52, 152, 219],    // 蓝色
            Self::OrphanFile => [26, 188, 156],       // 青色
            Self::DeepPathFile => [243, 156, 18],     // 深橙
            Self::LockFile => [236, 112, 99],         // 珊瑚红
            Self::RedundantArchive => [142, 68, 173], // 深紫
            Self::SystemResidue => [127, 140, 141],   // 深灰
        }
    }
}
