## treesize 项目
使用Rust 技术栈实现树形目录 + 树形图 (Treemap) 磁盘占用分析；多维度报表、文件分类统计、重复文件扫描；支持CLI和GUI模式运行。单个可执行文件运行。

### 功能
#### 集成工具优势
- WizTree ：快速 MFT 扫描思路 → rayon 并行扫描
- WinDirStat ：Squarified Treemap 可视化
- ncdu/diskonaut ：CLI 交互式分析
- TreeSize Free ：多维度报表 + TopN 排序
- Disk Savvy ：文件分类统计
- WinTrim ：重复文件检测


### 技术栈
- 基于**DDD 架构（领域驱动设计,4层）**和**TDD（测试驱动开发）**设计思想
- Rust
- clap（CLI 参数解析）
- egui + eframe（GUI 框架）
- 中文字体内嵌（SIL 协议，无版权风险）：Noto Sans SC + 系统默认字体（保底策略）


### 日志系统

支持多日志级别（DEBUG、INFO、ERROR），并提供文件输出和日志轮转功能。

- 日志级别：支持 DEBUG、INFO、ERROR 三个级别，可通过配置文件指定日志级别，默认 INFO 级别
- 日志格式：`[YYYY-MM-DD HH:MM:SS.SSS] [级别] [线程ID] [模块:行号] - 日志内容`
- 日志输出：支持文件输出和终端输出，文件输出路径默认 `logs/`，日志文件按日期轮转（每日一个文件），保留 32 天日志，自动清理过期日志
- 日志内容：统一使用中文，清晰记录操作行为、执行结果、错误信息，便于问题排查和审计
- 日志内容不能记录敏感信息，必须严格遵循脱敏策略
- 日志文件命名格式：`treesize-YYYYMMDD.log`，例如 `treesize-20231231.log`


### 文档和注释

- 项目文档统一存储 `docs/` 目录，按顺序标号命名中文文档
- 注释、日志内容统一使用中文
- 统一错误处理：所有错误信息都必须使用中文，禁止打印明文密码等敏感信息

### 成熟项目参考
- TreeSize Free ：多维度报表 + TopN 排序
- Disk Savvy ：文件分类统计
- WinTrim ：重复文件检测
- ncdu/diskonaut/diskusage ：CLI 交互式分析
- WinDirStat ：Squarified Treemap 可视化
- WizTree ：快速 MFT 扫描思路 → rayon 并行扫描
- SpaceSniffer
- SpaceMonger
- SquirrelDisk



### 约束和禁止条件

- 禁用 npm 包管理，可用pnpm，bun
- 禁用Maven，可用gradle

- 必须编译配置优化，从源头减小体积



## 附录

### 参考连接

- https://github.com/tobi/disktree
- 仓库： git@github.com:bliubiu/treesize.git



# AI 行为准则（Karpathy Standard）

## 核心原则

1. **先澄清，不假设** — 需求模糊时先问清楚，不猜测意图
2. **简洁优于聪明** — 简单、可读、最小化代码，不过度设计
3. **最小改动** — 只做必要修改，不改无关代码、不重构整个文件
4. **目标驱动** — 聚焦当前任务，不加"多余"功能

## 语言规则

1. 所有回复、解释、对话使用中文
2. 全程保持纯中文交互
3. 注释和文档使用中文
