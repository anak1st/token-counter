# 设计说明

`token-counter` 是一个用 **Rust + Ratatui** 编写的本地 AI 编程 Agent 用量、费用与速度分析工具。核心原则：**无本地存储、只读直扫、精确计价、速度指标宁缺毋滥**。

## 1. 数据流与分层架构

```text
本地数据源（只读）
  ├── ~/.local/share/opencode/opencode.db          (SQLite WAL)
  ├── ~/.dsh/sessions/**/session.v4.jsonl.zstd     (zstd JSONL，进程内并行解压)
  └── ~/.gemini/antigravity/conversations/*.db     (SQLite + Protobuf)
          │
          ▼  src/adapters/（统一 ScanQuery 剪枝）
     Vec<UsageEvent>  ← 入口即完成模型名归一 + 官方美元计价（src/core/pricing.rs）
          │
          ├──► src/report/aggregate.rs + render.rs  ──► 单次 CLI 纯文本报表
          ├──► src/report/session.rs                ──► 单次 CLI 会话 / Turn 查询
          └──► src/tui/                             ──► Ratatui 交互式 TUI 面板
```

### 目录结构

```text
config/
└── models.json               # 内嵌官方美元价目表与别名映射（全小写）
docs/
├── design.md                 # 架构与统计口径说明（本文档）
├── opencode.md               # opencode 数据源调研
├── dsh.md                    # dsh 数据源调研（仅 v4）
└── agy.md                    # agy（Antigravity）数据源调研
src/
├── main.rs                   # 薄入口：CLI 解析分发 / 启动 TUI
├── lib.rs                    # 核心库导出（core / adapters / report）
├── core/
│   ├── mod.rs                # 模块索引
│   ├── model.rs              # Agent 枚举、Usage 与归一化 UsageEvent
│   ├── pricing.rs            # config/models.json 加载、全小写校验、USD 计价
│   ├── time.rs               # jiff 本地时区时间窗与 Hour/Day/Week/Month 分桶
│   └── format.rs             # CLI 与 TUI 共用的数值、费用、耗时、路径与 CJK 截断格式化
├── adapters/
│   ├── mod.rs                # 模块索引与公共导出
│   ├── scan.rs               # ScanQuery 与 std::thread::scope 多 Agent 并行分发
│   ├── opencode/
│   │   ├── mod.rs            # 模块索引
│   │   ├── decode.rs         # session_message.data 反序列化与事件构造
│   │   └── scan.rs           # opencode SQLite 只读查询与 session_v2 元信息加载
│   ├── dsh/
│   │   ├── mod.rs            # 模块索引
│   │   ├── decode.rs         # dsh v4 字节预筛与强类型 JSONL 解码
│   │   ├── parser.rs         # 单会话纯状态机（路由、fork 去重、标题与中止回填）
│   │   └── scan.rs           # 候选文件剪枝与 thread::scope 多核并行解压
│   └── agy/
│       ├── mod.rs            # 模块索引
│       ├── proto.rs          # 零依赖 Protobuf wire 解码器
│       └── scan.rs           # agy SQLite（WAL / immutable）与摘要库元信息扫描
├── report/
│   ├── mod.rs                # 模块索引
│   ├── aggregate.rs          # 按 (agent, provider, model, bucket) 聚合
│   ├── render.rs             # 纯空格对齐时间维度报表（无边框、无 ANSI 颜色）
│   ├── session.rs            # 会话列表、跨 Agent 前缀匹配与 Turn 级聚合（CLI/TUI 共用）
│   └── session_render.rs     # 单次 CLI 会话列表与 Turn 明细纯文本渲染
├── cli/
│   ├── mod.rs                # 模块索引
│   ├── args.rs               # Clap 子命令定义（全英文 help，统一 AgentArg 默认 all）
│   └── spinner.rs            # 零依赖 TTY 读秒加载动画（完成后同行覆盖）
└── tui/
    ├── mod.rs                # 模块索引
    ├── runner.rs             # 终端生命周期（含鼠标捕获）与梯度 60s 后台刷新线程
    ├── data.rs               # TuiSnapshot 多视角预聚合（All + 按 Agent）
    ├── app.rs                # 6 个 Tab 状态、全局筛选、双表焦点与滚动裁剪
    ├── input.rs              # 键盘与鼠标点击/悬停滚轮事件分发
    └── ui/
        ├── mod.rs            # 模块索引
        ├── shell.rs          # 三段式顶层布局、顶栏框内 Agent 按钮与底栏状态
        ├── style.rs          # 圆角边框、聚焦高亮、模型配色、滚动条与 1 位有效数字 Y 轴
        ├── modals.rs         # 居中弹窗（Agent 筛选弹窗 + 未知模型弹窗）
        ├── overview.rs       # 精简全量 Summary + 可横向滚动对称柱状图 + 窗口联动 Models 面板
        ├── period.rs         # Hour / Day / Week / Month 通用上下联动双表
        ├── models_panel.rs   # 统一 Models 面板与跨表共享的右侧数值列宽
        ├── session.rs        # 会话列表（ID/Name/Folder 分列、Turns）+ 逐个 Turns 明细
        └── tests.rs          # TUI 渲染与交互集成测试
```

## 2. 归一化事件与指标口径

所有 adapter 在读取边界将数据归一化为 `UsageEvent`，下游统计层不关心具体来源格式。

### Token 分项不变量

- `input`：仅包含**未命中缓存**的输入 token（源若合并了 `cache_read`，adapter 入口必须扣减）。
- `output` 与 `reasoning` 互斥（源若为包含关系，adapter 入口必须拆分或全留 `output`）。
- `Tokens = input + output + reasoning + cache_read + cache_write`。
- `Cache% = (cache_read + cache_write) / Tokens × 100%`。
- TUI 展示层将 `Out` 定义为 `output + reasoning`，不再单独占一列 `Reason`，保证 `In + Out + Cache == Tokens`，且全部 token 列统一按 `M` 单位保留 3 位小数对齐。

### 计价口径

- 价目表定义在 `config/models.json`（编译期嵌入二进制），支持用户通过 `~/.config/token-counter/models.json` 覆盖。
- 启动时严格校验：所有模型主键与 `aliases` 必须全小写且无冲突。
- **不采信任何数据源自带的 `cost` 字段，不换汇**，统一按官方美元标准价计算：
  - `input` 与 `cache_write` 按 `input_cache_miss` 计价；
  - `cache_read` 按 `input_cache_hit` 计价；
  - `output + reasoning` 按 `output` 计价。
- 查不到定价的模型记入 `unknown_models`，不参与费用汇总并在界面提示，便于补充配置。

### 速度与上下文指标

- **TTFT**（首 token 延迟）：仅聚合带可靠首包样本的请求均值（秒）。无可靠样本时留空（`—`），绝不用总耗时兜底。
- **Speed**（纯解码吞吐，`tok/s`）：按有可靠纯解码窗口样本的请求汇总 `Σ(output + reasoning) / Σ(duration_ms) × 1000`，剔除 TTFT 与工具执行耗时。
- **PeakCtx**（会话/轮次峰值上下文）：单步最大 `input + cache_read + cache_write`，用于直观衡量上下文膨胀程度。

### 本地时区分桶

所有时间范围均采用本地时区半开区间 `[start_ms, end_ms)`：

- `Hourly`：`YYYY-MM-DD HH:00`
- `Daily`：`YYYY-MM-DD`
- `Weekly`：ISO 周 `YYYY-Www`（周一为一周起点）
- `Monthly`：`YYYY-MM`

## 3. TUI 梯度加载与当日增量刷新

TUI 坚持**零磁盘缓存**，同时通过内存梯度加载兼顾首屏速度、完整历史与极速增量刷新：

1. 启动时固定两个本地 `00:00` 切分点：`initial_cutoff_ms`（30 天前 `00:00`）与 `refresh_cutoff_ms`（今日 `00:00`）；
2. **阶段 1（首屏 30 天）**：先扫 `[initial_cutoff_ms, ∞)`（通常百毫秒级），立即推送 `is_partial = true` 快照点亮首屏与 30 天柱状图；
3. **阶段 2（历史补全）**：后台静默补扫 `[0, initial_cutoff_ms)` 一次并合并推送全量快照，随后将 `ts_ms < refresh_cutoff_ms` 的所有历史事件冻结驻留 worker 线程内存；
4. **阶段 3（60s 自动刷新 / 手动 `r`）**：仅重扫今日 `[refresh_cutoff_ms, ∞)`（1 天窗口）再与内存冻结历史合并，既极快又不重计、不漏计。

## 4. 新增数据源步骤

1. 先调研目标 Agent 的本地存储路径、字段含义与去重口径，编写 `docs/<source>.md`；
2. 在 `src/core/model.rs` 的 `Agent` 枚举中新增成员（TUI 的 `Session` 切换按钮与全量扫描会自动感知）；
3. 在 `src/adapters/<source>/` 实现只读扫描器并在 `src/adapters/mod.rs` 与 `src/adapters/scan.rs` 注册；
4. 在 `src/cli/args.rs` 的 `AgentArg` 补上 CLI 参数值（所有子命令共用）；
5. 如有新模型，在 `config/models.json` 中补充全小写定价条目与别名。
