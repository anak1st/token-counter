# token-counter

统计 AI coding agent 的每日 token 用量、费用、速度。TypeScript + Bun，核心统计零运行时依赖（CLI 层使用 commander + chalk），**无本地存储**——每次查询直接读数据源。

## 目标

命令行指定 agent 和日期范围，先输出合并所有 provider / 模型的每日总计，再按 (provider, 统一模型名) 分组，组内每天一行（同一模型来自不同 provider 时分开成组，组头显示 `provider · 模型`）：

```
token-counter daily --agent dsh --last 7
token-counter daily --agent mimo --start 2026-10-01 --end 2026-10-08
token-counter daily --agent zcode --start 2026-08-27 --end 2026-09-05
token-counter daily --agent opencode --last 7
```

`hourly` 和 `summary` 已预留命令位，暂未实现；它们只接受 `--agent`，不接受日期范围。

输出包含：

- **总计**：最前面一张表，合并所有有价 provider / 模型，回答整体用量与费用
- **token 用量**：合并为一列 `Tokens = input + output + reasoning + cache_read + cache_write`
- **Chart**：Tokens 后的柱状图；所有表共用同一最大日 token 值，最大值填满整条，宽度随终端 8–30 列自适应
- **缓存占比**：`Cache%` = 缓存 token ÷ 当日总 token
- **费用**：按天累计，一律按 `src/pricing.ts` 人民币价 × token 直接计（mimo/opencode 源 cost 是美元不采信、不换汇；dsh 源无 cost）
- **速度**：首 token 延迟（TTFT）、平均输出速度（tokens/s）
- **不再输出 `Per 1M`**：它留给后续更细的价格明细视角

输出为对齐的纯文本表格——先总计、后分组，总计表与分组表共用一套列宽，日期统一 `YYYY-MM-DD`，排版清晰即可，不画装饰线；**文案全英文**（中文双字节不利对齐），费用用人民币 `¥`（格式约定见 [design §6](docs/design.md)）。

未知模型（查不到人民币价）不出表格，只在末尾一行列出 provider/model，便于补价。

模型名在入口统一：价目表的键就是统一名，历史名 / 渠道名 / 免费档都逐个显式写在 `aliases` 里（如 `deepseek-flash`、`deepseek-v4.1-flash-free` → `deepseek-v4.1-flash`），分组与展示不再出现同一模型的多种名字。

## 文档

- **[docs/design.md](docs/design.md)** — 设计文档：数据流、归一化事件、日期/速度口径、无存储的取舍
- **[docs/output.md](docs/output.md)** — CLI 输出设计：总计表、简化分组表、token 合并口径与后续图表计划（**当前实现，无图表**）
- **[docs/thinking.md](docs/thinking.md)** — 思考强度（overthinking）专题分析：外部研究、本地可行性、估算实验；**结论暂不实现**
- **[docs/mimo.md](docs/mimo.md)** — mimo 数据源探索：SQLite 表结构、消息样例、注意事项（message/part 双视角勿双计；cost 字段是美元不采信；历史数据仅近期）
- **[docs/dsh.md](docs/dsh.md)** — dsh 数据源探索：zstd JSONL 事件流（**adapter 只解析 v4**，legacy 静默跳过）、会话级聚合缓存 `session_projcache`；desk 桌面版与 CLI 共用 `~/.dsh/sessions/`
- **[docs/zcode.md](docs/zcode.md)** — zcode 数据源探索：`~/.zcode/cli/db/db.sqlite` 的 `model_usage` 表（请求粒度、真实 TTFT；**`input_tokens` 含 cache_read，计价需扣减**；与 `turn_usage` 双视角只取前者）
- **[docs/opencode.md](docs/opencode.md)** — opencode 数据源探索：`~/.local/share/opencode/opencode.db` 的 `session_message` 表（请求粒度；`tokens.input` 不含 cache read；第三方源不兜底）；详细模型信息在 `kv` 的 models.dev 目录，会话本身只有 model id

## 技术选型

- **TypeScript + Bun 原生运行**，不编译；`bun run typecheck` 做类型检查
- **CLI 层**：commander 管子命令、参数与帮助，chalk 管颜色；核心统计（adapter / 聚合 / 计价）不依赖第三方
- **数据访问零依赖**：读 mimo/zcode/opencode 用内置 `bun:sqlite`（只读）；价格表是带类型的内置常量（[src/pricing.ts](src/pricing.ts)），无配置解析
- 运行时依赖只有 `commander` + `chalk`；开发期 `typescript` + `@types/bun`
- 价格表 [src/pricing.ts](src/pricing.ts)：价目表的键即统一模型名，`aliases` 显式收历史名 / 渠道名 / 免费档；按统一名平铺查价（厂商仅作价目表分组）；统一按高峰价（大部分在上班时间使用）；`cache_write` 并入 `input_cache_miss` 计价；定价依据与折扣期说明见各数据源文档

## 命令

```bash
bun run typecheck   # tsc --noEmit

# 查询（也可 bun run start -- <args>）
bun src/index.ts daily --agent dsh --last 7
bun src/index.ts daily --agent mimo --start 2026-10-01 --end 2026-10-08
bun src/index.ts daily --agent opencode --last 7
bun src/index.ts hourly --agent dsh       # 已预留，暂未实现
bun src/index.ts summary --agent dsh      # 已预留，暂未实现
bun src/index.ts --help
```

`--color auto|always|never` 控制颜色，默认 `auto`：只有 TTY 上色，管道输出自动无色；`always` / `never` 可显式覆盖。

## MVP 范围

1. 参数解析：子命令 `daily` / `hourly` / `summary`；`--agent` 必填；`daily` 日期二选一——`--last N` 或 `--start/--end`（`--end` 含当天）；`hourly` / `summary` 不接受日期范围
2. 被选中的只读 adapter（mimo/zcode/opencode 走 SQL 按时间过滤；dsh 解压 zstd 按版本解析）产出归一化事件
3. 内存按 (provider, 统一模型名, 日期) 聚合 + 计价 → 先折出跨分组合计，再按 (provider, 统一模型名) 分组输出简化表格（总 token、费用、缓存占比、TTFT、tokens/s）

## 待办

- [ ] 确认 mimo 历史数据的真实位置（当前库仅 2026-10-08 起的 3 个 session）
