use egui::{Context, FontDefinitions, FontData, FontFamily, FontId, TextStyle};

pub fn setup_chinese_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();

    load_chinese_font(&mut fonts);
    load_monospace_font(&mut fonts);

    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(TextStyle::Body, FontId::new(18.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Button, FontId::new(18.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Small, FontId::new(16.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Monospace, FontId::new(18.0, FontFamily::Monospace));
    ctx.set_style(style);
}

fn load_monospace_font(fonts: &mut FontDefinitions) {
    let candidates: [(&str, &str); 4] = [
        ("C:\\Windows\\Fonts\\cascadia.ttf", "Cascadia Code"),
        ("C:\\Windows\\Fonts\\consola.ttf", "Consolas"),
        ("/System/Library/Fonts/Menlo.ttc", "Menlo"),
        ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "DejaVu Sans Mono"),
    ];

    for (path, name) in candidates.iter() {
        if let Ok(data) = std::fs::read(path) {
            let font_id = name.to_string();
            fonts.font_data.insert(font_id.clone(), FontData::from_owned(data));
            fonts
                .families
                .entry(FontFamily::Monospace)
                .and_modify(|list| list.insert(0, font_id.clone()));
            tracing::info!(target: "treesize::gui", "加载等宽字体：{}", path);
            return;
        }
    }

    tracing::debug!(target: "treesize::gui", "未找到专用等宽字体，使用 egui 默认 Monospace");
}

#[cfg(embedded_font)]
fn load_chinese_font(fonts: &mut FontDefinitions) {
    #[cfg(font_ntsc_ttf)]
    let font_data = include_bytes!("../../assets/NotoSansSC.ttf");
    #[cfg(font_shsc_vf_ttf)]
    let font_data = include_bytes!("../../assets/SourceHanSansSC-VF.ttf");
    #[cfg(font_shcn_otf)]
    let font_data = include_bytes!("../../assets/SourceHanSansCN-Regular.otf");

    register_font(fonts, "NotoSansSC", font_data.to_vec());
    tracing::info!(target: "treesize::gui", "使用内嵌字体 Noto Sans SC");
}

#[cfg(not(embedded_font))]
fn load_chinese_font(fonts: &mut FontDefinitions) {
    let candidates = [
        "C:\\Windows\\Fonts\\NotoSansSC-Regular.ttf",
        "C:\\Windows\\Fonts\\NotoSansSC.ttf",
        "/usr/share/fonts/noto-cjk/NotoSansSC-Regular.otf",
        "/usr/share/fonts/opentype/noto/NotoSansSC-Regular.ttf",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
    ];

    let mut loaded = false;
    for font_path in &candidates {
        if let Ok(font_data) = std::fs::read(font_path) {
            let name = std::path::Path::new(font_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("chinese_font");
            register_font(fonts, name, font_data);
            tracing::info!(target: "treesize::gui", "加载系统字体：{}", font_path);
            loaded = true;
            break;
        }
    }

    if !loaded {
        tracing::warn!(target: "treesize::gui", "未找到中文字体，中文可能显示为方块。建议将 Noto Sans SC 放入 assets/ 目录后重新编译");
    }
}

fn register_font(fonts: &mut FontDefinitions, name: &str, data: Vec<u8>) {
    let font_id = name.to_string();
    fonts.font_data.insert(font_id.clone(), FontData::from_owned(data));
    fonts
        .families
        .entry(FontFamily::Proportional)
        .and_modify(|list| list.push(font_id.clone()));
    fonts
        .families
        .entry(FontFamily::Monospace)
        .and_modify(|list| list.push(font_id));
}