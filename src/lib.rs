//! treesize 库入口
//!
//! 按 DDD 分层组织：
//! - `domain`：领域层，核心实体与值对象
//! - `application`：应用层，编排业务用例
//! - `infrastructure`：基础设施层，文件系统与日志实现
//! - `cli`：命令行界面
//! - `gui`：图形界面

pub mod application;
pub mod cli;
pub mod domain;
pub mod gui;
pub mod infrastructure;

#[cfg(test)]
pub mod test_utils;

pub use domain::error::{DomainError, Result};
