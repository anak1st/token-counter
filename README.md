# token-counter

统计 AI coding agent 的每日 token 用量、费用、速度。TypeScript + Bun，运行时零依赖，**无本地存储**——每次查询直接读数据源。

## 目标

命令行指定数据源和日期范围，输出分组汇总——**一个 (provider, 模型) 一组，组内每天一行**（同一模型来自不同 provider 时分开成组，组头显示 `provider · 模型`）：

```
token-counter dsh --last 7
token-counter mimo --from 2026-10-01 --to 2026-10-08
token-counter zcode --from 2026-08-27 --to 2026-09-05
token-counter opencode --last 7
```

每行包含：

- **token 用量**：输入 / 输出 / 缓存，及各分项当日占比
- **费用**：按天累计，一律按 `src/pricing.ts` 人民币价 × token 直接计（mimo/opencode 源 cost 是美元不采信、不换汇；dsh 源无 cost）
- **性价比**：`Per 1M` 列 = 当日总费用 ÷ 当日总 token × 10⁶（混合有效单价，¥/百万 token）
- **速度**：首 token 延迟（TTFT）、平均输出速度（tokens/s）

输出为对齐的纯文本表格——排版清晰即可，不画装饰线；**文案全英文**（中文双字节不利对齐），费用用人民币 `¥`（格式约定见 [design §6](docs/design.md)）。

未知模型（查不到人民币价）不出表格，只在末尾一行列出 provider/model，便于补价。

## 文档

- **[docs/design.md](docs/design.md)** — 设计文档：数据流、归一化事件、日期/速度口径、无存储的取舍
- **[docs/thinking.md](docs/thinking.md)** — 思考强度（overthinking）专题分析：外部研究、本地可行性、估算实验；**结论暂不实现**
- **[docs/mimo.md](docs/mimo.md)** — mimo 数据源探索：SQLite 表结构、消息样例、注意事项（message/part 双视角勿双计；cost 字段是美元不采信；历史数据仅近期）
- **[docs/dsh.md](docs/dsh.md)** — dsh 数据源探索：zstd JSONL 事件流（**adapter 只解析 v4**，legacy 静默跳过）、会话级聚合缓存 `session_projcache`；desk 桌面版与 CLI 共用 `~/.dsh/sessions/`
- **[docs/zcode.md](docs/zcode.md)** — zcode 数据源探索：`~/.zcode/cli/db/db.sqlite` 的 `model_usage` 表（请求粒度、真实 TTFT；**`input_tokens` 含 cache_read，计价需扣减**；与 `turn_usage` 双视角只取前者）
- **[docs/opencode.md](docs/opencode.md)** — opencode 数据源探索：`~/.local/share/opencode/opencode.db` 的 `session_message` 表（请求粒度；`tokens.input` 不含 cache read；第三方源不兜底）；详细模型信息在 `kv` 的 models.dev 目录，会话本身只有 model id

## 技术选型

- **TypeScript + Bun 原生运行**，不编译；`bun run typecheck` 做类型检查
- **运行时零依赖**：读 mimo/zcode/opencode 用内置 `bun:sqlite`（只读）；价格表是带类型的内置常量（[src/pricing.ts](src/pricing.ts)），无配置解析
- 开发期仅 `typescript` + `@types/bun`
- 价格表 [src/pricing.ts](src/pricing.ts)：按模型名平铺查价（厂商仅作价目表分组）；统一按高峰价（大部分在上班时间使用）；`cache_write` 并入 `input_cache_miss` 计价；定价依据与折扣期说明见各数据源文档

## 命令

```bash
bun run typecheck   # tsc --noEmit

# 查询（也可 bun run start -- <args>）
bun src/index.ts dsh --last 7
bun src/index.ts mimo --from 2026-10-01 --to 2026-10-08
bun src/index.ts opencode --last 7
bun src/index.ts --help
```

## MVP 范围

1. 参数解析：位置参数选源（`mimo`/`dsh`/`zcode`/`opencode`）+ 日期（`--from/--to`、`--last N`）
2. 被选中的只读 adapter（mimo/zcode/opencode 走 SQL 按时间过滤；dsh 解压 zstd 按版本解析）产出归一化事件
3. 内存按 (provider, 模型, 日期) 聚合 + 计价 → 按 (provider, 模型) 分组的对齐表格输出（token 分项与占比、费用、TTFT、tokens/s）

## 待办

- [ ] 确认 mimo 历史数据的真实位置（当前库仅 2026-10-08 起的 3 个 session）
