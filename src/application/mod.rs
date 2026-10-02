//! 应用层
//!
//! 编排领域对象完成业务用例，不包含具体技术实现细节。

pub mod classify_service;
pub mod diff_service;
pub mod duplicate_service;
pub mod icicle;
pub mod models;
pub mod report_service;
pub mod scan_service;
pub mod trend_service;
pub mod waste_service;

pub use classify_service::{ClassifyReport, ClassifyService};
pub use diff_service::SnapshotDiffService;
pub use duplicate_service::{DuplicateGroup, DuplicateReport, DuplicateService};
pub use icicle::{compute_layout_v2, render_svg, IcicleBlock, IcicleDirection, LayoutConfig};
pub use models::{CategoryDiff, CategoryTrend, DirDiff, GrowthEntry, SizePoint, SnapshotDiff, TrendReport};
pub use report_service::{ReportService, TopNEntry};
pub use scan_service::ScanService;
pub use trend_service::TrendService;
pub use waste_service::{WasteConfig, WasteReport, WasteService, WasteTypeSummary};
