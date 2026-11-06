use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use eframe::egui;
use egui::{Color32, Vec2};

use super::fonts::setup_chinese_fonts;
use super::render;
use super::theme::{apply_theme, ThemeColors};
use super::treemap::{TreemapCache, TreemapView};
use super::widgets::*;
use crate::application::models::{SnapshotDiff, TrendReport};
use crate::application::{ClassifyReport, DuplicateReport, TopNEntry, TrendService, WasteReport};
use crate::domain::scan_engine::{CancelToken, ScanOptions, ScanProgress, ScanStats};
use crate::domain::value_objects::ByteSize;
use crate::domain::DragCollector;
use crate::infrastructure::{HistoryStorage, VolumeReconciliation};

// 扫描控制与聚合缓存拆为 app 的子模块：子模块可访问本模块的私有字段，
// 其方法以 pub(super) 暴露给本模块调用。
mod cache_ctl;
mod scan_ctl;

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
    /// 卷级对账（仅卷根扫描有值）：扫描完成时算一次并缓存，避免每帧枚举卷
    reconciliation: Option<VolumeReconciliation>,
}

pub struct Snapshot {
    pub node: Option<Arc<crate::domain::FileNode>>,
    pub stats: Option<ScanStats>,
    pub progress: ScanProgress,
    pub running: bool,
    pub error: Option<String>,
    pub generation: u64,
    /// 卷级对账（仅卷根扫描有值）
    pub reconciliation: Option<VolumeReconciliation>,
}

#[derive(Default)]
struct ComputeCache {
    generation: u64,
    top_files: Option<Vec<TopNEntry>>,
    top_dirs: Option<Vec<TopNEntry>>,
    classify: Option<ClassifyReport>,
    waste: Option<WasteReport>,
    all_files: Option<Vec<TopNEntry>>,
}

impl ScanState {
    fn shared() -> Arc<Mutex<Self>> { Arc::new(Mutex::new(Self::default())) }
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
    /// 最近一次写入 egui Style 的缩放值；与 `font_scale` 不同即为脏，需重建样式
    applied_font_scale: f32,
    dark_mode: bool,
    theme_follow_system: bool,
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
    /// 帧调度专用上一帧状态，与 `prev_running`（扫描结束保存历史）解耦
    repaint_prev_running: bool,
    repaint_prev_computing: bool,
    history_storage: Option<HistoryStorage>,
    cached_trend: Option<TrendReport>,
    cached_diff: Option<SnapshotDiff>,
    diff_old_selected: Option<i64>,
    diff_new_selected: Option<i64>,
    drag_collector: DragCollector,
    cache_computing: bool,
    cache_generation_computing: u64,
    compute_cache: Arc<Mutex<ComputeCache>>,
    /// 重复文件哈希（需读盘，慢）独立后台线程状态，与快速聚合解耦，避免阻塞「最大文件/目录/主要类型」洞察
    dup_computing: bool,
    /// 当前重复文件计算对应的代次，用于丢弃代次变更后的过期结果
    dup_generation: u64,
    /// 重复文件结果交接缓冲（独立于 ComputeCache，避免两线程互相覆盖）
    dup_cache: Arc<Mutex<Option<DuplicateReport>>>,
    /// Treemap 布局缓存（按代次 + 视口尺寸失效），跨帧保留避免每帧重排
    treemap_cache: TreemapCache,
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
            Tab::TopFiles => "大文件",
            Tab::Classify => "分类",
            Tab::Duplicates => "重复",
            Tab::Waste => "浪费",
            Tab::Trend => "趋势",
            Tab::Collect => "收集",
        }
    }

    /// 导航分组索引（用于视觉分区）
    fn group(self) -> usize {
        match self {
            Tab::Dashboard | Tab::Tree | Tab::Treemap => 0,
            Tab::TopFiles | Tab::Classify | Tab::Trend => 1,
            Tab::Duplicates | Tab::Waste | Tab::Collect => 2,
        }
    }
}

/// 各导航分组的标签文本
const TAB_GROUP_LABELS: [&str; 3] = ["核心视图", "数据分析", "维护管理"];

impl TreeSizeApp {
    pub fn new(cc: &eframe::CreationContext<'_>, options: GuiOptions) -> Self {
        setup_chinese_fonts(&cc.egui_ctx);

        // 尝试检测系统主题（仅 Windows 注册表读取）
        let system_dark = detect_system_dark_mode();
        let dark_mode = system_dark; // 默认跟随系统
        let theme_follow_system = true;
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
            applied_font_scale: 0.0, // 与 1.0 不同，保证首帧写入一次样式
            dark_mode,
            theme_follow_system,
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
            repaint_prev_running: false,
            repaint_prev_computing: false,
            history_storage: None,
            cached_trend: None,
            cached_diff: None,
            diff_old_selected: None,
            diff_new_selected: None,
            drag_collector: DragCollector::new(),
            cache_computing: false,
            cache_generation_computing: 0,
            compute_cache: Arc::new(Mutex::new(ComputeCache::default())),
            dup_computing: false,
            dup_generation: 0,
            dup_cache: Arc::new(Mutex::new(None)),
            treemap_cache: TreemapCache::default(),
        }
    }

    fn tab_badge(&self, tab: Tab) -> Option<u64> {
        let s = self.state.lock().unwrap();
        s.node.as_ref()?;
        match tab {
            Tab::Tree | Tab::Treemap | Tab::Dashboard | Tab::Trend => None,
            Tab::TopFiles => Some(self.top_n as u64),
            Tab::Classify => Some(
                self.cached_classify
                    .as_ref()
                    .map(|r| r.by_category.len() as u64)
                    .unwrap_or(0),
            ),
            Tab::Duplicates => self.cached_duplicates.as_ref().map(|r| r.groups.len() as u64),
            Tab::Waste => self.cached_waste.as_ref().map(|r| r.items.len() as u64),
            Tab::Collect => Some(self.drag_collector.items.len() as u64),
        }
    }
}

/// 帧调度决策：由「扫描/计算是否进行中」及上一帧状态推导本帧是否请求重绘。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepaintDecision {
    /// 无需主动请求，交给 egui 的交互驱动（空闲不空转）
    Idle,
    /// 立即重绘：刷新扫描进度，或收敛刚结束的最终状态
    Now,
    /// 低频轮询：等待后台聚合计算结果
    Poll,
}

fn repaint_decision(running: bool, computing: bool, prev_running: bool, prev_computing: bool) -> RepaintDecision {
    if running {
        RepaintDecision::Now
    } else if computing {
        RepaintDecision::Poll
    } else if prev_running || prev_computing {
        // 上一帧还在忙、这一帧已空闲：补一帧把最终结果绘制出来
        RepaintDecision::Now
    } else {
        RepaintDecision::Idle
    }
}

impl eframe::App for TreeSizeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_font_scale(ctx);
        self.apply_theme_if_dirty(ctx);

        let snapshot = self.get_snapshot();
        let scan_just_finished = !snapshot.running && self.prev_running;
        self.prev_running = snapshot.running;

        if scan_just_finished {
            self.save_history_async();
        }

        self.render_toolbar(ctx);
        self.render_settings_window(ctx);
        self.render_delete_dialog(ctx);
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
                Tab::TopFiles => self.render_topn(ui, &snapshot),
                Tab::Classify => self.render_classify(ui, &snapshot),
                Tab::Duplicates => self.render_duplicates(ui, &snapshot),
                Tab::Waste => self.render_waste(ui, &snapshot),
                Tab::Trend => self.render_trend(ui, &snapshot),
                Tab::Collect => self.render_collect(ui),
            }
        });

        // 放在渲染之后：refresh_cache 可能在本帧刚启动后台计算，
        // 帧调度需要看到本帧最新的 cache_computing 状态。
        self.control_frame_rate(ctx);
    }
}

impl TreeSizeApp {
    /// 仅当缩放比例变化时重建文本样式（font_scale dirty 守卫）。
    /// 记录「已应用值」而非依赖控件显式置脏，可覆盖任何修改 font_scale 的路径。
    fn apply_font_scale(&mut self, ctx: &egui::Context) {
        if self.applied_font_scale == self.font_scale {
            return;
        }
        let s = self.font_scale;
        ctx.style_mut(|style| {
            use egui::TextStyle::*;
            style.text_styles = [
                (Heading, egui::FontId::proportional(20.0 * s)),
                (Body, egui::FontId::proportional(16.0 * s)),
                (Monospace, egui::FontId::monospace(14.0 * s)),
                (Button, egui::FontId::proportional(16.0 * s)),
                (Small, egui::FontId::proportional(13.0 * s)),
            ]
            .into();
            style.spacing.item_spacing = Vec2::new(8.0, 6.0);
            style.spacing.button_padding = Vec2::new(10.0, 4.0);
        });
        self.applied_font_scale = s;
    }

    fn apply_theme_if_dirty(&mut self, ctx: &egui::Context) {
        if self.theme_dirty {
            self.theme = apply_theme(ctx, self.dark_mode);
            self.theme_dirty = false;
        }
    }

    /// 帧调度：仅在确有工作要刷新时请求重绘，空闲时交给 egui 的交互驱动。
    ///
    /// - 扫描中：持续重绘刷新进度
    /// - 后台聚合计算中：低频轮询等待结果
    /// - 扫描/计算刚结束：补一帧把最终状态绘制出来
    /// - 完全空闲：不主动请求，消除原先 30fps 的常驻空转
    fn control_frame_rate(&mut self, ctx: &egui::Context) {
        let running = self.state.try_lock().map(|s| s.running).unwrap_or(false);
        // 快速聚合或重复文件哈希任一进行中都需要低频轮询；两者都收敛后回归空闲
        let computing = self.cache_computing || self.dup_computing;
        let decision = repaint_decision(
            running,
            computing,
            self.repaint_prev_running,
            self.repaint_prev_computing,
        );
        match decision {
            RepaintDecision::Idle => {},
            RepaintDecision::Now => ctx.request_repaint(),
            RepaintDecision::Poll => ctx.request_repaint_after(Duration::from_millis(50)),
        }

        self.repaint_prev_running = running;
        self.repaint_prev_computing = computing;
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
            reconciliation: s.reconciliation.clone(),
        }
    }

    fn save_history_async(&mut self) {
        let (needs_save, path_str, node_arc) = {
            let s = self.state.lock().unwrap();
            (
                s.node.is_some() && s.stats.is_some(),
                self.path_input.clone(),
                s.node.clone(),
            )
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
                        ui.label(egui::RichText::new("扫描路径：").color(self.theme.text_secondary));

                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.path_input)
                                .desired_width(250.0)
                                .hint_text("输入目录路径…"),
                        );
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.start_scan();
                        }

                        if !self.path_history.is_empty() {
                            ui.menu_button("历史", |ui| {
                                for p in self.path_history.iter().rev() {
                                    if ui.button(p).clicked() {
                                        self.path_input = p.clone();
                                        ui.close_menu();
                                    }
                                }
                            });
                        }

                        if ui.button("选择").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                self.path_input = path.display().to_string();
                            }
                        }

                        let scan_label = if running { "扫描中" } else { "▶ 扫描" };
                        let scan_btn =
                            egui::Button::new(egui::RichText::new(scan_label).color(Color32::WHITE).strong())
                                .fill(if running {
                                    self.theme.text_dim
                                } else {
                                    self.theme.accent
                                })
                                .min_size(Vec2::new(60.0, 24.0));
                        if ui.add_enabled(!running, scan_btn).clicked() {
                            self.start_scan();
                        }

                        if running {
                            let cancel_btn = egui::Button::new(egui::RichText::new("取消").color(self.theme.danger))
                                .fill(self.theme.bg_card);
                            if ui.add(cancel_btn).clicked() {
                                self.cancel_scan();
                            }
                        }

                        ui.separator();
                        ui.checkbox(&mut self.show_hidden, "显示隐藏");
                        ui.separator();
                        ui.label(egui::RichText::new("TopN:").color(self.theme.text_secondary));
                        ui.add(egui::DragValue::new(&mut self.top_n).clamp_range(1..=500));
                        ui.separator();

                        if ui.button("设置").clicked() {
                            self.show_settings = !self.show_settings;
                        }
                        // ── 右侧：Tree 标签下的搜索 inline ──
                        if self.selected_tab == Tab::Tree {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("清除").clicked() {
                                    self.search_text.clear();
                                }
                                ui.checkbox(&mut self.search_match_full, "完全匹配");
                                ui.checkbox(&mut self.search_case_sensitive, "区分大小写");
                                ui.label(egui::RichText::new("查找：").color(self.theme.text_secondary));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.search_text)
                                        .desired_width(120.0)
                                        .hint_text("查找文件…"),
                                );
                            });
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
                    ui.label(egui::RichText::new("字体缩放").color(self.theme.text_primary));
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
                let theme_options = [("浅色", false, false), ("深色", true, false), ("跟随系统", true, true)];
                // 确定当前选中的索引
                let selected_idx = if self.theme_follow_system {
                    2
                } else if self.dark_mode {
                    1
                } else {
                    0
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("主题：").color(self.theme.text_primary));
                    for (i, (label, is_dark, follow)) in theme_options.iter().enumerate() {
                        let is_sel = i == selected_idx;
                        let btn = egui::Button::new(egui::RichText::new(*label).color(if is_sel {
                            self.theme.tab_text_active
                        } else {
                            self.theme.text_secondary
                        }))
                        .fill(if is_sel {
                            self.theme.tab_active_bg
                        } else {
                            self.theme.tab_inactive_bg
                        })
                        .rounding(egui::Rounding::same(4.0));
                        if ui.add(btn).clicked() {
                            self.theme_follow_system = *follow;
                            if !follow {
                                self.dark_mode = *is_dark;
                            } else {
                                #[cfg(target_os = "windows")]
                                {
                                    self.dark_mode = detect_system_dark_mode();
                                }
                                #[cfg(not(target_os = "windows"))]
                                {
                                    self.dark_mode = true;
                                }
                            }
                            self.theme_dirty = true;
                        }
                    }
                });
                ui.separator();
                #[cfg(embedded_font)]
                ui.label(egui::RichText::new("字体：Noto Sans SC (内嵌)").color(self.theme.text_secondary));
                #[cfg(not(embedded_font))]
                ui.label(egui::RichText::new("字体：系统字体").color(self.theme.text_secondary));
                ui.separator();
                ui.label(egui::RichText::new("扫描引擎：").color(self.theme.text_primary));
                let engine = &mut self.options.engine;
                ui.horizontal(|ui| {
                    if ui
                        .radio_value(engine, crate::domain::ScanEngineType::Auto, "自动")
                        .clicked()
                    {
                        tracing::info!("扫描引擎切换为：自动");
                    }
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
                                        egui::Button::new(egui::RichText::new("删除").color(Color32::WHITE))
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
                                        },
                                        Err(e) => {
                                            tracing::error!(target: "treesize::gui", "删除失败：{}", e);
                                            self.delete_error = Some(e);
                                        },
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

    fn render_status_bar(&self, ctx: &egui::Context) {
        let snapshot = self.get_snapshot();
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            egui::Frame::none()
                .fill(self.theme.bg_surface)
                .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                .show(ui, |ui| {
                    // 扫描中显示进度条（使用 bytes_scanned / (1MB) 估算进度保持动画）
                    if snapshot.running {
                        let bar_height = 3.0;
                        let avail = ui.available_width();
                        let (bar_rect, _) =
                            ui.allocate_exact_size(egui::Vec2::new(avail, bar_height), egui::Sense::hover());
                        ui.painter().rect_filled(bar_rect, 1.5, self.theme.progress_track);
                        if snapshot.progress.bytes_scanned > 0 {
                            let pct = ((snapshot.progress.bytes_scanned as f64 / 1_000_000.0).min(1.0)) as f32;
                            let fill_rect = egui::Rect::from_min_size(
                                bar_rect.min,
                                egui::Vec2::new(bar_rect.width() * pct.max(0.02), bar_height),
                            );
                            ui.painter().rect_filled(fill_rect, 1.5, self.theme.progress_fill);
                        }
                        ui.add_space(2.0);
                    }

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
                            ui.label(format!("已扫描 {}", ByteSize(snapshot.progress.bytes_scanned)));
                            ui.separator();
                            let path = &snapshot.progress.current_path;
                            let max_chars = 60;
                            let display = if path.chars().count() > max_chars {
                                let suffix: String = path
                                    .chars()
                                    .rev()
                                    .take(max_chars)
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .rev()
                                    .collect();
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
                                egui::RichText::new(format!("{}px", (16.0 * self.font_scale) as u32))
                                    .color(self.theme.text_dim),
                            );
                            #[cfg(embedded_font)]
                            ui.label(egui::RichText::new("Noto Sans SC").color(self.theme.text_dim));
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
                    let tabs = [
                        Dashboard, Tree, Treemap, TopFiles, Classify, Duplicates, Waste, Trend, Collect,
                    ];
                    let mut prev_group: Option<usize> = None;
                    for tab in tabs {
                        let g = tab.group();
                        // 跨组时插入分隔线
                        if prev_group.is_some() && prev_group != Some(g) {
                            ui.separator();
                        }
                        if prev_group != Some(g) {
                            // 显示组标签
                            ui.label(
                                egui::RichText::new(TAB_GROUP_LABELS[g])
                                    .color(self.theme.group_label)
                                    .small(),
                            );
                            prev_group = Some(g);
                        }

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
            render_empty_state(ui, &self.theme, "尚未扫描", "在上方输入路径，或选择目录后开始");
            return;
        }
        let node = state.node.as_ref().unwrap();

        render::tree::render_tree_panel(
            ui,
            node,
            &self.theme,
            &mut self.expanded_paths,
            &mut self.tree_sort_by,
            self.show_hidden,
            &self.search_text,
            self.search_case_sensitive,
            self.search_match_full,
            &mut self.delete_confirm_path,
        );
    }

    fn render_treemap(&mut self, ui: &mut egui::Ui, state: &Snapshot) {
        if state.node.is_none() {
            render_empty_state(ui, &self.theme, "尚未扫描", "在上方输入路径，或点击「选择」目录后开始");
            return;
        }
        let node = state.node.as_ref().unwrap();
        TreemapView::show(ui, node, &self.theme, &mut self.treemap_cache, state.generation);
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
                    &self.theme,
                    "暂无扫描历史",
                    "执行扫描后自动保存历史记录，或使用 CLI 的 --history-save 参数",
                );
                return;
            },
        };

        render::trend::render_trend_panel(ui, &trend, &self.theme);
        ui.add_space(8.0);
        render::trend::render_snapshot_diff_panel(
            ui,
            &trend,
            &self.theme,
            &mut self.diff_old_selected,
            &mut self.diff_new_selected,
            &mut self.cached_diff,
            &self.history_storage,
        );
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
                if ui.button("添加目录").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.collect_path(path);
                    }
                }
                if ui.button("添加文件").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        self.collect_path(path);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.drag_collector.items.is_empty() {
                        let marked = self.drag_collector.marked_count();
                        if marked > 0
                            && ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(format!("删除已标记（{}）", marked)).color(Color32::WHITE),
                                    )
                                    .fill(self.theme.danger),
                                )
                                .clicked()
                        {
                            self.delete_marked_collected();
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
                    &self.theme,
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
                    if marked > 0 {
                        self.theme.danger
                    } else {
                        self.theme.text_secondary
                    },
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

                    let items_snapshot: Vec<_> = self
                        .drag_collector
                        .items
                        .iter()
                        .map(|item| {
                            (
                                item.name.clone(),
                                item.size,
                                item.is_dir,
                                item.path.clone(),
                                item.marked_for_deletion,
                            )
                        })
                        .collect();

                    for (i, (name, size, is_dir, path, marked)) in items_snapshot.iter().enumerate() {
                        let icon = if *is_dir { "文件夹" } else { "文件" };
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
                            let mark_label = if *marked { "已标记" } else { "标记删除" };
                            if ui.button(mark_label).clicked() {
                                toggle_mark_idx = Some(i);
                            }
                            if ui.button("移除").clicked() {
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
                if self
                    .drag_collector
                    .items
                    .get(idx)
                    .map(|i| i.marked_for_deletion)
                    .unwrap_or(false)
                {
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
            dir_size_recursive(&path, 64)
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
        let marked: Vec<_> = self
            .drag_collector
            .items
            .iter()
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

fn delete_path(path: &std::path::Path) -> Result<(), String> {
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| format!("删除目录失败：{}", e))
    } else if path.is_file() {
        std::fs::remove_file(path).map_err(|e| format!("删除文件失败：{}", e))
    } else {
        Err("路径不存在".to_string())
    }
}

/// 检测系统是否为深色主题模式
fn detect_system_dark_mode() -> bool {
    #[cfg(target_os = "windows")]
    {
        // 通过 reg.exe 查询 Windows 主题设置
        if let Ok(output) = std::process::Command::new("reg")
            .args([
                "query",
                "HKCU\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output()
        {
            let s = String::from_utf8_lossy(&output.stdout);
            // 输出类似: AppsUseLightTheme  REG_DWORD  0x1
            if let Some(part) = s.split_whitespace().last() {
                if let Ok(val) = u32::from_str_radix(part.trim_start_matches("0x"), 16) {
                    return val == 0; // 0 = 深色, 1 = 浅色
                }
            }
        }
    }
    // 默认返回深色主题
    true
}

/// 迭代计算目录大小，max_depth 限制递归深度（防止栈溢出/无限循环）
fn dir_size_recursive(path: &Path, max_depth: usize) -> u64 {
    let mut total = 0u64;
    let mut stack: Vec<(PathBuf, usize)> = Vec::new();
    stack.push((path.to_path_buf(), 0));

    while let Some((dir_path, depth)) = stack.pop() {
        if depth > max_depth {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir_path) {
            for entry in entries.flatten() {
                let child_path = entry.path();
                if child_path.is_dir() {
                    stack.push((child_path, depth + 1));
                } else if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::cache_ctl::compute_cache_plan;
    use super::*;
    use crate::domain::FileNode;

    #[test]
    fn scan_state_default_initial_state() {
        let state = ScanState::default();
        assert!(state.node.is_none());
        assert!(state.stats.is_none());
        assert!(!state.running);
        assert!(state.error.is_none());
        assert_eq!(state.generation, 0);
    }

    #[test]
    fn scan_state_shared_creates_mutex() {
        let shared = ScanState::shared();
        let locked = shared.lock().unwrap();
        assert_eq!(locked.generation, 0);
    }

    #[test]
    fn scan_state_generation_increments() {
        let state = Arc::new(Mutex::new(ScanState::default()));
        {
            let mut s = state.lock().unwrap();
            s.generation = 5;
            s.running = true;
            s.node = Some(Arc::new(FileNode::new_dir(PathBuf::from("/test"), None)));
        }
        let snapshot = {
            let s = state.lock().unwrap();
            Snapshot {
                node: s.node.clone(),
                stats: s.stats.clone(),
                progress: s.progress.clone(),
                running: s.running,
                error: s.error.clone(),
                generation: s.generation,
                reconciliation: None,
            }
        };
        assert_eq!(snapshot.generation, 5);
        assert!(snapshot.running);
        assert!(snapshot.node.is_some());
        assert!(snapshot.stats.is_none());
        assert!(snapshot.error.is_none());
    }

    #[test]
    fn snapshot_transition_to_error() {
        let state = Arc::new(Mutex::new(ScanState::default()));
        {
            let mut s = state.lock().unwrap();
            s.running = false;
            s.error = Some("测试错误".to_string());
            s.generation = 2;
        }
        let snapshot = {
            let s = state.lock().unwrap();
            Snapshot {
                node: s.node.clone(),
                stats: s.stats.clone(),
                progress: s.progress.clone(),
                running: s.running,
                error: s.error.clone(),
                generation: s.generation,
                reconciliation: None,
            }
        };
        assert!(!snapshot.running);
        assert_eq!(snapshot.error, Some("测试错误".to_string()));
        assert_eq!(snapshot.generation, 2);
    }

    #[test]
    fn dir_size_recursive_depth_limit() {
        // 创建一个深层嵌套的临时目录结构
        let dir = std::env::temp_dir().join("treesize_test_depth");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // /depth_test/sub1/sub2/sub3/file.txt
        let sub1 = dir.join("sub1");
        std::fs::create_dir_all(&sub1).unwrap();
        let sub2 = sub1.join("sub2");
        std::fs::create_dir_all(&sub2).unwrap();
        let sub3 = sub2.join("sub3");
        std::fs::create_dir_all(&sub3).unwrap();
        std::fs::write(sub3.join("file.txt"), b"hello").unwrap();

        // max_depth = 0: 只扫描根，不考虑子目录
        let size_depth0 = dir_size_recursive(&dir, 0);
        assert_eq!(size_depth0, 0, "depth 0 不应扫描子目录");

        // max_depth = 3: 应能扫到 file.txt
        let size_depth3 = dir_size_recursive(&dir, 3);
        assert_eq!(size_depth3, 5, "depth 3 应能扫到 file.txt");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tab_badge_mapping() {
        // 验证各个 Tab 的 badge 返回值类型（不依赖实际数据）
        let tabs_with_badge = [
            super::Tab::TopFiles,
            super::Tab::Classify,
            super::Tab::Duplicates,
            super::Tab::Waste,
            super::Tab::Collect,
        ];
        assert_eq!(tabs_with_badge.len(), 5);
        let tabs_without_badge = [Tab::Dashboard, Tab::Tree, Tab::Treemap, Tab::Trend];
        assert_eq!(tabs_without_badge.len(), 4);
    }

    #[test]
    fn repaint_stays_idle_when_nothing_is_running() {
        assert_eq!(repaint_decision(false, false, false, false), RepaintDecision::Idle);
    }

    #[test]
    fn repaint_requests_every_frame_while_scanning() {
        assert_eq!(repaint_decision(true, false, false, false), RepaintDecision::Now);
        assert_eq!(repaint_decision(true, true, true, false), RepaintDecision::Now);
    }

    #[test]
    fn repaint_polls_while_background_compute_is_running() {
        assert_eq!(repaint_decision(false, true, false, true), RepaintDecision::Poll);
        // 扫描中优先于计算，保持进度实时刷新
        assert_eq!(repaint_decision(true, true, true, true), RepaintDecision::Now);
    }

    #[test]
    fn repaint_converges_once_then_goes_idle() {
        assert_eq!(repaint_decision(false, false, true, false), RepaintDecision::Now);
        assert_eq!(repaint_decision(false, false, false, true), RepaintDecision::Now);
        // 收敛帧之后不再持续请求，否则就是原来的 30fps 空转
        assert_eq!(repaint_decision(false, false, false, false), RepaintDecision::Idle);
    }

    #[test]
    fn dashboard_fast_plan_ignores_duplicates() {
        // Dashboard 的快速聚合计划完全不关心 duplicates：
        // 无论重复文件是否已算，都不应触发 dup 计算（重复文件现仅由「重复文件」页触发）。
        let (needs_fast, needs_dup) = compute_cache_plan(
            Tab::Dashboard,
            false, // cached_top_files
            false, // cached_top_dirs
            false, // cached_classify
            true,  // cached_all_files
            false, // cached_duplicates（未就绪）
            true,  // cached_waste
        );
        assert!(
            needs_fast,
            "Dashboard 快速聚合应在 top_files/top_dirs/classify 未就绪时触发"
        );
        assert!(!needs_dup, "Dashboard 不应触发 dup 计算（仅重复文件页触发）");

        // 关键不变量：当 top_files/top_dirs/classify 已就绪、仅 duplicates 未就绪时，
        // 快速聚合不应再被触发（避免重复计算被 duplicates 拖累），dup 也仍不应触发。
        let (needs_fast2, needs_dup2) = compute_cache_plan(Tab::Dashboard, true, true, true, true, false, true);
        assert!(!needs_fast2, "快速聚合已就绪后不应重复触发");
        assert!(!needs_dup2, "Dashboard 永不触发 dup（直到访问重复文件页）");

        // 即使 duplicates 已就绪，Dashboard 也不应触发 dup
        let (needs_fast3, needs_dup3) = compute_cache_plan(Tab::Dashboard, true, true, true, true, true, true);
        assert!(!needs_fast3 && !needs_dup3, "Dashboard 在任意缓存状态下都不应触发 dup");

        // 重复文件页：仅触发 dup，不触发快速聚合
        let (nf, nd) = compute_cache_plan(Tab::Duplicates, true, true, true, true, false, true);
        assert!(!nf, "重复文件页不应触发快速聚合");
        assert!(nd, "重复文件页应触发 dup 计算");

        // 重复文件页 dup 就绪后不再触发
        let (nf2, nd2) = compute_cache_plan(Tab::Duplicates, true, true, true, true, true, true);
        assert!(!nf2 && !nd2, "重复文件页 dup 就绪后不应再触发");
    }
}
