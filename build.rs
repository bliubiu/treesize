//! 构建脚本
//!
//! 检测 assets/ 目录下的中文字体文件，按优先级选择并设置编译标志，
//! 使 GUI 模式内嵌开源中文字体（SIL 协议，无版权风险）。
//!
//! 优先级顺序：
//!   1. assets/NotoSansSC.ttf         — Google Noto Sans SC，TTF 格式
//!   2. assets/SourceHanSansSC-VF.ttf — Adobe Source Han Sans SC，可变字体
//!   3. assets/SourceHanSansCN-Regular.otf — Adobe Source Han Sans CN，OTF 格式
//!
//! 检测到第一个存在的文件即停止，并设置对应的 cfg 标志。

use std::path::Path;

fn main() {
    // 字体候选列表： (路径, cfg_flag)
    const FONT_CANDIDATES: &[(&str, &str)] = &[
        ("assets/NotoSansSC.ttf", "font_ntsc_ttf"),
        ("assets/SourceHanSansSC-VF.ttf", "font_shsc_vf_ttf"),
        ("assets/SourceHanSansCN-Regular.otf", "font_shcn_otf"),
    ];

    let mut detected = false;
    for (path, cfg_flag) in FONT_CANDIDATES {
        if Path::new(path).exists() {
            println!("cargo:rustc-cfg=embedded_font");
            println!("cargo:rustc-cfg={}", cfg_flag);
            println!("cargo:warning=检测到 {}，将内嵌中文字体", path);
            detected = true;
            break;
        }
    }

    if !detected {
        println!("cargo:warning=未检测到字体文件（assets/NotoSansSC.ttf 等），GUI 将使用系统字体");
    }

    // 注册所有字体路径变更检测
    for (path, _) in FONT_CANDIDATES {
        println!("cargo:rerun-if-changed={}", path);
    }
}