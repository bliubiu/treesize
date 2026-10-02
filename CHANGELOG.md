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

### 已知问题

- **MFT 数据读取路径尚未端到端验证。** NTFS 直读 MFT 需要管理员权限，
  当前开发环境不在 Administrators 组内，`\\.\C:` / `$MFT` / `$Boot` 均返回
  `ERROR_ACCESS_DENIED`，无法提权。已验证的是引导扇区解析（6 个纯函数单元测试）、
  路径构造、权限预检、错误映射与引擎回落；`read_mft_raw → parse_mft_records →
  build_tree` 这条链仍需在管理员会话下与 Fs 引擎对账。
- 存在 66 条既有 clippy warning（`ntfs_reader.rs` 可折叠的 `if`、`vec!` 可换数组；
  `scan_engine.rs` 的 `impl Default` 可派生），本轮只清除了 error，未改动这些无关代码。

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