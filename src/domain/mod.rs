//! 领域层
//!
//! 包含核心业务实体、值对象与领域服务抽象，不依赖任何基础设施。

pub mod drag_collector;
pub mod error;
pub mod file_node;
pub mod scan_engine;
pub mod scan_history;
pub mod value_objects;
pub mod waste_type;

pub use drag_collector::{CollectedItem, DragCollector};
pub use error::{DomainError, Result};
pub use file_node::FileNode;
pub use scan_engine::ScanEngineType;
pub use value_objects::{ByteSize, FileCategory, FileType};
pub use waste_type::WasteType;
