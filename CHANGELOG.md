# 更新日志

本文件记录 treesize 项目的显著变更。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [未发布]

### 新增

- **Windows 快速目录枚举器** `infrastructure/win_enum.rs`
  - 基于 `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)`，64 KB 8 字节对齐缓冲，
    一次系统调用同时取回文件名、类型、逻辑长度、**分配大小**、修改时间、reparse tag
  - 字段偏移全部用 `std::mem::offset_of!` 从结构体定义推导，读取前逐字段做边界检查
  - 非配对 UTF-16 代理项容错解码，保留原始 `OsStr`，保证拼接路径始终可打开
  - 支持扩展路径（`\\?\`）与 UNC 路径
  - 打开后立即试填缓冲：FAT32 与部分网络重定向器允许打开目录却在列举时才报错，
    提前探明才能干净回落，而不是吐出半个目录后失败
- **跨平台目录读取门面** `infrastructure/dir_reader.rs`
  - `EntryKind` / `RawEntry` 统一 Windows 快速路径与 `std::fs::read_dir` 回落路径
  - `allocated_size_of()` 供单文件扫描使用，走 `FileStandardInfo.AllocationSize`
- `--apparent-size` 命令行参数：默认统计**占用空间**，显式指定时统计**逻辑长度**
- 扫描引擎对比分析文档 `docs/06-扫描引擎对比分析.md`

### 变更

- **MFT 引擎修复非驻留属性列表读取失败的根因：卷设备句柄 `ReadFile` 要求长度为扇区倍数**
  - 诊断证据：所有失败读取的长度（如 15200、424、320 字节）均非 512 倍数，且偏移全部簇对齐；
    同一句柄按簇对齐长度连读 1.67 GB 的 $MFT 主体完全正常
  - `read_volume_span` 新增 `sector` 参数：偏移与长度均为扇区倍数时走原零拷贝快路径，
    否则整段补齐到扇区倍数读入临时缓冲后回填（额外一次内存拷贝，仅限属性列表等小块读取）
  - 修复后 `resolve_nonresident_attr_lists` 成功解析数从 15 提升到 5552，
    带非驻留属性列表的记录（>1 条 `FILE_NAME`、扩展属性、$MFT 等使用 `$ATTRIBUTE_LIST` 的记录）
    其扩展引用全部正确装配
  - 端到端验证：6.9 GiB 大文件（$MFT 记录 1231，属性列表分布 3 VCN）此前 MFT 引擎报 0 B，
    现在大小、扩展跨区 run 全部恢复正确，整目录口径与 Fs 引擎一致
- **`fs_scanner` 重写为 `rayon::scope` 结构化并发遍历**，移除 `jwalk` 依赖
  - 递归栈深度 O(1)，逐目录一次系统调用
  - 计数按 1024 条批量提交，避免每条目争抢同一条原子量
  - `MemoryMonitor` 改为加锁访问
  - 深度与树深度解耦
- **大小口径默认从 `meta.len()` 改为 `AllocationSize`**
  - 修复了原先 `GetCompressedFileSizeW` 对普通文件返回**文件长度**而非占用空间的问题
  - 默认输出与 `du` / TreeSize / WizTree 的口径一致
- **`detect_best_engine` 增加 MFT 权限预检**
  - 仅当卷为 NTFS **且** 当前进程能打开卷设备时才选择 MFT 引擎
  - 否则回落 Fs 引擎并输出 WARN——否则默认 `Auto` 会让所有非管理员用户扫描直接失败
- `is_ntfs_volume` 改用 `GetVolumeInformationW` 判定文件系统名
  - 读引导扇区需要管理员权限，不能用于「是不是 NTFS」的探测
- `ScanOptions` 新增 `apparent_size` 字段，默认 `false`（占用空间）
- MSRV 提升至 Rust 1.80

### 修复

- **MFT 引擎在 NTFS 上完全不可用**（两个独立根因）
  - *根因 1：几何字段取自错误的结构偏移。* 代码按 MSDN 的
    `NTFS_VOLUME_DATA_BUFFER` 布局读取 `FSCTL_GET_NTFS_VOLUME_DATA`，但 Windows 11 上
    该布局对不上：`SectorsPerCluster@16` 实测为 `60891903`、`BytesPerSector@24` 为
    `29397102`，与真实的 `8` / `512` 完全不符，导致簇大小下溢、字节偏移变成 0，
    最终报 `MFT 字节偏移无效`。**改法**：几何信息改从 NTFS 引导扇区读取
    （字段偏移由 NTFS 格式规范固定、可自校验），FSCTL 只保留已验证正确的
    `MftValidDataLength@56` 与卷序列号；新增 `BootGeometry::decode_record_size`
    处理引导扇区 `0x40` 的双编码约定（正数 = 簇数，负数 = 2^(-v) 字节）。
  - *根因 2：使用目录句柄读取裸扇区。* 代码打开的是 `\\?\C:\`（Win32 目录路径），
    这种句柄只支持元数据类 `DeviceIoControl`，按字节偏移 `ReadFile` 直接返回
    `ERROR_INVALID_FUNCTION(1)`。MFT 直读必须使用卷设备路径 `\\.\C:`。
    **改法**：新增 `path_to_volume_device()` 与 `VolumeHandle::open()`；
    `ERROR_ACCESS_DENIED` 映射为专门的 `NtfsError::NeedAdministrator`，
    错误文案直接给出可操作建议（提权重跑或改用 `--engine fs`）。
- **枚举器无限重放目录内容**（内存暴涨至 25 GB）
  - `GetFileInformationByHandleEx` 返回的是 `BOOL` 而非写入字节数，原代码误当字节数，
    导致只解析 1 字节；`done` 标志又只在 `cursor >= filled` 分支检查，
    而记录链 terminator 会把游标归零后永远走不到该分支
  - 改用 `Option<usize>` 游标形态，沿 `NextEntryOffset` 链解析至 0
- **`follow_links = true` 时的死循环（挂死）风险**
  - 用 `canonicalize` 后的真实路径去重，闭环目录环检测
- **卷句柄泄漏**：`close_handle(HANDLE)` 改为 `VolumeHandle` newtype + `Drop`，
  解引用裸指针的 `unsafe` 收敛到 `VolumeHandle::open` 一处，
  错误分支不再需要手动关闭
- **树构建递归深度导致栈溢出（MFT/USN）**
  - GUI 扫描线程使用 `std::thread::Builder` 设置 `stack_size = 256 MB`
  - `mft_scanner::rebuild_tree` 与 `usn_scanner::build_tree_from_usn` 使用 `stacker::maybe_grow`（red_zone 64 KB，stack_size 256 MB）扩栈防护
  - 新增依赖 `stacker = "0.1"`
- **目录大小递归（工具函数）改为迭代 DFS，消除潜在栈溢出**（`app.rs::dir_size_recursive`）
- **MFT/USN 树构建在父指针环上无限递归导致内存暴涨（GUI 卡死，7 GB+ 持续增长）**
  - MFT 记录的 `$FILE_NAME` 父指针未经校验，损坏或硬链接数据可形成
    自引用 / `A→B→A` / 回指根的引用环；`build_recursive` 无环检测，
    无限递归不断分配 `FileNode`（`stacker::maybe_grow` 只防栈溢出，防不住堆分配）
  - 修复：`mft_scanner::rebuild_tree` 与 `usn_scanner::build_tree_from_usn`
    增加 visited 集合，同一记录号只允许建树一次，命中时 DEBUG 日志标明
  - 新增单元测试 `rebuild_tree_breaks_parent_cycles`（自引用 + 回指根数据）
- **MFT 直读引擎数据缺失/整树错位（CLI/GUI 双引擎口径对账修复，4 个根因）**
  - *根因 1：$MFT 多 extent 读取错误。* `$MFT` 在 NTFS 上由多个非连续
    extent 组成（实测本机 7 个 extent），旧实现按固定字节偏移一次连续读，
    89% 的“记录”读到的是其他数据区（1,636,608 条原始记录里只有 17.7 万条
    魔数合法）。**改法**：解析 `$MFT` 记录 0 的 `$DATA` runlist
    （mapping pairs，含符号扩展的 LCN 解码），按 extent 逐段读取按 VCN 装配。
    修复后有效记录从 178,430 → **1,343,567**（7.5 倍）。
  - *根因 2：MFT 记录号误读。* `FILE_RECORD_SEGMENT_HEADER` 头部没有记录号，
    旧代码把 0x2C-0x2F 当记录号读取，父子关系索引全错、子树展开失败。
    **改法**：记录号由调用方按 $MFT 块序号（chunk 索引）传参提供。
  - *根因 3：扫描根被忽略。* `rebuild_tree` 固定从卷根（记录 5）建树，
    扫描 `C:\Apps` 返回整卷数据。**改法**：沿路径逐级匹配目录名
    （Windows 大小写不敏感）定位扫描根记录号，仅构建该子树；
    根不在元数据中时报错回落 Fs 引擎而非返回整卷。
  - *根因 4：大小口径不一致。* MFT 之前取 `$DATA` 真实长度（逻辑长度），
    与 Fs 引擎默认的分配大小（占用空间）不对齐。**改法**：同时解析
    非驻留 `$DATA` 头 0x28（分配大小）与 0x30（真实长度），
    按 `--apparent-size` 选择，MFT/USN 与 Fs 口径一致。
  - 端到端对账（提权 CLI，`C:\Apps\baidu`）：
    文件数/目录数与 Fs 引擎完全一致（3,854 文件 / 351 目录）；
    大小差异 ~3%（MFT `$DATA` allocated 字段与 API AllocationSize
    在压缩/稀疏文件上的口径差，记录为后续调优项）。
- **`cargo build --release` 编译警告清零**
  - `egui::Stroke::new(1.0, ...)` 等 16 处浮点字面量补上显式 `f32` 类型，
    消除 `float_literal_f32_fallback` 警告
    （涉及 `theme.rs` / `widgets.rs` / `treemap.rs` / `sunburst.rs` /
    `render/trend.rs` / `render/dashboard.rs`；该警告在未来 Rust 版本将升级为硬错误）
- **GUI 全卷扫描慢 + 内存高（C:\ 扫描实测根因与修复）**
  - *CPU 争抢：解析被拖慢 65 倍。* 第一轮扫描完成后 GUI 的缓存计算线程
    （Dashboard 页含 `DuplicateService` 哈希）立即启动，与第二轮扫描共用
    全局 rayon 线程池：MFT 解析（纯内存）从 0.4s（`C:\Apps` 实测）被拖到
    25.6s，且哈希读盘与 MFT 读取争抢磁盘 I/O。**改法**：扫描运行期间
    `refresh_cache` 直接返回，不基于旧树启动计算（旧结果在扫描完成后
    因代次变化作废，本就是无效计算）。
  - *内存：大缓冲区持有到底。* 1.67 GB 的 MFT 原始缓冲区与 135 万条
    解析结果一直持有到 `scan()` 返回。**改法**：解析完成后立即
    `drop(raw_data)`，条目生成后立即 `drop(parsed_records)`，
    树构建阶段的常驻内存减少约 2 GB。
  - *重复文件哈希与扫描争抢线程池。* 重复缓存计算（blake3 快速/全量哈希）
    使用全局 rayon 线程池，与 MFT 扫描共享全部核心，磁盘 I/O 互相干扰。
    **改法**：`DuplicateService` 哈希阶段（Stage 3 快速哈希、Stage 5 全量
    哈希）迁移到低并发专用线程池（一半核心、最多 4 线程），扫描运行期间
    不再与 MFT 解析争抢资源。
- **MFT 引擎整卷统计虚增（约等于卷容量的一次性多算）**
  - `$BadClus`（MFT 记录 8，坏簇容器）的非驻留 `$DATA` 流 `$BAD` 的
    allocated/real size 字段恒等于**整卷容量**（本机 146.48 GiB），
    大小解析照常计入，导致 MFT 引擎扫描 `C:\` 总量虚增一整卷
    （245.48 GiB → 实际 99.00 GiB）。WizTree / Everything 等同类工具均排除该记录
  - **改法**：`ntfs_reader` 定义 `BAD_CLUS_RECORD` 常量与 `is_bad_clus()`，
    `parse_mft_record` / `parse_mft_record_mut` 的非驻留 `$DATA` 分支
    对记录 8 跳过大小解析；条目本身保留（树中显示 0 B，与其他元文件一致）
  - 提权复扫实测：总量 245.48 → **99.00 GiB**，
    与 Fs 引擎口径（106.73 GiB 含权限错误目录缺口）对齐
- **Dashboard 不再自动触发重复文件哈希（消除空闲态高频轮询空转）**
  - 之前 `refresh_cache` 在 Dashboard 页把「重复文件」哈希与快速聚合放进同一判定：
    整卷扫描后 `DuplicateService::scan` 全量内容哈希需数分钟，哈希未完导致
    `cache_computing` 恒真、UI 退化为 50ms 轮询空转（空闲 CPU 长期 24–63%），
    且「最大文件 / 最大目录 / 主要类型」等毫秒级洞察也被慢哈希阻塞、一直显示「计算中…」。
  - **改法**：把"是否触发重复文件计算"抽成纯函数 `compute_cache_plan`，
    其 `needs_dup` **仅由「重复文件」页触发**，Dashboard 不再自动算。
    快速聚合（TopN / 分类 / 大文件 / 可清理）与重复哈希走两条独立后台线程，
    互不覆盖、互不阻塞；`control_frame_rate` 仅当两路都收敛才回空闲。
  - Dashboard 的「重复文件」洞察行：未分析时显示
    「未分析（切换到「重复文件」页）」引导，而非整行消失。
  - 新增单元测试 `dashboard_fast_plan_ignores_duplicates` 守护该契约：
    若有人把 duplicates 重新塞回 Dashboard 触发条件，测试立即失败。

### 已知问题

- **MFT 数据读取路径尚未端到端验证。** NTFS 直读 MFT 需要管理员权限，
  当前开发环境不在 Administrators 组内，`\\.\C:` / `$MFT` / `$Boot` 均返回
  `ERROR_ACCESS_DENIED`，无法提权。已验证的是引导扇区解析（6 个纯函数单元测试）、
  路径构造、权限预检、错误映射与引擎回落；`read_mft_raw → parse_mft_records →
  build_tree` 这条链仍需在管理员会话下与 Fs 引擎对账。
- 代码层 lint 已清零：`cargo clippy --all-targets` 除 glutin/winit 上游
  future-incompat 提示外无任何代码警告（历史遗留的 66 条 `ntfs_reader` /
  `scan_engine` 可派生项已在本轮一并清理）。
- **实时 GUI 真机复验受当前沙箱策略限制**：应用读取系统主题会触发被禁用的
  `reg.exe`，无法在本会话启动窗口截屏目视。逻辑修复由单元 / 集成测试覆盖
  （`cargo test` 全绿），卷级对账、Treemap、设置等视觉项已在策略允许时段的
  前序会话完成截图验证。

### 性能

release 构建，热缓存，单次运行：

| 目录 | 文件数 | 耗时 | 吞吐 | 峰值内存 |
|---|---:|---:|---:|---:|
| `C:/Users/adm/.cargo/registry` | 60,195 | 709 ms | 84,901 文件/s | 77.6 MB |
| `C:/Program Files` | 61,394 | 558 ms | 110,025 文件/s | — |
| `D:/19-Training/Rust` | 75,893 | 698 ms | 108,729 文件/s | — |
| `C:/Windows/System32` | 14,551 | 342 ms | 42,546 文件/s | 24.8 MB |

正确性交叉验证（`registry`，独立基准为 PowerShell）：

| 指标 | 基准 | treesize |
|---|---:|---:|
| 逻辑长度总和 | 1,960,803,267 | 1,960,803,267 |
| 文件数 | 60,195 | 60,195 |
| 目录数 | 13,750 | 13,750 |

分配大小总和为 2,064,691,288 字节，比逻辑长度高 5.30%（簇对齐开销）。

## [0.1.0] - 初始版本

- 树形目录 + Treemap 磁盘占用分析
- 多维度报表、文件分类统计、重复文件扫描
- CLI 与 GUI 双模式运行