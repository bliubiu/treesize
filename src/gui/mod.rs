//! GUI 图形界面
//!
//! 基于 egui + eframe 实现。Phase 1 提供最小骨架，Phase 2 实现完整 Treemap。

pub mod app;
pub mod fonts;
pub mod render;
pub mod sunburst;
pub mod theme;
pub mod treemap;
pub mod widgets;

use crate::cli::CliArgs;
use crate::domain::error::Result;

/// GUI 入口
pub fn run(args: &CliArgs) -> Result<()> {
    let path = args.path.clone().unwrap_or_else(|| std::path::PathBuf::from("."));

    let options = app::GuiOptions {
        initial_path: path,
        scan_options: args.to_scan_options(),
    };

    let mut native_options = eframe::NativeOptions::default();
    native_options.viewport = native_options
        .viewport
        .with_inner_size([1200.0, 800.0])
        .with_title("treesize - 磁盘占用分析");

    // 注意：run_native 的第一个参数仅用于存储路径，需保持 ASCII 避免中文目录名导致权限错误
    if let Err(e) = eframe::run_native(
        "treesize",
        native_options,
        Box::new(move |cc| Box::new(app::TreeSizeApp::new(cc, options))),
    ) {
        return Err(crate::domain::error::DomainError::ScanFailed(format!(
            "GUI 启动失败：{e}"
        )));
    }

    Ok(())
}
