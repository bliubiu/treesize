use egui::{Color32, Rect, Vec2};

use crate::domain::FileNode;
use crate::gui::theme::ThemeColors;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TreeSortBy {
    SizeDesc,
    SizeAsc,
    NameAsc,
    NameDesc,
}

#[allow(clippy::too_many_arguments)]
pub fn render_tree_node(
    ui: &mut egui::Ui,
    node: &FileNode,
    parent_size: u64,
    depth: usize,
    show_hidden: bool,
    search_text: &str,
    search_lower: &str,
    case_sensitive: bool,
    match_full: bool,
    theme: &ThemeColors,
    expanded: &mut std::collections::HashSet<String>,
    sort_by: TreeSortBy,
    delete_request: &mut Option<std::path::PathBuf>,
) {
    if depth > 0 && !show_hidden && node.name.starts_with('.') {
        return;
    }

    let matches = matches_search_opt(node.name.as_str(), search_text, search_lower, case_sensitive, match_full);
    if !matches && !search_text.is_empty() {
        let has_matching_child = node.children.iter().any(|c| {
            matches_search_opt(c.name.as_str(), search_text, search_lower, case_sensitive, match_full)
        });
        if !has_matching_child {
            return;
        }
    }

    let percent = if parent_size > 0 && node.size.0 > 0 {
        (node.size.0 as f64 / parent_size as f64) * 100.0
    } else if node.size.0 > 0 {
        100.0
    } else {
        0.0
    };
    let icon = if node.is_dir() { "📁" } else { "📄" };

    let is_expandable = node.is_dir() && !node.children.is_empty();
    let key = node.path.to_string_lossy().to_string();
    let is_expanded = expanded.contains(&key);

    let row_height = 20.0;
    let indent = depth as f32 * 18.0;

    ui.horizontal(|ui| {
        ui.add_space(indent);

        if parent_size > 0 {
            let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(4.0, row_height), egui::Sense::hover());
            let [r, g, b] = node.category.base_color();
            let cat_color = Color32::from_rgb(r, g, b);
            ui.painter().rect_filled(bar_rect, 1.0, theme.bar_track);
            let ratio = (node.size.0 as f64 / parent_size as f64) as f32;
            let fill_h = (row_height * ratio).max(2.0).min(row_height);
            let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(4.0, fill_h));
            ui.painter().rect_filled(fill_rect, 1.0, cat_color);
        } else {
            ui.add_space(4.0);
        }
        ui.add_space(4.0);

        if is_expandable {
            let arrow = if is_expanded { "▼" } else { "▶" };
            if ui.add(
                egui::Label::new(
                    egui::RichText::new(arrow).color(theme.text_secondary),
                ).sense(egui::Sense::click())
            ).clicked() {
                if is_expanded {
                    expanded.remove(&key);
                } else {
                    expanded.insert(key);
                }
            }
        } else {
            ui.add_space(12.0);
        }

        let percent_color = if percent > 50.0 {
            theme.danger
        } else if percent > 20.0 {
            theme.warn
        } else {
            theme.text_secondary
        };

        let name_label = egui::RichText::new(format!("{} {}  ", icon, node.name))
            .color(if node.is_dir() { theme.accent } else { theme.text_primary });
        let name_response = ui.add(
            egui::Label::new(name_label)
                .sense(egui::Sense::click())
        );

        if name_response.clicked() && node.is_dir() {
            open_in_explorer(&node.path);
        }

        if node.is_dir() && name_response.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }

        let size_label = egui::RichText::new(format!("{}", node.size))
            .color(theme.text_secondary)
            .family(egui::FontFamily::Monospace);
        let pct_label = egui::RichText::new(format!(" {:.1}%", percent))
            .color(percent_color)
            .family(egui::FontFamily::Monospace);

        ui.label(size_label);
        ui.label(pct_label);

        let resp = ui.label("");
        resp.context_menu(|ui| {
            if ui.button("复制路径").clicked() {
                ui.output_mut(|o| o.copied_text = node.path.display().to_string());
                ui.close_menu();
            }
            if ui.button("复制名称").clicked() {
                ui.output_mut(|o| o.copied_text = node.name.clone());
                ui.close_menu();
            }
            ui.separator();
            if ui.button("🗑 删除").clicked() {
                *delete_request = Some(node.path.clone());
                ui.close_menu();
            }
        });
    });

    if is_expanded {
        let mut children: Vec<&FileNode> = node.children.iter().collect();
        sort_children(&mut children, sort_by);
        for child in children {
            render_tree_node(
                ui,
                child,
                node.size.0,
                depth + 1,
                show_hidden,
                search_text,
                search_lower,
                case_sensitive,
                match_full,
                theme,
                expanded,
                sort_by,
                delete_request,
            );
        }
    }
}

fn sort_children(children: &mut Vec<&FileNode>, sort_by: TreeSortBy) {
    match sort_by {
        TreeSortBy::SizeDesc => {
            children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        }
        TreeSortBy::SizeAsc => {
            children.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| a.name.cmp(&b.name)));
        }
        TreeSortBy::NameAsc => {
            children.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| b.size.cmp(&a.size)));
        }
        TreeSortBy::NameDesc => {
            children.sort_by(|a, b| b.name.cmp(&a.name).then_with(|| b.size.cmp(&a.size)));
        }
    }
}

fn matches_search_opt(
    name: &str,
    search: &str,
    search_lower: &str,
    case_sensitive: bool,
    match_full: bool,
) -> bool {
    if search_lower.is_empty() {
        return true;
    }
    let name_lower = name.to_lowercase();
    let name_to_check = if case_sensitive { name } else { name_lower.as_str() };
    let search_to_check = if case_sensitive { search } else { search_lower };
    if match_full {
        name_to_check == search_to_check
    } else {
        name_to_check.contains(search_to_check)
    }
}

#[cfg(target_os = "windows")]
fn open_in_explorer(path: &std::path::Path) {
    let _ = std::process::Command::new("explorer")
        .arg(path)
        .spawn();
}

#[cfg(target_os = "macos")]
fn open_in_explorer(path: &std::path::Path) {
    let _ = std::process::Command::new("open")
        .arg(path)
        .spawn();
}

#[cfg(target_os = "linux")]
fn open_in_explorer(path: &std::path::Path) {
    let _ = std::process::Command::new("xdg-open")
        .arg(path)
        .spawn();
}