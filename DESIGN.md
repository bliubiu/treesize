---
title: treesize GUI 设计系统（egui 设计 Token）
version: alpha
description: treesize 基于 egui + eframe 的桌面 GUI 设计语言与 Token 规范。覆盖明暗双主题色板、字体栈、圆角/间距策略、组件 Token、文件分类着色映射、仪表盘目录专用色板与卷级对账卡片。作为 GUI 视觉实现的唯一权威参考。
---

# treesize GUI 设计系统

> 适用范围：本仓库 `src/gui/*` 全部界面（工具栏、状态栏、Tab 栏、仪表盘、Treemap、Sunburst、TopN、分类、重复、浪费、趋势、收集、差异、设置）。
> 实现位置：`src/gui/theme.rs`（ThemeColors / apply_theme）、`src/gui/widgets.rs`（通用组件）、`src/gui/fonts.rs`（字体族注册）、`src/gui/render/dashboard.rs`（签名条目录色板 / 卷级对账卡片）、`src/gui/app/`（扫描/缓存子模块）、`src/domain/value_objects.rs`（FileCategory 着色）、`src/infrastructure/volume_usage.rs` + `scan_diagnostics.rs`（对账数据）。

## 1. 设计原则

1. **工具优先于装饰**：信息密度 > 视觉花哨。配色克制，层级靠背景深浅 + 细描边表达，不依赖重投影。
2. **冷色调语义**：主强调色采用青蓝（Cyan/Teal），传达「数据流 / 冷存储」的工程气质；语义色（成功/危险/警告）仅在状态与操作处出现。
3. **明暗双主题同源**：界面语义色全部来自同一套 `ThemeColors` Token，深色/浅色只是同一组语义的不同取值，禁止在组件里写死颜色。
4. **数据编码色独立**：文件分类色与目录段色板属于「数据编码色」，由 domain/渲染层以固定 RGB 定义、与主题语义色分离，明暗主题下保持一致，便于跨主题对照。
5. **中文优先**：界面文案全中文；数值、路径、字节大小使用等宽字体以对齐。
6. **圆角克制**：默认 4px 小圆角，Treemap 单元格直角（0），不滥用大圆角。

## 2. 颜色 Token（ThemeColors）

所有组件只能引用下列 Token，不得硬编码 RGB。两套主题由 `apply_theme(ctx, dark)` 注入到 egui `Visuals` 并额外暴露为 `ThemeColors` 供自绘（Treemap / 图例 / 签名条）使用。

### 2.1 深色主题（默认，`dark = true`）

| Token | 值 | 用途 |
|---|---|---|
| `bg_primary` | `#0A0E14` | 主背景（panel） |
| `bg_surface` | `#141A23` | 表层 / 窗口底 |
| `bg_card` | `#1C2433` | 卡片底 |
| `bg_hover` | `#263043` | 悬停 / 徽标底 |
| `accent` | `#00BCD4` | 主强调（青蓝） |
| `accent_hover` | `#4DD0E1` | 强调悬停 |
| `accent_dim` | `#0097A7` | 弱化强调 |
| `text_primary` | `#ECEFF1` | 主文字 |
| `text_secondary` | `#90A4AE` | 次级文字 |
| `text_dim` | `#546E7A` | 辅助 / 占位 |
| `border` | `#263238` | 描边 |
| `border_light` | `#37474F` | 次级描边 / 图例框 |
| `success` | `#66BB6A` | 成功 / 无重复 |
| `danger` | `#EF5350` | 危险 / 浪费 |
| `warn` | `#FFB74D` | 警告 / 提示 |
| `tab_active_bg` / `tab_inactive_bg` | `#1C2433` / `#141A23` | Tab 背景 |
| `tab_text_active` / `tab_text_inactive` | `#00BCD4` / `#90A4AE` | Tab 文字 |
| `bar_track` / `bar_fill` | `#263238` / `#00BCD4` | 占比条底 / 填充 |
| `progress_track` / `progress_fill` | `#1C2433` / `#00BCD4` | 进度条底 / 填充 |
| `group_label` | `#546E7A` | 分组标签 |

### 2.2 浅色主题（`dark = false`）

| Token | 值 |
|---|---|
| `bg_primary` | `#F5F7FA` |
| `bg_surface` | `#FFFFFF` |
| `bg_card` | `#ECEFF1` |
| `bg_hover` | `#E0E4E8` |
| `accent` | `#00838F` |
| `accent_hover` | `#00ACC1` |
| `accent_dim` | `#4DD0E1` |
| `text_primary` | `#263238` |
| `text_secondary` | `#607D8B` |
| `text_dim` | `#90A4AE` |
| `border` | `#CFD8DC` |
| `border_light` | `#DDE6EB` |
| `success` | `#43A047` |
| `danger` | `#E53935` |
| `warn` | `#F57C00` |
| `tab_active_bg` / `tab_inactive_bg` | `#ECEFF1` / `#FFFFFF` |
| `tab_text_active` / `tab_text_inactive` | `#00838F` / `#607D8B` |
| `bar_track` / `bar_fill` | `#CFD8DC` / `#00838F` |
| `progress_track` / `progress_fill` | `#ECEFF1` / `#00838F` |
| `group_label` | `#90A4AE` |

## 3. 字体

- **中文**：优先内嵌 `Noto Sans SC`（SIL 协议，无版权风险）；未内嵌时按平台回退 `msyh.ttc` / `simhei.ttc` / `NotoSansSC*` / `STHeiti`。
- **等宽**：用于字节大小、路径、数值对齐。按平台探测 `Cascadia Code` / `Consolas` / `Menlo` / `DejaVu Sans Mono`，缺失则退回 egui 默认。
- **字号策略**：**唯一来源是 `app.rs::apply_font_scale`**，以 `font_scale`（0.8–2.0）整体缩放，基准 `Heading 20 / Body 16 / Monospace 14 / Button 16 / Small 13`（px），并统一设置 `item_spacing (8, 6)`、`button_padding (10, 4)`。
  - `fonts.rs::setup_chinese_fonts` **只注册字体族**（`ctx.set_fonts`），不再设置 `text_styles`。此前写死的 Body 18 等静态字号会被 `apply_font_scale` 每帧覆盖，属冗余定义，已删除。
  - 约定：新增字号/间距调整一律改 `apply_font_scale`，不得在 `fonts.rs` 另设一套。

## 4. 圆角与间距

- **圆角**：统一 `Rounding::same(4.0)`（卡片、Tab、占比条背景、输入框）；图例框 4；**Treemap 单元格 0（直角）**；进度条/占比条填充 2。避免大圆角。
- **卡片内边距**：`Margin::symmetric(12, 8)`（stat_card）或 `(10, 8)`（洞察块）。
- **区块间距**：`ui.add_space(8.0)` 分隔主要区块；网格 `spacing([16, 6])`。
- **Tab 栏**：组标签 `text_dim` 小字 + 分隔线；Tab 选中态底部 2px `accent` 横线。

## 5. 组件 Token

| 组件 | 实现 | 视觉规范 |
|---|---|---|
| `stat_card` / `stat_card_sized` | `widgets::stat_card` | `bg_card` 底 + `border` 描边 + 4px 圆角；标签 `text_secondary` small，值 `accent`/语义色 strong；默认最小宽 120 |
| `render_tab_button` | `widgets::render_tab_button` | 选中 `tab_active_bg` + `tab_text_active` + 底部 2px `accent` 横线；徽标 `bg_hover` 圆角 8，10px 字号 |
| `bar_cell_color` | `widgets::bar_cell_color` | `bar_track` 底 + 自定义 `fill`；宽 120 高 10 圆角 2 |
| `insight_row` | `widgets::insight_row` | 两列网格：标签 `text_secondary` + 值（`text_primary` 名 + `accent` 等宽大小） |
| `render_empty_state` | `widgets::render_empty_state` | 居中图标（文件夹 + 问号）+ 标题 + 提示。签名含 `theme: &ThemeColors`：图标主体 `text_dim`、问号 `bg_surface`（挖空对比）、标题 `text_primary`、提示 `text_secondary`，随明暗主题自动切换 |
| Treemap | `treemap::TreemapView` | Squarified 算法自绘，按 `FileCategory` 着色，目录按深度变暗；布局结果按 (代次, 视口) 缓存为 `Vec<TreemapCell>`，渲染与 hover 命中复用同一份 |
| Sunburst | `sunburst::SunburstView` | 同心环可视化（同色板） |
| 仪表盘签名条 | `render::dashboard::render_signature_bar` | Top 8 堆叠条 + 双列图例；目录段用 §6.1 的 6 色板，文件段用分类色；条形与图例共用 `signature_segment_color` 保证配色一致 |
| 卷级对账卡片 | `render::dashboard::render_volume_reconciliation` | 仅卷根扫描显示；`bg_card` + `border` + 4px 圆角，两列 Grid；差额语义色见 §5.1 |

### 5.1 卷级对账卡片

数据来源：`Snapshot.reconciliation`（`src/infrastructure/volume_usage.rs::reconcile`）。卷空间查询有系统调用开销，因此**在扫描线程里算一次并缓存**，不在渲染帧里枚举卷；CLI 在扫描摘要后输出同等口径的文本段落。

| 元素 | Token / 规则 |
|---|---|
| 容器 | `bg_card` 底 + `border` 1px + 4px 圆角 + `Margin::symmetric(10, 8)` |
| 标题 | `text_primary` strong，「卷级对账 · C:」 |
| 两列 Grid | 标签 `text_secondary`；数值 `text_primary`；`spacing([16, 3])` |
| 差额「未计入」 | `warn`（卷上占用但本次未遍历到） |
| 差额「多算」 | `danger`（口径或硬链接导致计入偏大） |
| 差额「一致」 | `success` |
| 说明文字 | `text_secondary` small，明确「差额不等于扫描出错」及常见来源 |
| 不可读目录 | `CollapsingHeader`（`warn` 文字）+ 内嵌 `ScrollArea` 限高 180；路径 `text_primary`、原因 `text_secondary`，默认折叠避免上百条刷屏（`text_dim` 在深色卡片上小字对比度不足，真机目视后已上调） |

> 设计约束：卡片**只读**，不修正任何扫描数值；颜色只表达差额方向，不表达「错误」。
> 数据口径：卷已用空间 = 总容量 − 当前用户可用空间；本次计入 = 文件节点按当前口径（分配大小 / 表观大小）累加，两者本就不同量。

## 6. 文件分类着色映射（Treemap / 图例 / 仪表盘签名条）

`FileCategory::base_color()` 提供 RGB，供 Treemap 单元格、`draw_legend`、占比条统一使用：

| 类别 | 中文标签 | base_color (RGB) | 预览 |
|---|---|---|---|
| Video | 视频 | `231, 76, 60` | 红 |
| Audio | 音频 | `243, 156, 18` | 橙 |
| Image | 图片 | `46, 204, 113` | 绿 |
| Document | 文档 | `52, 152, 219` | 蓝 |
| Archive | 压缩包 | `155, 89, 182` | 紫 |
| Executable | 可执行 | `236, 112, 99` | 砖红 |
| Source | 源代码 | `26, 188, 156` | 青绿 |
| Database | 数据库 | `241, 196, 15` | 黄 |
| System | 系统 | `149, 165, 166` | 灰蓝 |
| Other | 其他 | `127, 140, 141` | 青灰 |

- **目录（Treemap / Sunburst）**：无独立类别色，统一使用「其他」基色，并按嵌套深度 `× 0.85^depth` 逐级变暗以表达层级。
- **文字对比度**：单元格文字按 `r+g+b > 384` 选用黑/白，保证可读。
- **目录（仪表盘签名条）**：使用独立 6 色色板，见 §6.1，与 Treemap 的深度变暗策略互不混用。

### 6.1 仪表盘签名条目录专用色板（Top 8）

实现：`src/gui/render/dashboard.rs::signature_segment_color(is_dir, category, index)`。

- **目录段**：按 `index % 6` 循环取下列 6 色，相邻目录段色相明显区分；
- **文件段**：仍使用 `FileCategory::base_color()`；
- **条形与图例共用同一函数、同一 `index`**，保证配色严格一致；
- 段内文字对比度沿用 `r+g+b > 384` 选黑/白（如天蓝段用黑字，青/靛蓝段用白字）。

| index % 6 | RGB | 颜色 | 说明 |
|---|---|---|---|
| 0 | `0, 151, 167` | 青 | 与深色主题 `accent_dim` 同源，主目录段 |
| 1 | `63, 81, 181` | 靛蓝 | 冷色相，与青拉开距离 |
| 2 | `121, 134, 203` | 柔蓝 | 中明度冷色 |
| 3 | `38, 166, 154` | 青绿 | 与 `Source` 分类色同族 |
| 4 | `79, 195, 247` | 天蓝 | 高明度，配黑字 |
| 5 | `149, 117, 205` | 紫 | 与 `Archive` 分类色同族 |

> 设计动机：替代原先「目录统一用 `Other` 灰青 + 偶数段 `×0.85` 变暗」的做法——相邻目录段仅差 15% 亮度，几乎无法区分。
> 约束：6 色均为固定 RGB（属 §1 设计原则第 4 条的数据编码色），不随明暗主题切换；若后续需要主题化，应把它们收敛进 `ThemeColors` token，而不是在渲染函数里另写一套。

## 7. 明暗切换与可访问性

- `apply_theme(ctx, dark)`：深/浅两套注入 egui `Visuals`（panel/window/noninteractive/inactive/hovered/active/selection/hyperlink）。
- 跟随系统：Windows 通过 `reg query` 读取注册表判断；非 Windows 当前固定深色（Linux 主题探测较复杂，后续可补）。
- 运行时切换：`LightMode::{System, Follow, ForceLight, ForceDark}`（ForceLight/Dark 优先，Follow 走跟随系统，System 走 OS 原生）。
- 可访问性：强调色与背景对比满足基本可读性；Treemap / 签名条文字黑白自适应；空状态图标随主题取色。

## 8. 改进点跟踪

| 项 | 状态 | 说明 |
|---|---|---|
| 空状态图标 | ✅ 已修复 | 改用 `text_dim` / `bg_surface` / `text_primary` / `text_secondary`，随主题切换 |
| 签名条目录配色 | ✅ 已修复 | 引入 6 色目录色板（§6.1），移除奇偶 `×0.85` 变暗 hack |
| 字体定义 | ✅ 已修复 | 删除 `fonts.rs` 冗余 `text_styles`，统一到 `apply_font_scale` |
| Treemap 流畅度 | ✅ 已修复 | 按 (代次, 视口) 缓存 `Vec<TreemapCell>`，渲染与 hover 命中复用同一份布局 |
| `app.rs` 体积 | ✅ 已拆分 | 扫描控制 → `gui/app/scan_ctl.rs`；聚合缓存 → `gui/app/cache_ctl.rs`（app 子模块 + `pub(super)` 方法） |
| 卷级对账 | ✅ 已新增 | 卷根扫描显示卷已用 / 本次计入 / 未计入差额 / 不可读目录清单（§5.1）；**不改扫描数值** |
| 后续可选项 | — | 渲染视图方法仍集中，可继续按 Tab 拆分；6 色目录色板可收敛进 `ThemeColors`；超大目录 LOD 聚合提示 |
