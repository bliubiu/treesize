//! 领域值对象
//!
//! 不可变的、自校验的值对象。所有大小以字节为单位存储，仅在展示时格式化。

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// 字节大小值对象
///
/// 内部以 `u64` 字节存储，提供人类可读格式化与常用换算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct ByteSize(pub u64);

impl ByteSize {
    pub const fn from_bytes(b: u64) -> Self {
        Self(b)
    }

    pub const fn bytes(self) -> u64 {
        self.0
    }

    pub const fn kib(self) -> f64 {
        self.0 as f64 / 1024.0
    }

    pub const fn mib(self) -> f64 {
        self.kib() / 1024.0
    }

    pub const fn gib(self) -> f64 {
        self.mib() / 1024.0
    }

    pub const fn tib(self) -> f64 {
        self.gib() / 1024.0
    }

    /// 人类可读的二进制单位字符串，例如 `1.23 GiB`。
    pub fn human_readable(self) -> String {
        const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
        let mut size = self.0 as f64;
        let mut idx = 0;
        while size >= 1024.0 && idx < UNITS.len() - 1 {
            size /= 1024.0;
            idx += 1;
        }
        if idx == 0 {
            format!("{} {}", self.0, UNITS[0])
        } else {
            format!("{:.2} {}", size, UNITS[idx])
        }
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.human_readable())
    }
}

impl From<u64> for ByteSize {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// 文件大类，用于分类统计与 Treemap 着色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileCategory {
    /// 视频
    Video,
    /// 音频
    Audio,
    /// 图片
    Image,
    /// 文档
    Document,
    /// 压缩包
    Archive,
    /// 可执行程序
    Executable,
    /// 源代码
    Source,
    /// 数据库
    Database,
    /// 系统文件
    System,
    /// 其他
    Other,
}

impl FileCategory {
    /// 根据扩展名推断大类
    pub fn from_extension(ext: &str) -> Self {
        let ext = ext.to_ascii_lowercase();
        match ext.as_str() {
            // 视频
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg"
            | "ts" | "rmvb" | "rm" => Self::Video,
            // 音频
            "mp3" | "flac" | "wav" | "aac" | "ogg" | "wma" | "m4a" | "ape" | "opus" | "aiff" => {
                Self::Audio
            }
            // 图片
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "tif" | "webp" | "heic" | "heif"
            | "svg" | "ico" | "psd" | "raw" | "cr2" | "nef" => Self::Image,
            // 文档
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
            | "rtf" | "txt" | "md" | "csv" | "epub" | "djvu" | "pages" | "numbers" | "key" => {
                Self::Document
            }
            // 压缩包
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "lz4" | "iso" | "cab" => {
                Self::Archive
            }
            // 可执行
            "exe" | "msi" | "app" | "dmg" | "deb" | "rpm" | "apk" | "appimage" | "bat" | "cmd"
            | "ps1" | "sh" => Self::Executable,
            // 源代码
            "rs" | "go" | "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "java" | "kt" | "py"
            | "js" | "tsx" | "jsx" | "rb" | "php" | "swift" | "scala" | "lua" | "pl"
            | "cs" | "vb" | "fs" | "clj" | "ex" | "exs" | "elm" | "dart" | "gradle" | "sbt"
            | "toml" | "yaml" | "yml" | "json" | "xml" | "ini" | "cfg" | "conf" => Self::Source,
            // 数据库
            "db" | "sqlite" | "sqlite3" | "mdb" | "accdb" | "dbf" | "sql" | "bson" => {
                Self::Database
            }
            // 系统
            "dll" | "so" | "dylib" | "sys" | "ko" | "drv" | "lnk" | "tmp" | "log" | "lock" => {
                Self::System
            }
            _ => Self::Other,
        }
    }

    /// 用于 Treemap 着色的基础色（RGB）
    pub fn base_color(self) -> [u8; 3] {
        match self {
            Self::Video => [231, 76, 60],
            Self::Audio => [243, 156, 18],
            Self::Image => [46, 204, 113],
            Self::Document => [52, 152, 219],
            Self::Archive => [155, 89, 182],
            Self::Executable => [236, 112, 99],
            Self::Source => [26, 188, 156],
            Self::Database => [241, 196, 15],
            Self::System => [149, 165, 166],
            Self::Other => [127, 140, 141],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Video => "视频",
            Self::Audio => "音频",
            Self::Image => "图片",
            Self::Document => "文档",
            Self::Archive => "压缩包",
            Self::Executable => "可执行",
            Self::Source => "源代码",
            Self::Database => "数据库",
            Self::System => "系统",
            Self::Other => "其他",
        }
    }
}

/// 文件类型分类：目录或文件
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileType {
    Directory,
    File,
}

/// 规范化扩展名（小写、不含点），目录返回空字符串
pub fn normalize_extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// 取文件名（含目录），失败返回空字符串
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_size_format() {
        assert_eq!(ByteSize(0).human_readable(), "0 B");
        assert_eq!(ByteSize(512).human_readable(), "512 B");
        assert_eq!(ByteSize(1024).human_readable(), "1.00 KiB");
        assert_eq!(ByteSize(1024 * 1024).human_readable(), "1.00 MiB");
        assert_eq!(ByteSize(1024 * 1024 * 1024).human_readable(), "1.00 GiB");
    }

    #[test]
    fn category_from_extension() {
        assert_eq!(FileCategory::from_extension("mp4"), FileCategory::Video);
        assert_eq!(FileCategory::from_extension("MP3"), FileCategory::Audio);
        assert_eq!(FileCategory::from_extension("rs"), FileCategory::Source);
        assert_eq!(FileCategory::from_extension("unknown"), FileCategory::Other);
    }

    #[test]
    fn normalize_extension_works() {
        assert_eq!(normalize_extension(Path::new("/a/b/c.RS")), "rs");
        assert_eq!(normalize_extension(Path::new("/a/b/README")), "");
    }
}
