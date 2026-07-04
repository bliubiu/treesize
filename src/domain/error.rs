//! 领域错误类型

use thiserror::Error;

/// 领域层错误
#[derive(Debug, Error)]
pub enum DomainError {
    #[error("路径不存在：{0}")]
    PathNotFound(String),

    #[error("路径不可访问：{0}")]
    PathInaccessible(String),

    #[error("路径不是目录：{0}")]
    NotADirectory(String),

    #[error("扫描失败：{0}")]
    ScanFailed(String),

    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),

    #[error("参数错误：{0}")]
    InvalidArgument(String),

    #[error("内存使用超过限制（{0} MB）")]
    MemoryLimitExceeded(u64),

    #[error("扫描时间超过限制（{0} 秒）")]
    TimeLimitExceeded(u64),

    #[error("文件数超过限制（{0} 个）")]
    FileLimitExceeded(u64),

    #[error("用户取消扫描")]
    ScanCancelled,
}

/// 领域层统一返回类型
pub type Result<T> = std::result::Result<T, DomainError>;
