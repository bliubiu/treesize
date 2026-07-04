//! CLI 集成测试
//!
//! 验证 treesize 二进制的端到端行为

use std::fs;
use std::path::PathBuf;

/// 辅助：创建临时测试目录结构
fn create_test_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    fs::write(root.join("a.txt"), "hello world").unwrap(); // 11 bytes
    fs::write(root.join("b.mp4"), "x".repeat(1000)).unwrap(); // 1000 bytes
    fs::write(root.join("c.txt"), "rust").unwrap(); // 4 bytes
    fs::write(root.join("d.zip"), "z".repeat(500)).unwrap(); // 500 bytes

    let sub = root.join("subdir");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("e.rs"), "fn main() {}").unwrap(); // 12 bytes
    fs::write(sub.join("f.rs"), "fn test() {}").unwrap(); // 12 bytes

    let hidden = root.join(".hidden");
    fs::create_dir_all(&hidden).unwrap();
    fs::write(hidden.join("secret.txt"), "password123").unwrap();

    dir
}

fn treesize_bin() -> PathBuf {
    // 优先使用调试构建（开发环境）
    let mut p = std::env::current_dir().unwrap();
    p.push("target");
    p.push("debug");
    p.push("treesize.exe");
    if p.exists() {
        return p;
    }

    // 回退到发布构建
    p = std::env::current_dir().unwrap();
    p.push("target");
    p.push("release");
    p.push("treesize.exe");
    if p.exists() {
        return p;
    }

    panic!(
        "集成测试需要先构建 treesize 二进制：\
         cargo build 或 cargo build --release。\
         查找路径：{}",
        p.display()
    );
}

#[test]
fn cli_tree_output() {
    let dir = create_test_tree();
    let bin = treesize_bin();
    if !bin.exists() {
        eprintln!("跳过集成测试：可执行文件不存在");
        return;
    }

    let output = std::process::Command::new(&bin)
        .args(["--depth", "5", dir.path().to_str().unwrap()])
        .output()
        .expect("执行失败");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("a.txt"), "应包含 a.txt");
    assert!(stdout.contains("b.mp4"), "应包含 b.mp4");
    assert!(stdout.contains("subdir"), "应包含 subdir");
}

#[test]
fn cli_json_output() {
    let dir = create_test_tree();
    let bin = treesize_bin();
    if !bin.exists() {
        eprintln!("跳过集成测试：可执行文件不存在");
        return;
    }

    let output = std::process::Command::new(&bin)
        .args(["-o", "json", "--depth", "5", dir.path().to_str().unwrap()])
        .output()
        .expect("执行失败");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // JSON 输出可能混合了扫描摘要，提取 JSON 部分
    let json_start = stdout.find('{').expect("应包含 JSON 起始");
    let json_str = &stdout[json_start..];
    let json: serde_json::Value = serde_json::from_str(json_str).expect("应为有效 JSON");
    assert!(json["children"].is_array(), "应有 children 数组");
}

#[test]
fn cli_classify_output() {
    let dir = create_test_tree();
    let bin = treesize_bin();
    if !bin.exists() {
        eprintln!("跳过集成测试：可执行文件不存在");
        return;
    }

    let output = std::process::Command::new(&bin)
        .args([
            "--classify",
            "--depth",
            "1",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("执行失败");

    // 分类统计输出到 stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("文件分类统计"), "应包含分类统计");
    assert!(stderr.contains("视频"), "应包含视频分类");
}

#[test]
fn cli_exclude_dir() {
    let dir = create_test_tree();
    let bin = treesize_bin();
    if !bin.exists() {
        eprintln!("跳过集成测试：可执行文件不存在");
        return;
    }

    let output = std::process::Command::new(&bin)
        .args([
            "--depth",
            "5",
            "--exclude-dir",
            "subdir",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("执行失败");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("e.rs"), "排除 subdir 后不应包含 e.rs");
}

#[test]
fn cli_no_hidden() {
    let dir = create_test_tree();
    let bin = treesize_bin();
    if !bin.exists() {
        eprintln!("跳过集成测试：可执行文件不存在");
        return;
    }

    let output = std::process::Command::new(&bin)
        .args([
            "--depth",
            "5",
            "--no-hidden",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("执行失败");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("secret.txt"), "排除隐藏文件后不应包含 secret.txt");
}
