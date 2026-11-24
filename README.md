# token-counter

基于 **Rust + Ratatui** 的 AI 编程 Agent 本地 token 用量、费用与速度统计工具。**无本地磁盘存储**，直接只读扫描各 Agent 本地会话库，支持交互式 TUI 面板与纯文本单次 CLI 报表，并提供其他同类工具少见的 **TTFT（首 token 延迟）** 与 **纯解码速度（tokens/s）** 指标。

## 支持的数据源

| Agent | 标识 | 本地数据路径 | 格式 | 调研文档 |
|---|---|---|---|---|
| OpenCode | `opencode` | `~/.local/share/opencode/opencode.db` | SQLite（WAL 只读） | [docs/opencode.md](docs/opencode.md) |
| DeepSeek Harness | `dsh` | `~/.dsh/sessions/**/session.v4.jsonl.zstd`（支持 `$DSH_HOME`） | zstd JSONL（仅 v4） | [docs/dsh.md](docs/dsh.md) |
| Antigravity | `agy` | `~/.gemini/antigravity/conversations/*.db` | SQLite + Protobuf | [docs/agy.md](docs/agy.md) |

## 构建与安装

需要 Rust 1.85+ 工具链：

```bash
# 编译 release 二进制
cargo build --release

# 可选：安装到 ~/.cargo/bin
cargo install --path .

# 运行单元测试
cargo test
```

## 交互式 TUI

直接运行（不带参数）或执行 `tui` 子命令即可启动全屏终端面板：

```bash
token-counter
# 或
token-counter tui
```

### 视图一览

| Tab | 说明 |
|---|---|
| `Overview` | 顶部全量 KPI + 按终端宽度自适应柱数的堆叠柱状图（支持 `Days` / `Hours` 切换）+ 底部全量 `agent · provider · model` 明细 |
| `Hour` | 按小时倒序汇总 + 下方选中小时的模型明细 |
| `Day` | 按天倒序汇总 + 下方选中日期的模型明细 |
| `Week` | 按 ISO 周倒序汇总 + 下方选中周的模型明细 |
| `Month` | 按月倒序汇总 + 下方选中月份的模型明细 |
| `Session` | 上半区会话列表（倒序，共用全局 Agent 筛选含 `all`）+ 下半区选中会话的逐个 Turn 明细（升序） |

### 梯度刷新机制

- 启动先扫描最近 **30 天**，毫秒级渲染首屏与近 30 天柱状图；
- 紧接着后台静默补全 30 天前的历史，并将今日 `00:00` 之前的历史事件冻结驻留内存（零磁盘落盘）；
- 之后每 **60 秒**（或按 `r` 手动刷新）仅重扫**今天（1 天）**再与内存冻结历史无缝拼合。

### 快捷键与鼠标操作

| 操作 | 作用 |
|---|---|
| `←` / `→` / `h` / `l` 或鼠标点击 Tab | 切换顶部视图 |
| `Tab` / `Enter` 或鼠标点击面板 | 在上下两个子框之间切换焦点（聚焦框以白色边框高亮） |
| `a` 或鼠标点击顶部 `Agent: ... [a]` | 打开 Agent 筛选弹窗（全 6 个视图共用，含 `all` 与各 Agent 会话数/用量/费用） |
| `t` 或鼠标点击 `Days │ Hours` | 在 `Overview` 切换柱状图按天 / 按小时展示（KPI 与 Models 仍保持全量） |
| `u` 或鼠标点击底栏 `N unknown [u]` | 打开未知模型列表弹窗（查看未配置价格的 `agent · provider · model`） |
| `↑` / `↓` / `j` / `k` / `PgUp` / `PgDn` / `g` / `G` 或鼠标滚轮 | 滚动当前焦点列表（滚轮支持悬停直接滚动对应子框） |
| `Esc` | 关闭弹窗、从下方子框退回上方主表或退出 TUI |
| `r` | 立即刷新今日增量数据 |
| `q` / `Ctrl+C` | 退出 TUI |

## 单次 CLI 报表

所有单次报表输出为纯空格对齐英文文本（无边框、无 ANSI 颜色，适合管道或复制）；TTY 环境下扫描时在 `stderr` 显示读秒动画，完成后同行擦除。

```bash
# 按小时 / 天 / 周 / 月统计（默认最近 24h / 14d / 12w / 12m，默认合并全部 Agent）
token-counter hourly
token-counter daily
token-counter daily -a dsh -n 30
token-counter weekly -a opencode -n 8
token-counter monthly -a agy -n 6

# 查询单个 Agent 最近会话列表（-a 必选，默认最近 20 条）
token-counter session -a dsh
token-counter session -a opencode -n 10

# 按会话 ID 或唯一前缀查看单个会话的 Turn 轮次明细
token-counter session -a dsh -i 019ab3c4
token-counter session -a agy -i 228ca72a

# 查看帮助
token-counter --help
```

## 指标与计价口径

- **Tokens**：`input + output + reasoning + cache_read + cache_write`。TUI 中 `Out` 合并展示 `output + reasoning`（保证 `In + Out + Cache = Tokens`），全部 token 列统一使用 `M` 单位（保留 3 位小数）便于直观对比。
- **Cache%**：`(cache_read + cache_write) / Tokens`。
- **Cost**：统一按官方美元价（`$USD`）精确结算，**不采信数据源自带 cost、不换汇**。
- **TTFT**：带可靠首包样本请求的平均首 token 延迟（秒）；缺失可靠样本时不造假，显示 `—`。
- **Speed**：带可靠纯解码窗口样本请求的加权平均吞吐 `Σ(output + reasoning) / Σ(duration_ms)`（`tok/s`），剔除 TTFT 与工具执行耗时。
- **PeakCtx**：单个会话或轮次中的单步最大上下文（`input + cache_read + cache_write`）。

### 价目表配置

- 默认内置官方美元价目表位于 [config/models.json](config/models.json)（编译期嵌入二进制），所有模型键名与 `aliases` 均为全小写。
- 如需自定义或补充新模型价格，可创建 `~/.config/token-counter/models.json` 覆盖内置配置；未命中的模型会在报表末尾或 TUI 状态栏提示。

## 文档索引

- [docs/design.md](docs/design.md) — 分层架构、归一化指标口径、两段式刷新机制与新增数据源指南
- [docs/opencode.md](docs/opencode.md) — OpenCode SQLite 表结构、Turn 切分与速度字段调研
- [docs/dsh.md](docs/dsh.md) — DeepSeek Harness v4 事件流、Fork 去重与速度口径调研
- [docs/agy.md](docs/agy.md) — Antigravity SQLite + Protobuf 字段解码与速度口径调研
- [AGENTS.md](AGENTS.md) — 协作约定与代码风格规范
