use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use std::path::Path;

use eframe::egui;
use egui::{Color32, Vec2};

use crate::application::{
    ClassifyReport, ClassifyService, DuplicateReport, DuplicateService, ReportService, ScanService,
    SnapshotDiffService, TopNEntry, TrendService, WasteReport, WasteService,
};
use crate::domain::scan_engine::{CancelToken, ScanOptions, ScanProgress, ScanStats};
use crate::application::models::{SnapshotDiff, TrendReport};
use crate::domain::value_objects::ByteSize;
use crate::domain::DragCollector;
use crate::infrastructure::HistoryStorage;

#[cfg(target_os = "windows")]
use crate::infrastructure::{FsScanEngine, MftScanEngine, UsnScanEngine};
#[cfg(not(target_os = "windows"))]
use crate::infrastructure::FsScanEngine;

use super::render;
use super::widgets::*;
use super::sunburst::SunburstView;
use super::treemap::TreemapView;
use super::theme::{apply_theme, ThemeColors};
use super::fonts::setup_chinese_fonts;

pub struct GuiOptions {
    pub initial_path: PathBuf,
    pub scan_options: ScanOptions,
}

#[derive(Default)]
struct ScanState {
    node: Option<Arc<crate::domain::FileNode>>,
    stats: Option<ScanStats>,
    progress: ScanProgress,
    running: bool,
    error: Option<String>,
    cancel: CancelToken,
    started_at: Option<SystemTime>,
    generation: u64,
}

pub struct Snapshot {
    pub node: Option<Arc<crate::domain::FileNode>>,
    pub stats: Option<ScanStats>,
    pub progress: ScanProgress,
    pub running: bool,
    pub error: Option<String>,
    pub generation: u64,
}

#[derive(Default)]
struct ComputeCache {
    generation: u64,
    top_files: Option<Vec<TopNEntry>>,
    top_dirs: Option<Vec<TopNEntry>>,
    classify: Option<ClassifyReport>,
    duplicates: Option<DuplicateReport>,
    waste: Option<WasteReport>,
    all_files: Option<Vec<TopNEntry>>,
}

impl ScanState {
    fn shared() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }
}

pub struct TreeSizeApp {
    path_input: String,
    options: ScanOptions,
    state: Arc<Mutex<ScanState>>,
    selected_tab: Tab,
    top_n: usize,
    show_hidden: bool,
    search_text: String,
    search_case_sensitive: bool,
    search_match_full: bool,
    font_scale: f32,
    dark_mode: bool,
    show_settings: bool,
    theme_dirty: bool,
    theme: ThemeColors,
    expanded_paths: std::collections::HashSet<String>,
    tree_sort_by: render::tree::TreeSortBy,
    path_history: std::collections::VecDeque<String>,
    cache_generation: u64,
    cache_top_n: usize,
    cached_top_files: Option<Vec<TopNEntry>>,
    cached_top_dirs: Option<Vec<TopNEntry>>,
    cached_classify: Option<ClassifyReport>,
    cached_duplicates: Option<DuplicateReport>,
    cached_waste: Option<WasteReport>,
    cached_all_files: Option<Vec<TopNEntry>>,
    top_files_sort_by: TopFilesSortBy,
    top_files_search: String,
    dashboard_hover: Option<usize>,
    delete_confirm_path: Option<std::path::PathBuf>,
    delete_error: Option<String>,
    prev_running: bool,
    history_storage: Option<HistoryStorage>,
    cached_trend: Option<TrendReport>,
    cached_diff: Option<SnapshotDiff>,
    diff_old_selected: Option<i64>,
    diff_new_selected: Option<i64>,
    drag_collector: DragCollector,
    cache_computing: bool,
    cache_generation_computing: u64,
    compute_cache: Arc<Mutex<ComputeCache>>,
    last_repaint: Instant,
}

#[derive(Clone)]
pub(crate) enum TopFilesSortBy {
    SizeDesc,
    SizeAsc,
    NameAsc,
    NameDesc,
    PercentDesc,
    PercentAsc,
    CategoryAsc,
    CategoryDesc,
    ModifiedDesc,
    ModifiedAsc,
}

impl TopFilesSortBy {
    pub(crate) fn toggle(&self) -> Self {
        match self {
            Self::SizeDesc => Self::SizeAsc,
            Self::SizeAsc => Self::SizeDesc,
            Self::NameAsc => Self::NameDesc,
            Self::NameDesc => Self::NameAsc,
            Self::PercentDesc => Self::PercentAsc,
            Self::PercentAsc => Self::PercentDesc,
            Self::CategoryAsc => Self::CategoryDesc,
            Self::CategoryDesc => Self::CategoryAsc,
            Self::ModifiedDesc => Self::ModifiedAsc,
            Self::ModifiedAsc => Self::ModifiedDesc,
        }
    }

    pub(crate) fn is_asc(&self) -> bool {
        matches!(
            self,
            Self::SizeAsc | Self::NameAsc | Self::PercentAsc | Self::CategoryAsc | Self::ModifiedAsc
        )
    }

    pub(crate) fn column(&self) -> &'static str {
        match self {
            Self::SizeDesc | Self::SizeAsc => "size",
            Self::NameAsc | Self::NameDesc => "name",
            Self::PercentDesc | Self::PercentAsc => "percent",
            Self::CategoryAsc | Self::CategoryDesc => "category",
            Self::ModifiedDesc | Self::ModifiedAsc => "modified",
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::SizeDesc => "大小 ↓",
            Self::SizeAsc => "大小 ↑",
            Self::NameAsc => "名称 ↑",
            Self::NameDesc => "名称 ↓",
            Self::PercentDesc => "占比 ↓",
            Self::PercentAsc => "占比 ↑",
            Self::CategoryAsc => "类别 ↑",
            Self::CategoryDesc => "类别 ↓",
            Self::ModifiedDesc => "修改时间 ↓",
            Self::ModifiedAsc => "修改时间 ↑",
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Dashboard,
    Tree,
    Treemap,
    Sunburst,
    TopFiles,
    Classify,
    Duplicates,
    Waste,
    Trend,
    Collect,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Dashboard => "概览",
            Tab::Tree => "树形",
            Tab::Treemap => "Treemap",
            Tab::Sunburst => "环形图",
            Tab::TopFiles => "大文件",
            Tab::Classify => "分类",
            Tab::Duplicates => "重复",
            Tab::Waste => "浪费",
            Tab::Trend => "趋势",
            Tab::Collect => "收集",
        }
    }
}

impl TreeSizeApp {
    pub fn new(cc: &eframe::CreationContext<'_>, options: GuiOptions) -> Self {
        setup_chinese_fonts(&cc.egui_ctx);

        let dark_mode = false;
        let theme = apply_theme(&cc.egui_ctx, dark_mode);

        let path_input = options.initial_path.display().to_string();
        Self {
            path_input,
            options: options.scan_options,
            state: ScanState::shared(),
            selected_tab: Tab::Dashboard,
            top_n: 50,
            show_hidden: true,
            search_text: String::new(),
            search_case_sensitive: false,
            search_match_full: false,
            font_scale: 1.0,
            dark_mode,
            show_settings: false,
            theme_dirty: false,
            theme,
            expanded_paths: std::collections::HashSet::new(),
            tree_sort_by: render::tree::TreeSortBy::SizeDesc,
            path_history: std::collections::VecDeque::new(),
            cache_generation: 0,
            cache_top_n: 0,
            cached_top_files: None,
            cached_top_dirs: None,
            cached_classify: None,
            cached_duplicates: None,
            cached_waste: None,
            cached_all_files: None,
            top_files_sort_by: TopFilesSortBy::SizeDesc,
            top_files_search: String::new(),
            dashboard_hover: None,
            delete_confirm_path: None,
            delete_error: None,
            prev_running: false,
            history_storage: None,
            cached_trend: None,
            cached_diff: None,
            diff_old_selected: None,
            diff_new_selected: None,
            drag_collector: DragCollector::new(),
            cache_computing: false,
            cache_generation_computing: 0,
            compute_cache: Arc::new(Mutex::new(ComputeCache::default())),
            last_repaint: Instant::now(),
        }
    }

    fn start_scan(&mut self) {
        let path_str = self.path_input.trim().to_string();
        let path = PathBuf::from(&path_str);
        tracing::info!(target: "treesize::gui", "start_scan 被调用，路径：{}", path.display());

        if !path.exists() {
            self.state.lock().unwrap().error = Some(format!("路径不存在：{}", path.display()));
            tracing::warn!(target: "treesize::gui", "路径不存在：{}", path.display());
            return;
        }

        if !self.path_history.contains(&path_str) {
            self.path_history.push_back(path_str.clone());
            if self.path_history.len() > 20 {
                self.path_history.pop_front();
            }
        }

        let state = self.state.clone();
        let mut options = self.options.clone();
        options.include_hidden = self.show_hidden;

        let generation = {
            let mut s = state.lock().unwrap();
            s.cancel.cancel();
            s.cancel = CancelToken::new();
            s.running = true;
            s.error = None;
            s.node = None;
            s.stats = None;
            s.progress = ScanProgress::default();
            s.started_at = Some(SystemTime::now());
            s.generation += 1;
            s.generation
        };

        let cancel_token = {
            let s = state.lock().unwrap();
            s.cancel.clone()
        };

        let path_clone = path.clone();
        tracing::info!(target: "treesize::gui", "即将启动扫描线程，代次：{}", generation);

        std::thread::spawn(move || {
            tracing::info!(target: "treesize::gui", "扫描线程已启动，路径：{}", path_clone.display());

            let mut current_options = options.clone();
            let mut fallback_to_fs = false;

            let result = loop {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tracing::info!(target: "treesize::gui", "创建扫描引擎...");
                    let engine = create_scan_engine(&current_options);
                    tracing::info!(target: "treesize::gui", "创建扫描服务...");
                    let service = ScanService::new(engine);

                    let state_clone = state.clone();
                    let progress_cb = move |p: &ScanProgress| {
                        if let Ok(mut s) = state_clone.lock() {
                            if s.generation != generation {
                                return;
                            }
                            s.progress = p.clone();
                        }
                    };

                    tracing::info!(target: "treesize::gui", "开始执行扫描...");
                    service.scan(path.clone(), &current_options, Some(&progress_cb), Some(&cancel_token))
                }));

                match &result {
                    Ok(Err(e)) => {
                        let err_str = e.to_string();
                        if !fallback_to_fs && current_options.engine == crate::domain::scan_engine::ScanEngineType::Mft 
                            && (err_str.contains("MFT") || err_str.contains("NTFS") || err_str.contains("LCN")) 
                        {
                            tracing::warn!(target: "treesize::gui", "MFT 扫描失败，自动降级到 Fs 引擎重试");
                            fallback_to_fs = true;
                            current_options.engine = crate::domain::scan_engine::ScanEngineType::Fs;
                            continue;
                        }
                    }
                    _ => {}
                }

                break result;
            };

            tracing::info!(target: "treesize::gui", "扫描线程执行完毕");

            let (result_node, result_stats, result_error) = match result {
                Ok(Ok((node, stats))) => {
                    tracing::info!(target: "treesize::gui", "GUI 扫描完成");
                    (Some(node), Some(stats), None)
                }
                Ok(Err(e)) => {
                    tracing::error!(target: "treesize::gui", "GUI 扫描失败：{e}");
                    (None, None, Some(e.to_string()))
                }
                Err(panic_payload) => {
                    let msg = panic_payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| panic_payload.downcast_ref::<String>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "扫描线程内部错误".to_string());
                    tracing::error!(target: "treesize::gui", "扫描线程 panic：{msg}");
                    (None, None, Some(msg))
                }
            };

            if let Ok(mut s) = state.lock() {
                if s.generation != generation {
                    tracing::info!(target: "treesize::gui", "跳过过期扫描结果（代次不匹配）");
                    return;
                }
                s.running = false;
                s.error = result_error;
                if let (Some(n), Some(st)) = (result_node, result_stats) {
                    s.node = Some(Arc::new(n));
                    s.stats = Some(st);
                }
            }
        });
    }

    fn cancel_scan(&mut self) {
        let s = self.state.lock().unwrap();
        s.cancel.cancel();
    }

    fn tab_badge(&self, tab: Tab) -> Option<u64> {
        let s = self.state.lock().unwrap();
        if s.node.is_none() {
            return None;
        }
        match tab {
            Tab::Tree | Tab::Treemap | Tab::Sunburst | Tab::Dashboard | Tab::Trend => None,
            Tab::TopFiles => Some(self.top_n as u64),
            Tab::Classify => Some(
                self.cached_classify
                    .as_ref()
                    .map(|r| r.by_category.len() as u64)
                    .unwrap_or(0),
            ),
            Tab::Duplicates => self
                .cached_duplicates
                .as_ref()
                .map(|r| r.groups.len() as u64),
            Tab::Waste => self
                .cached_waste
                .as_ref()
                .map(|r| r.items.len() as u64),
            Tab::Collect => Some(self.drag_collector.items.len() as u64),
        }
    }
}

impl eframe::App for TreeSizeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_font_scale(ctx);
        self.apply_theme_if_dirty(ctx);
        self.control_frame_rate(ctx);

        let snapshot = self.get_snapshot();
        let scan_just_finished = !snapshot.running && self.prev_running;
        self.prev_running = snapshot.running;

        if scan_just_finished {
            self.save_history_async();
        }

        self.render_toolbar(ctx);
        self.render_settings_window(ctx);
        self.render_delete_dialog(ctx);
        self.render_search_toolbar(ctx);
        self.render_status_bar(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_tab_bar(ui);
            ui.separator();

            let snapshot = self.get_snapshot();
            self.invalidate_cache_if_needed(&snapshot);
            self.refresh_cache(&snapshot);

            match self.selected_tab {
                Tab::Dashboard => self.render_dashboard(ui, &snapshot),
                Tab::Tree => self.render_tree(ui, &snapshot),
                Tab::Treemap => self.render_treemap(ui, &snapshot),
                Tab::Sunburst => self.render_sunburst(ui, &snapshot),
                Tab::TopFiles => self.render_topn(ui, &snapshot),
                Tab::Classify => self.render_classify(ui, &snapshot),
                Tab::Duplicates => self.render_duplicates(ui, &snapshot),
                Tab::Waste => self.render_waste(ui, &snapshot),
                Tab::Trend => self.render_trend(ui, &snapshot),
                Tab::Collect => self.render_collect(ui),
            }
        });
    }
}

impl TreeSizeApp {
    fn apply_font_scale(&mut self, ctx: &egui::Context) {
        let base_size = 18.0;
        ctx.style_mut(|style| {
            for (_, font_id) in &mut style.text_styles {
                font_id.size = base_size * self.font_scale;
            }
            style.spacing.item_spacing = Vec2::new(6.0, 4.0);
            style.spacing.button_padding = Vec2::new(8.0, 3.0);
        });
    }

    fn apply_theme_if_dirty(&mut self, ctx: &egui::Context) {
        if self.theme_dirty {
            self.theme = apply_theme(ctx, self.dark_mode);
            self.theme_dirty = false;
        }
    }

    fn control_frame_rate(&mut self, ctx: &egui::Context) {
        let running = self.state.try_lock().map(|s| s.running).unwrap_or(false);
        let now = Instant::now();
        let min_frame_interval = Duration::from_secs_f32(1.0 / 30.0);
        if running {
            ctx.request_repaint();
        } else if self.prev_running {
            ctx.request_repaint();
            self.prev_running = false;
        } else if now.duration_since(self.last_repaint) >= min_frame_interval {
            ctx.request_repaint_after(min_frame_interval);
        }
        self.last_repaint = now;
    }

    fn get_snapshot(&self) -> Snapshot {
        let s = self.state.lock().unwrap();
        Snapshot {
            node: s.node.clone(),
            stats: s.stats.clone(),
            progress: s.progress.clone(),
            running: s.running,
            error: s.error.clone(),
            generation: s.generation,
        }
    }

    fn save_history_async(&mut self) {
        let (needs_save, path_str, node_arc) = {
            let s = self.state.lock().unwrap();
            (s.node.is_some() && s.stats.is_some(), self.path_input.clone(), s.node.clone())
        };
        if needs_save {
            let elapsed_ms = {
                let s = self.state.lock().unwrap();
                s.stats.as_ref().map(|st| st.elapsed_ms).unwrap_or(0)
            };
            if let Some(node_arc) = node_arc {
                let path = std::path::PathBuf::from(&path_str);
                std::thread::spawn(move || {
                    let (snapshot, categories, dirs) = TrendService::build_snapshot(&path, &node_arc, elapsed_ms);
                    if let Ok(storage) = HistoryStorage::open_default() {
                        if let Err(e) = storage.save_snapshot(&snapshot, &categories, &dirs) {
                            tracing::warn!("保存扫描历史失败：{e}");
                        } else {
                            tracing::info!("扫描历史已保存");
                        }
                    }
                });
            }
        }
    }

    fn render_toolbar(&mut self, ctx: &egui::Context) {
        let running = self.state.try_lock().map(|s| s.running).unwrap_or(false);

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            egui::Frame::none()
                .fill(self.theme.bg_surface)
                .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("扫描路径：")
                                .color(self.theme.text_secondary),
                        );

                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.path_input)
                                .desired_width(250.0)
                                .hint_text("输入目录路径…"),
                        );
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.start_scan();
                        }

                        if !self.path_history.is_empty() {
                            ui.menu_button("📜 历史", |ui| {
                                for p in self.path_history.iter().rev() {
                                    if ui.button(p).clicked() {
                                        self.path_input = p.clone();
                                        ui.close_menu();
                                    }
                                }
                            });
                        }

                        if ui.button("📁 选择").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                self.path_input = path.display().to_string();
                            }
                        }

                        let scan_label = if running { "⏳ 扫描中" } else { "▶ 扫描" };
                        let scan_btn = egui::Button::new(
                            egui::RichText::new(scan_label)
                                .color(Color32::WHITE)
                                .strong(),
                        )
                        .fill(if running { self.theme.text_dim } else { self.theme.accent })
                        .min_size(Vec2::new(60.0, 24.0));
                        if ui.add_enabled(!running, scan_btn).clicked() {
                            self.start_scan();
                        }

                        if running {
                            let cancel_btn = egui::Button::new(
                                egui::RichText::new("■ 取消")
                                    .color(self.theme.danger),
                            )
                            .fill(self.theme.bg_card);
                            if ui.add(cancel_btn).clicked() {
                                self.cancel_scan();
                            }
                        }

                        ui.separator();
                        ui.checkbox(&mut self.show_hidden, "显示隐藏");
                        ui.separator();
                        ui.label(
                            egui::RichText::new("TopN:")
                                .color(self.theme.text_secondary),
                        );
                        ui.add(egui::DragValue::new(&mut self.top_n).clamp_range(1..=500));
                        ui.separator();

                        if ui.button("⚙ 设置").clicked() {
                            self.show_settings = !self.show_settings;
                        }
                    });
                });
        });
    }

    fn render_settings_window(&mut self, ctx: &egui::Context) {
        let mut settings_open = self.show_settings;
        egui::Window::new("设置")
            .open(&mut settings_open)
            .default_size([320.0, 220.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("字体缩放")
                            .color(self.theme.text_primary),
                    );
                    if ui.small_button("重置").clicked() {
                        self.font_scale = 1.0;
                    }
                });
                ui.add(
                    egui::Slider::new(&mut self.font_scale, 0.8..=2.0)
                        .text("倍")
                        .step_by(0.05),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("主题：")
                            .color(self.theme.text_primary),
                    );
                    let prev = self.dark_mode;
                    let dark_btn = egui::Button::new(
                        egui::RichText::new("深色")
                            .color(if self.dark_mode { self.theme.tab_text_active } else { self.theme.text_secondary }),
                    )
                    .fill(if self.dark_mode { self.theme.tab_active_bg } else { self.theme.tab_inactive_bg })
                    .rounding(egui::Rounding::same(4.0));
                    if ui.add(dark_btn).clicked() {
                        self.dark_mode = true;
                    }
                    let light_btn = egui::Button::new(
                        egui::RichText::new("亮色")
                            .color(if !self.dark_mode { self.theme.tab_text_active } else { self.theme.text_secondary }),
                    )
                    .fill(if !self.dark_mode { self.theme.tab_active_bg } else { self.theme.tab_inactive_bg })
                    .rounding(egui::Rounding::same(4.0));
                    if ui.add(light_btn).clicked() {
                        self.dark_mode = false;
                    }
                    if self.dark_mode != prev {
                        self.theme_dirty = true;
                    }
                });
                ui.separator();
                #[cfg(embedded_font)]
                ui.label(
                    egui::RichText::new("字体：Noto Sans SC (内嵌)")
                        .color(self.theme.text_secondary),
                );
                #[cfg(not(embedded_font))]
                ui.label(
                    egui::RichText::new("字体：系统字体")
                        .color(self.theme.text_secondary),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new("扫描引擎：")
                        .color(self.theme.text_primary),
                );
                let engine = &mut self.options.engine;
                ui.horizontal(|ui| {
                    if ui
                        .radio_value(engine, crate::domain::ScanEngineType::Fs, "文件系统")
                        .clicked()
                    {
                        tracing::info!("扫描引擎切换为：文件系统");
                    }
                    #[cfg(target_os = "windows")]
                    if ui
                        .radio_value(engine, crate::domain::ScanEngineType::Mft, "MFT 直接读取")
                        .clicked()
                    {
                        tracing::info!("扫描引擎切换为：MFT 直接读取");
                    }
                    #[cfg(target_os = "windows")]
                    if ui
                        .radio_value(engine, crate::domain::ScanEngineType::Usn, "USN Journal")
                        .clicked()
                    {
                        tracing::info!("扫描引擎切换为：USN Journal");
                    }
                });
            });
        if !settings_open {
            self.show_settings = false;
        }
    }

    fn render_delete_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = &self.delete_confirm_path {
            let path_clone = path.clone();
            let mut show_dialog = true;
            egui::Window::new("确认删除")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("确定要删除以下内容吗？").strong());
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(path_clone.display().to_string())
                                .color(self.theme.text_secondary)
                                .small(),
                        );
                        ui.add_space(8.0);
                        if let Some(err) = &self.delete_error {
                            ui.colored_label(self.theme.danger, format!("错误：{}", err));
                            ui.add_space(8.0);
                        }
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("取消").clicked() {
                                    show_dialog = false;
                                }
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("删除").color(Color32::WHITE),
                                        )
                                        .fill(self.theme.danger),
                                    )
                                    .clicked()
                                {
                                    match delete_path(&path_clone) {
                                        Ok(_) => {
                                            tracing::info!(target: "treesize::gui", "已删除：{}", path_clone.display());
                                            show_dialog = false;
                                            self.delete_confirm_path = None;
                                            self.delete_error = None;

                                            {
                                                let mut s = self.state.lock().unwrap();
                                                if let Some(node) = s.node.as_mut() {
                                                    Arc::make_mut(node).remove_path(&path_clone);
                                                    tracing::debug!(target: "treesize::gui", "增量刷新完成");
                                                }
                                            }

                                            self.cache_generation = 0;
                                            self.cached_top_files = None;
                                            self.cached_top_dirs = None;
                                            self.cached_classify = None;
                                            self.cached_duplicates = None;
                                            self.cached_all_files = None;
                                        }
                                        Err(e) => {
                                            tracing::error!(target: "treesize::gui", "删除失败：{}", e);
                                            self.delete_error = Some(e);
                                        }
                                    }
                                }
                            });
                        });
                        ui.add_space(10.0);
                    });
                });
            if !show_dialog {
                self.delete_confirm_path = None;
                self.delete_error = None;
            }
        }
    }

    fn render_search_toolbar(&mut self, ctx: &egui::Context) {
        if self.selected_tab == Tab::Tree {
            egui::TopBottomPanel::top("search_toolbar").show(ctx, |ui| {
                egui::Frame::none()
                    .fill(self.theme.bg_surface)
                    .inner_margin(egui::Margin::symmetric(6.0, 3.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("🔍 查找：")
                                    .color(self.theme.text_secondary),
                            );
                            ui.text_edit_singleline(&mut self.search_text);
                            ui.checkbox(&mut self.search_case_sensitive, "区分大小写");
                            ui.checkbox(&mut self.search_match_full, "完全匹配");
                            if ui.button("清除").clicked() {
                                self.search_text.clear();
                            }
                        });
                    });
            });
        }
    }

    fn render_status_bar(&self, ctx: &egui::Context) {
        let snapshot = self.get_snapshot();
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            egui::Frame::none()
                .fill(self.theme.bg_surface)
                .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (dot_color, status_text) = if snapshot.running {
                            (self.theme.warn, "扫描中")
                        } else if snapshot.error.is_some() {
                            (self.theme.danger, "错误")
                        } else if snapshot.stats.is_some() {
                            (self.theme.success, "完成")
                        } else {
                            (self.theme.text_dim, "就绪")
                        };
                        painter_dot(ui, dot_color);
                        ui.label(egui::RichText::new(status_text).color(dot_color).strong());
                        ui.separator();

                        if snapshot.running {
                            ui.label(format!(
                                "{} 文件 / {} 目录",
                                snapshot.progress.files_scanned, snapshot.progress.dirs_scanned
                            ));
                            ui.separator();
                            ui.label(format!(
                                "已扫描 {}",
                                ByteSize(snapshot.progress.bytes_scanned)
                            ));
                            ui.separator();
                            let path = &snapshot.progress.current_path;
                            let max_chars = 60;
                            let display = if path.chars().count() > max_chars {
                                let suffix: String = path.chars().rev().take(max_chars).collect::<Vec<_>>().into_iter().rev().collect();
                                format!("…{}", suffix)
                            } else {
                                path.clone()
                            };
                            ui.label(egui::RichText::new(display).color(self.theme.text_secondary));
                        } else if let Some(e) = &snapshot.error {
                            ui.colored_label(self.theme.danger, format!("错误：{e}"));
                        } else if let Some(stats) = &snapshot.stats {
                            ui.label(format!(
                                "{} 文件 / {} 目录 / {}",
                                stats.total_files, stats.total_dirs, stats.total_size
                            ));
                            ui.separator();
                            ui.label(format!("耗时 {} ms", stats.elapsed_ms));
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}px",
                                    (18.0 * self.font_scale) as u32
                                ))
                                .color(self.theme.text_dim),
                            );
                            #[cfg(embedded_font)]
                            ui.label(
                                egui::RichText::new("Noto Sans SC")
                                    .color(self.theme.text_dim),
                            );
                        });
                    });
                });
        });
    }

    fn render_tab_bar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(self.theme.bg_primary)
            .inner_margin(egui::Margin::symmetric(4.0, 4.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    use Tab::*;
                    let tabs = [Dashboard, Tree, Treemap, Sunburst, TopFiles, Classify, Duplicates, Waste, Trend, Collect];
                    for tab in tabs {
                        let selected = self.selected_tab == tab;
                        let badge = self.tab_badge(tab);
                        render_tab_button(ui, tab.label(), selected, badge, &self.theme, |ui| {
                            self.selected_tab = tab;
                            let _ = ui;
                        });
                    }
                });
            });
    }

    fn invalidate_cache_if_needed(&mut self, snapshot: &Snapshot) {
        let cache_invalid = snapshot.generation != self.cache_generation
            || self.cache_top_n != self.top_n;
        if cache_invalid {
            self.cache_generation = snapshot.generation;
            self.cache_top_n = self.top_n;
            self.cached_top_files = None;
            self.cached_top_dirs = None;
            self.cached_classify = None;
            self.cached_duplicates = None;
            self.cached_waste = None;
            self.cached_all_files = None;
            *self.compute_cache.lock().unwrap() = ComputeCache::default();
        }
    }

    fn refresh_cache(&mut self, snapshot: &Snapshot) {
        if snapshot.node.is_none() {
            return;
        }

        let needs_compute = (self.selected_tab == Tab::Dashboard &&
            (self.cached_top_files.is_none() || self.cached_top_dirs.is_none() ||
             self.cached_classify.is_none() || self.cached_duplicates.is_none())) ||
            (self.selected_tab == Tab::TopFiles && self.cached_all_files.is_none()) ||
            (self.selected_tab == Tab::Classify && self.cached_classify.is_none()) ||
            (self.selected_tab == Tab::Duplicates && self.cached_duplicates.is_none()) ||
            (self.selected_tab == Tab::Waste && self.cached_waste.is_none());

        if needs_compute && !self.cache_computing {
            self.cache_computing = true;
            self.cache_generation_computing = snapshot.generation;

            let state_clone = self.state.clone();
            let compute_cache = self.compute_cache.clone();
            let selected_tab = self.selected_tab;
            let top_n = self.top_n;
            let generation = snapshot.generation;

            std::thread::spawn(move || {
                let s = state_clone.lock().unwrap();
                if let Some(node) = s.node.as_ref() {
                    if s.generation != generation {
                        return;
                    }

                    let node_clone = node.clone();
                    drop(s);

                    let mut cache = compute_cache.lock().unwrap();
                    cache.generation = generation;

                    match selected_tab {
                        Tab::Dashboard => {
                            cache.top_files = Some(ReportService::top_n_files_report(&node_clone, top_n));
                            cache.top_dirs = Some(ReportService::top_n_dirs_report(&node_clone, top_n));
                            cache.classify = Some(ClassifyService::analyze(&node_clone));
                            cache.duplicates = Some(DuplicateService::scan(&node_clone, 1024));
                        }
                        Tab::TopFiles => {
                            cache.all_files = Some(ReportService::top_n_files_report(&node_clone, top_n.max(200)));
                        }
                        Tab::Classify => {
                            cache.classify = Some(ClassifyService::analyze(&node_clone));
                        }
                        Tab::Duplicates => {
                            cache.duplicates = Some(DuplicateService::scan(&node_clone, 1024));
                        }
                        Tab::Waste => {
                            cache.waste = Some(WasteService::scan(&node_clone));
                        }
                        _ => {}
                    }
                }
            });
        }

        if self.cache_computing {
            let mut cache = self.compute_cache.lock().unwrap();
            if cache.generation == snapshot.generation {
                if let Some(tf) = cache.top_files.take() { self.cached_top_files = Some(tf); }
                if let Some(td) = cache.top_dirs.take() { self.cached_top_dirs = Some(td); }
                if let Some(cl) = cache.classify.take() { self.cached_classify = Some(cl); }
                if let Some(dd) = cache.duplicates.take() { self.cached_duplicates = Some(dd); }
                if let Some(wa) = cache.waste.take() { self.cached_waste = Some(wa); }
                if let Some(af) = cache.all_files.take() { self.cached_all_files = Some(af); }
                self.cache_computing = false;
            }
        }
    }

    fn render_dashboard(&mut self, ui: &mut egui::Ui, state: &Snapshot) {
        render::dashboard::render_dashboard(
            ui,
            state,
            &self.theme,
            &mut self.dashboard_hover,
            &self.cached_top_files,
            &self.cached_top_dirs,
            &self.cached_duplicates,
            &self.cached_classify,
        );
    }

    fn render_tree(&mut self, ui: &mut egui::Ui, state: &Snapshot) {
        if state.node.is_none() {
            render_empty_state(ui, "尚未扫描", "在上方输入路径，或点击「📁 选择」目录后开始");
            return;
        }
        let node = state.node.as_ref().unwrap();

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("排序：")
                    .color(self.theme.text_secondary)
                    .small(),
            );
            let sorts = [
                (render::tree::TreeSortBy::SizeDesc, "大小↓"),
                (render::tree::TreeSortBy::SizeAsc, "大小↑"),
                (render::tree::TreeSortBy::NameAsc, "名称↓"),
                (render::tree::TreeSortBy::NameDesc, "名称↑"),
            ];
            for (by, label) in &sorts {
                let selected = self.tree_sort_by == *by;
                let bg = if selected {
                    self.theme.tab_active_bg
                } else {
                    self.theme.tab_inactive_bg
                };
                let btn = egui::Button::new(
                    egui::RichText::new(*label)
                        .color(self.theme.text_secondary)
                        .small(),
                )
                .fill(bg)
                .rounding(egui::Rounding::same(3.0))
                .min_size(Vec2::new(0.0, 20.0));
                if ui.add(btn).clicked() {
                    self.tree_sort_by = *by;
                }
            }
        });
        ui.add_space(4.0);

        let search_lower = self.search_text.to_lowercase();
        let mut delete_request: Option<std::path::PathBuf> = None;

        egui::ScrollArea::both().show(ui, |ui| {
            render::tree::render_tree_node(
                ui,
                node,
                node.size.0,
                0,
                self.show_hidden,
                &self.search_text,
                &search_lower,
                self.search_case_sensitive,
                self.search_match_full,
                &self.theme,
                &mut self.expanded_paths,
                self.tree_sort_by,
                &mut delete_request,
            );
        });

        if let Some(path) = delete_request {
            self.delete_confirm_path = Some(path);
        }
    }

    fn render_treemap(&self, ui: &mut egui::Ui, state: &Snapshot) {
        if state.node.is_none() {
            render_empty_state(ui, "尚未扫描", "在上方输入路径，或点击「📁 选择」目录后开始");
            return;
        }
        let node = state.node.as_ref().unwrap();
        TreemapView::show(ui, node, &self.theme);
    }

    fn render_sunburst(&self, ui: &mut egui::Ui, state: &Snapshot) {
        if state.node.is_none() {
            render_empty_state(ui, "尚未扫描", "在上方输入路径，或点击「📁 选择」目录后开始");
            return;
        }
        let node = state.node.as_ref().unwrap();
        SunburstView::show(ui, node, &self.theme);
    }

    fn render_trend(&mut self, ui: &mut egui::Ui, state: &Snapshot) {
        let scan_path = state
            .node
            .as_ref()
            .map(|n| n.path.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path_input.clone());

        if self.history_storage.is_none() {
            self.history_storage = HistoryStorage::open_default().ok();
        }
        if self.cached_trend.is_none() {
            if let Some(ref storage) = self.history_storage {
                self.cached_trend = storage.get_trend(&scan_path).ok();
            }
        }

        let trend = match self.cached_trend.clone() {
            Some(t) => t,
            None => {
                render_empty_state(
                    ui,
                    "暂无扫描历史",
                    "执行扫描后自动保存历史记录，或使用 CLI 的 --history-save 参数",
                );
                return;
            }
        };

        if trend.size_trend.is_empty() {
            render_empty_state(
                ui,
                "暂无扫描历史",
                "还没有该路径的历史记录，完成一次扫描后自动生成",
            );
            return;
        }

        ui.horizontal(|ui| {
            let first = &trend.size_trend[0];
            let last = trend.size_trend.last().unwrap();
            let growth = last.total_size.saturating_sub(first.total_size);
            let pct = if first.total_size > 0 {
                (growth as f64 / first.total_size as f64) * 100.0
            } else {
                0.0
            };
            stat_card(ui, "扫描次数", &trend.snapshots.len().to_string(), self.theme.accent, &self.theme);
            stat_card(ui, "首次", &first.date.format("%Y-%m-%d").to_string(), self.theme.text_secondary, &self.theme);
            stat_card(ui, "最近", &last.date.format("%Y-%m-%d").to_string(), self.theme.text_secondary, &self.theme);
            stat_card(ui, "增长", &format!("{} ({:.1}%)", ByteSize(growth), pct), self.theme.warn, &self.theme);
        });

        ui.add_space(8.0);
        render::trend::trend_line_chart(ui, &trend, &self.theme);
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);
        self.render_snapshot_diff_panel(ui, &trend);
    }

    fn render_snapshot_diff_panel(&mut self, ui: &mut egui::Ui, trend: &TrendReport) {
        ui.heading(
            egui::RichText::new("快照对比")
                .color(self.theme.text_primary)
                .strong(),
        );
        ui.add_space(4.0);

        if trend.snapshots.len() < 2 {
            ui.label(
                egui::RichText::new("需要至少 2 次扫描记录才能进行对比")
                    .color(self.theme.text_secondary),
            );
            return;
        }

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("旧快照：").color(self.theme.text_primary));
            egui::ComboBox::from_id_source("diff_old_combo")
                .selected_text(
                    self.diff_old_selected
                        .and_then(|id| trend.snapshots.iter().find(|s| s.id == Some(id)))
                        .map(|s| s.scanned_at.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "选择...".to_string()),
                )
                .show_ui(ui, |ui: &mut egui::Ui| {
                    for snap in &trend.snapshots {
                        if let Some(id) = snap.id {
                            let label = snap.scanned_at.format("%Y-%m-%d %H:%M").to_string();
                            ui.selectable_value(&mut self.diff_old_selected, Some(id), label);
                        }
                    }
                });

            ui.add_space(16.0);

            ui.label(egui::RichText::new("新快照：").color(self.theme.text_primary));
            egui::ComboBox::from_id_source("diff_new_combo")
                .selected_text(
                    self.diff_new_selected
                        .and_then(|id| trend.snapshots.iter().find(|s| s.id == Some(id)))
                        .map(|s| s.scanned_at.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "选择...".to_string()),
                )
                .show_ui(ui, |ui: &mut egui::Ui| {
                    for snap in &trend.snapshots {
                        if let Some(id) = snap.id {
                            let label = snap.scanned_at.format("%Y-%m-%d %H:%M").to_string();
                            ui.selectable_value(&mut self.diff_new_selected, Some(id), label);
                        }
                    }
                });

            ui.add_space(16.0);

            if ui.button("对比").clicked() {
                self.compute_diff();
            }
        });

        ui.add_space(8.0);

        if let Some(ref diff) = self.cached_diff {
            render::trend::render_diff_result(ui, diff, &self.theme);
        }
    }

    fn compute_diff(&mut self) {
        let old_id = match self.diff_old_selected {
            Some(id) => id,
            None => return,
        };
        let new_id = match self.diff_new_selected {
            Some(id) => id,
            None => return,
        };

        let storage = match self.history_storage.as_ref() {
            Some(s) => s,
            None => return,
        };

        match storage.get_diff_data(old_id, new_id) {
            Ok((old, new, old_cats, new_cats, old_dirs, new_dirs)) => {
                let diff = SnapshotDiffService::compare(&old, &new, &old_cats, &new_cats, &old_dirs, &new_dirs);
                self.cached_diff = Some(diff);
            }
            Err(e) => {
                tracing::error!("快照对比失败：{e}");
            }
        }
    }

    fn render_topn(&mut self, ui: &mut egui::Ui, state: &Snapshot) {
        if let Some(action) = render::topn::render_topn(
            ui,
            state,
            &self.theme,
            &mut self.top_files_search,
            &mut self.top_n,
            &mut self.top_files_sort_by,
            &self.cached_all_files,
        ) {
            if self.top_files_sort_by.column() == action.column() {
                self.top_files_sort_by = self.top_files_sort_by.toggle();
            } else {
                self.top_files_sort_by = action;
            }
        }
    }

    fn render_classify(&self, ui: &mut egui::Ui, state: &Snapshot) {
        render::classify::render_classify(ui, state, &self.theme, &self.cached_classify);
    }

    fn render_duplicates(&self, ui: &mut egui::Ui, state: &Snapshot) {
        render::duplicates::render_duplicates(ui, state, &self.theme, &self.cached_duplicates);
    }

    fn render_waste(&self, ui: &mut egui::Ui, state: &Snapshot) {
        render::waste::render_waste(ui, state, &self.theme, &self.cached_waste);
    }

    fn render_collect(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("📂 添加目录").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.collect_path(path);
                    }
                }
                if ui.button("📄 添加文件").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        self.collect_path(path);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.drag_collector.items.is_empty() {
                        let marked = self.drag_collector.marked_count();
                        if marked > 0 {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(format!("🗑 删除已标记（{}）", marked))
                                            .color(Color32::WHITE),
                                    )
                                    .fill(self.theme.danger),
                                )
                                .clicked()
                            {
                                self.delete_marked_collected();
                            }
                        }
                        if ui.button("清空全部").clicked() {
                            self.drag_collector.clear();
                        }
                    }
                });
            });

            ui.add_space(8.0);

            if self.drag_collector.items.is_empty() {
                ui.add_space(20.0);
                render_empty_state(
                    ui,
                    "暂无收集文件",
                    "点击上方「添加目录」或「添加文件」按钮选择要管理的项目",
                );
                return;
            }

            ui.horizontal(|ui| {
                stat_card(
                    ui,
                    "已收集",
                    &self.drag_collector.items.len().to_string(),
                    self.theme.accent,
                    &self.theme,
                );
                stat_card(
                    ui,
                    "总大小",
                    &ByteSize(self.drag_collector.total_size).to_string(),
                    self.theme.text_primary,
                    &self.theme,
                );
                let marked = self.drag_collector.marked_count();
                stat_card(
                    ui,
                    "待删除",
                    &marked.to_string(),
                    if marked > 0 { self.theme.danger } else { self.theme.text_secondary },
                    &self.theme,
                );
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            let mut remove_idx = None;
            let mut toggle_mark_idx = None;
            egui::Grid::new("collect_grid")
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("名称").strong().color(self.theme.text_primary));
                    ui.label(egui::RichText::new("大小").strong().color(self.theme.text_primary));
                    ui.label(egui::RichText::new("类型").strong().color(self.theme.text_primary));
                    ui.label(egui::RichText::new("路径").strong().color(self.theme.text_primary));
                    ui.label(egui::RichText::new("操作").strong().color(self.theme.text_primary));
                    ui.end_row();

                    let items_snapshot: Vec<_> = self.drag_collector.items.iter()
                        .map(|item| (item.name.clone(), item.size, item.is_dir, item.path.clone(), item.marked_for_deletion))
                        .collect();

                    for (i, (name, size, is_dir, path, marked)) in items_snapshot.iter().enumerate() {
                        let icon = if *is_dir { "📁" } else { "📄" };
                        ui.label(format!("{} {}", icon, name));

                        ui.label(ByteSize(*size).to_string());

                        let type_text = if *is_dir { "目录" } else { "文件" };
                        ui.label(type_text);

                        ui.label(
                            egui::RichText::new(path.display().to_string())
                                .color(self.theme.text_secondary)
                                .small(),
                        );

                        ui.horizontal(|ui| {
                            let mark_label = if *marked { "✅ 已标记" } else { "☐ 标记删除" };
                            if ui.button(mark_label).clicked() {
                                toggle_mark_idx = Some(i);
                            }
                            if ui.button("✕ 移除").clicked() {
                                remove_idx = Some(i);
                            }
                        });

                        ui.end_row();
                    }
                });

            if let Some(idx) = remove_idx {
                self.drag_collector.remove(idx);
            }
            if let Some(idx) = toggle_mark_idx {
                if self.drag_collector.items.get(idx).map(|i| i.marked_for_deletion).unwrap_or(false) {
                    self.drag_collector.unmark_for_deletion(idx);
                } else {
                    self.drag_collector.mark_for_deletion(idx);
                }
            }
        });
    }

    fn collect_path(&mut self, path: std::path::PathBuf) {
        let is_dir = path.is_dir();
        let size = if is_dir {
            dir_size_recursive(&path)
        } else {
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        };
        let mut item = crate::domain::CollectedItem::new(path).with_size(size);
        if is_dir {
            item = item.as_dir();
        }
        self.drag_collector.add(item);
    }

    fn delete_marked_collected(&mut self) {
        let marked: Vec<_> = self.drag_collector.items.iter()
            .enumerate()
            .filter(|(_, item)| item.marked_for_deletion)
            .map(|(_, item)| item.path.clone())
            .collect();

        if marked.is_empty() {
            return;
        }

        if let Some(path) = marked.into_iter().next() {
            self.delete_confirm_path = Some(path);
            self.delete_error = None;
        }
    }
}

fn create_scan_engine(options: &ScanOptions) -> Arc<dyn crate::domain::scan_engine::ScanEngine> {
    #[cfg(target_os = "windows")]
    {
        match options.engine {
            crate::domain::scan_engine::ScanEngineType::Mft => {
                tracing::info!("GUI 使用 MFT 直接读取引擎（极速）");
                Arc::new(MftScanEngine::new())
            }
            crate::domain::scan_engine::ScanEngineType::Usn => {
                tracing::info!("GUI 使用 USN Journal 增量引擎");
                Arc::new(UsnScanEngine::new())
            }
            crate::domain::scan_engine::ScanEngineType::Fs => {
                tracing::info!("GUI 使用 FsScanEngine（文件系统遍历）");
                Arc::new(FsScanEngine::new())
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        tracing::info!("GUI 使用 FsScanEngine（文件系统遍历）");
        Arc::new(FsScanEngine::new())
    }
}

fn delete_path(path: &std::path::Path) -> Result<(), String> {
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| format!("删除目录失败：{}", e))
    } else if path.is_file() {
        std::fs::remove_file(path).map_err(|e| format!("删除文件失败：{}", e))
    } else {
        Err("路径不存在".to_string())
    }
}

fn dir_size_recursive(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += dir_size_recursive(&path);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}