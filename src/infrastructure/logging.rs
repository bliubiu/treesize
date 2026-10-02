//! 日志系统初始化
//!
//! 提供多级别日志（DEBUG/INFO/ERROR）、文件输出与日志轮转。
//! 文件命名格式：`treesize-YYYYMMDD.log`，默认保留 32 天。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};
use tracing_core::{Event, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

// ─── 日志级别 ──────────────────────────────────────────────────────────────

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Error,
}

impl Default for LogLevel {
    fn default() -> Self {
        Self::Info
    }
}

impl LogLevel {
    /// 转换为 `EnvFilter` 使用的字符串
    fn as_filter(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Error => "error",
        }
    }
}

// ─── 日志 guard ───────────────────────────────────────────────────────────

/// 日志初始化返回的 guard，drop 时关闭文件句柄
pub struct LogGuard {
    _file_guard: tracing_appender::non_blocking::WorkerGuard,
}

// ─── 日志文件轮转器（每日轮转，文件名：treesize-YYYYMMDD.log）─────────────

/// 支持每日轮转的文件写入器。
///
/// 每天第一次写入时会按日期创建 `treesize-YYYYMMDD.log` 文件，
/// 后续写入追加到同一文件。仅被 `tracing_appender::non_blocking`
/// 的后台线程单线程调用，无需锁。
struct DailyLogRotator {
    log_dir: PathBuf,
    current_date: String,
    file: Option<File>,
}

impl DailyLogRotator {
    fn new(log_dir: PathBuf) -> Self {
        Self {
            log_dir,
            current_date: String::new(),
            file: None,
        }
    }

    /// 获取当前日期的文件句柄，必要时创建新文件
    fn get_file(&mut self) -> io::Result<&mut File> {
        let today = Local::now().format("%Y%m%d").to_string();
        if self.current_date != today {
            let path = self.log_dir.join(format!("treesize-{today}.log"));
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            self.file = Some(file);
            self.current_date = today;
        }
        // 仅在初始化或日期变更后 file 才会为 None
        Ok(self.file.as_mut().expect("日志文件句柄不应为 None"))
    }
}

impl Write for DailyLogRotator {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.get_file()?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(ref mut file) = self.file {
            file.flush()
        } else {
            Ok(())
        }
    }
}

// DailyLogRotator 只被 tracing_appender 后台线程访问，Send 足够了
// File 在 Windows 上是 Send，PathBuf 和 String 都是 Send
// 不需要实现 Sync

// ─── 自定义日志格式化器 ──────────────────────────────────────────────────

/// 自定义日志事件格式化器。
///
/// 输出格式（完全匹配规范）：
/// `[YYYY-MM-DD HH:MM:SS.SSS] [级别] [线程ID] [模块:行号] - 日志内容`
#[derive(Clone, Copy)]
struct TreesizeFormat;

impl<S, N> FormatEvent<S, N> for TreesizeFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> std::fmt::Result {
        let meta = event.metadata();
        let now = Local::now();

        // [YYYY-MM-DD HH:MM:SS.SSS]
        write!(
            &mut writer,
            "[{}.{:03}]",
            now.format("%Y-%m-%d %H:%M:%S"),
            now.timestamp_subsec_millis(),
        )?;

        // [级别]
        write!(&mut writer, " [{}]", meta.level())?;

        // [线程ID]
        write!(&mut writer, " [{:?}]", std::thread::current().id())?;

        // [模块:行号]
        let module = meta.module_path().unwrap_or(meta.target());
        if let Some(line) = meta.line() {
            write!(&mut writer, " [{}:{line}]", module)?;
        } else {
            write!(&mut writer, " [{}]", module)?;
        }

        // - 内容
        write!(&mut writer, " - ")?;

        // 格式化字段（消息内容）
        ctx.format_fields(writer.by_ref(), event)?;

        // 换行
        writeln!(writer)
    }
}

// ─── 初始化 ───────────────────────────────────────────────────────────────

/// 初始化日志系统
///
/// `log_dir` 为日志文件目录，`level` 为日志级别。
/// 同时输出到文件（按日期轮转）和终端。
/// 自动注册 panic hook 将 panic 信息记录到日志。
pub fn init(log_dir: &Path, level: LogLevel) -> Result<LogGuard, String> {
    std::fs::create_dir_all(log_dir).map_err(|e| format!("创建日志目录失败：{e}"))?;

    // ── 清理过期日志（32 天前） ──
    cleanup_old_logs(log_dir);

    // ── 文件写入器（每日轮转） ──
    let file_writer = DailyLogRotator::new(log_dir.to_path_buf());
    let (non_blocking_file, file_guard) = tracing_appender::non_blocking(file_writer);

    // ── 过滤器 ──
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level.as_filter()));

    // ── 自定义格式 ──
    let treesize_format = TreesizeFormat;

    // ── stderr 层（终端输出到 stderr，避免污染 stdout） ──
    let stderr_layer = fmt::layer().event_format(treesize_format).with_writer(io::stderr);

    // ── 文件层 ──
    let file_layer = fmt::layer()
        .event_format(treesize_format)
        .with_writer(non_blocking_file)
        .with_ansi(false);

    // ── 注册订阅器 ──
    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
        .map_err(|e| format!("初始化日志订阅器失败：{e}"))?;

    // ── 注册 panic hook ──
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        prev_hook(panic_info);

        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().map(|s| s.to_string()))
            .unwrap_or_else(|| "未知 panic 载荷".to_string());

        if let Some(location) = panic_info.location() {
            tracing::error!(
                target: "treesize::panic",
                "panic at {}:{}:{} — {}",
                location.file(),
                location.line(),
                location.column(),
                payload,
            );
        } else {
            tracing::error!(target: "treesize::panic", "panic: {}", payload);
        }
    }));

    Ok(LogGuard {
        _file_guard: file_guard,
    })
}

// ─── 日志清理 ─────────────────────────────────────────────────────────────

/// 清理超过 32 天的日志文件
fn cleanup_old_logs(log_dir: &Path) {
    let today = Local::now().naive_local().date();

    if let Ok(entries) = fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let file_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_owned(),
                None => continue,
            };

            // 只匹配 treesize-YYYYMMDD.log
            if let Some(date_str) = file_name.strip_prefix("treesize-").and_then(|s| s.strip_suffix(".log")) {
                if date_str.len() == 8 && date_str.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y%m%d") {
                        let days_old = (today - date).num_days();
                        if days_old > 32 {
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;

    // ── LogLevel ───────────────────────────────────────────────────────

    #[test]
    fn log_level_default_is_info() {
        assert_eq!(LogLevel::default(), LogLevel::Info);
    }

    #[test]
    fn log_level_as_filter() {
        assert_eq!(LogLevel::Debug.as_filter(), "debug");
        assert_eq!(LogLevel::Info.as_filter(), "info");
        assert_eq!(LogLevel::Error.as_filter(), "error");
    }

    #[test]
    fn log_level_debug_equality() {
        assert_eq!(LogLevel::Debug, LogLevel::Debug);
        assert_ne!(LogLevel::Debug, LogLevel::Info);
    }

    // ── DailyLogRotator ────────────────────────────────────────────────

    #[test]
    fn daily_log_rotator_creates_file_on_write() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().to_path_buf();

        let mut rotator = DailyLogRotator::new(log_dir.clone());
        let today = chrono::Local::now().format("%Y%m%d").to_string();

        // 写入内容
        writeln!(rotator, "test log entry").unwrap();
        rotator.flush().unwrap();

        // 验证文件已创建
        let expected_path = log_dir.join(format!("treesize-{today}.log"));
        assert!(expected_path.exists(), "日志文件应已创建");

        let content = fs::read_to_string(&expected_path).unwrap();
        assert_eq!(content, "test log entry\n");
    }

    #[test]
    fn daily_log_rotator_appends_to_same_file() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().to_path_buf();

        let mut rotator = DailyLogRotator::new(log_dir.clone());

        writeln!(rotator, "line 1").unwrap();
        writeln!(rotator, "line 2").unwrap();
        rotator.flush().unwrap();

        let today = chrono::Local::now().format("%Y%m%d").to_string();
        let expected_path = log_dir.join(format!("treesize-{today}.log"));
        let content = fs::read_to_string(&expected_path).unwrap();
        assert_eq!(content, "line 1\nline 2\n");
    }

    #[test]
    fn daily_log_rotator_flush_without_file() {
        // 未初始化时 flush 不应报错
        let dir = tempdir().unwrap();
        let mut rotator = DailyLogRotator::new(dir.path().to_path_buf());
        rotator.flush().unwrap();
    }

    #[test]
    fn daily_log_rotator_write_multiple_chunks() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().to_path_buf();
        let mut rotator = DailyLogRotator::new(log_dir.clone());

        // 分多次写入
        rotator.write_all(b"hello ").unwrap();
        rotator.write_all(b"world").unwrap();
        rotator.flush().unwrap();

        let today = chrono::Local::now().format("%Y%m%d").to_string();
        let content = fs::read_to_string(&log_dir.join(format!("treesize-{today}.log"))).unwrap();
        assert_eq!(content, "hello world");
    }

    // ── 日志清理 ───────────────────────────────────────────────────────

    #[test]
    fn cleanup_old_logs_removes_expired_files() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path();

        // 创建两个日志文件：一个过期的（33 天前），一个当天的
        let old_date = (chrono::Local::now() - chrono::Duration::days(33))
            .format("%Y%m%d")
            .to_string();
        let today = chrono::Local::now().format("%Y%m%d").to_string();

        fs::write(log_dir.join(format!("treesize-{old_date}.log")), "old").unwrap();
        fs::write(log_dir.join(format!("treesize-{today}.log")), "current").unwrap();

        // 执行清理
        cleanup_old_logs(log_dir);

        // 过期文件应被删除
        assert!(
            !log_dir.join(format!("treesize-{old_date}.log")).exists(),
            "33 天前的日志应被清理"
        );
        // 当天文件应保留
        assert!(log_dir.join(format!("treesize-{today}.log")).exists(), "当天日志应保留");
    }

    #[test]
    fn cleanup_old_logs_keeps_recent_files() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path();

        // 创建多个不过期的日志（32 天内）
        let recent_date = (chrono::Local::now() - chrono::Duration::days(30))
            .format("%Y%m%d")
            .to_string();
        fs::write(log_dir.join(format!("treesize-{recent_date}.log")), "recent").unwrap();

        cleanup_old_logs(log_dir);

        assert!(
            log_dir.join(format!("treesize-{recent_date}.log")).exists(),
            "30 天前的日志不应被清理"
        );
    }

    #[test]
    fn cleanup_old_logs_ignores_non_log_files() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path();

        // 创建非日志格式的文件
        fs::write(log_dir.join("treesize-20230101.txt"), "wrong extension").unwrap();
        fs::write(log_dir.join("notes.log"), "wrong prefix").unwrap();
        fs::write(log_dir.join("other_file.bak"), "backup").unwrap();

        // 清理不应删除这些文件
        cleanup_old_logs(log_dir);

        assert!(log_dir.join("treesize-20230101.txt").exists());
        assert!(log_dir.join("notes.log").exists());
        assert!(log_dir.join("other_file.bak").exists());
    }

    #[test]
    fn cleanup_old_logs_handles_empty_dir() {
        let dir = tempdir().unwrap();
        // 空目录不应 panic
        cleanup_old_logs(dir.path());
    }

    #[test]
    fn cleanup_old_logs_handles_nonexistent_dir() {
        // 不存在的目录不应 panic
        cleanup_old_logs(Path::new("/nonexistent/log/dir/xyz"));
    }

    #[test]
    fn cleanup_old_logs_exactly_32_days_is_kept() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path();

        // 恰好 32 天前（边界条件）
        let exact_old = (chrono::Local::now() - chrono::Duration::days(32))
            .format("%Y%m%d")
            .to_string();
        fs::write(log_dir.join(format!("treesize-{exact_old}.log")), "boundary").unwrap();

        cleanup_old_logs(log_dir);

        // 32 天应保留（条件是 > 32 才删除）
        assert!(
            log_dir.join(format!("treesize-{exact_old}.log")).exists(),
            "恰好 32 天的日志应保留"
        );
    }

    #[test]
    fn cleanup_old_logs_33_days_is_removed() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path();

        let old = (chrono::Local::now() - chrono::Duration::days(33))
            .format("%Y%m%d")
            .to_string();
        fs::write(log_dir.join(format!("treesize-{old}.log")), "to delete").unwrap();

        cleanup_old_logs(log_dir);

        assert!(!log_dir.join(format!("treesize-{old}.log")).exists(), "33 天前应被删除");
    }

    // ── LogGuard ───────────────────────────────────────────────────────

    #[test]
    fn log_guard_is_send() {
        // 编译期验证：LogGuard 实现了 Send
        fn assert_send<T: Send>() {}
        assert_send::<LogGuard>();
    }
}
