//! treesize 二进制入口
//!
//! 根据 `--mode` 参数或子命令分发到 CLI 或 GUI 模式
//! 双击运行（无参数）时自动启动 GUI 模式

use std::process::ExitCode;

use clap::Parser;
use treesize::cli::{CliArgs, CliMode};

fn main() -> ExitCode {
    let args = CliArgs::parse();

    let mode = if is_double_clicked() && matches!(args.mode, CliMode::Cli) && args.path.is_none() {
        CliMode::Gui
    } else {
        args.mode
    };

    let _log_guard = match treesize::infrastructure::logging::init(&args.log_dir, args.log_level) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("初始化日志失败：{e}");
            return ExitCode::FAILURE;
        },
    };

    tracing::info!("treesize 启动，运行模式：{:?}", mode);

    let result = match mode {
        CliMode::Cli => treesize::cli::run(&args),
        CliMode::Gui => treesize::gui::run(&args),
    };

    match result {
        Ok(()) => {
            tracing::info!("treesize 正常退出");
            ExitCode::SUCCESS
        },
        Err(e) => {
            tracing::error!("treesize 异常退出：{e}");
            eprintln!("错误：{e}");
            ExitCode::FAILURE
        },
    }
}

/// 判断是否为双击运行（无参数启动）
fn is_double_clicked() -> bool {
    std::env::args().len() == 1
}
