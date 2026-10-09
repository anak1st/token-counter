# token-counter

统计 AI coding agent 的每日 token 用量、费用、速度。TypeScript + Bun，运行时零依赖，**无本地存储**——每次查询直接读数据源。

## 目标

命令行指定数据源和日期范围，输出分组汇总——**一个模型一组，组内每天一行**：

```
token-counter dsh --last 7
token-counter mimo --from 2026-10-01 --to 2026-10-08
```

每行包含：

- **token 用量**：输入 / 输出 / 缓存，及各分项当日占比
- **费用**：按天累计，一律按 `src/pricing.ts` 人民币价 × token 直接计（mimo 源 cost 是美元不采信、不换汇；dsh 源无 cost）
- **性价比**：`Per 1M` 列 = 当日总费用 ÷ 当日总 token × 10⁶（混合有效单价，¥/百万 token）
- **速度**：首 token 延迟（TTFT）、平均输出速度（tokens/s）

输出为对齐的纯文本表格——排版清晰即可，不画装饰线；**文案全英文**（中文双字节不利对齐），费用用人民币 `¥`（格式约定见 [design §6](docs/design.md)）。

## 文档

- **[docs/design.md](docs/design.md)** — 设计文档：数据流、归一化事件、日期/速度口径、无存储的取舍
- **[docs/thinking.md](docs/thinking.md)** — 思考强度（overthinking）专题分析：外部研究、本地可行性、估算实验；**结论暂不实现**
- **[docs/mimo.md](docs/mimo.md)** — mimo 数据源探索：SQLite 表结构、消息样例、注意事项（message/part 双视角勿双计；cost 字段是美元不采信；历史数据仅近期）
- **[docs/dsh.md](docs/dsh.md)** — dsh 数据源探索：zstd JSONL 事件流（**adapter 只解析 v4**，legacy 静默跳过）、会话级聚合缓存 `session_projcache`；desk 桌面版与 CLI 共用 `~/.dsh/sessions/`

## 技术选型

- **TypeScript + Bun 原生运行**，不编译；`bun run typecheck` 做类型检查
- **运行时零依赖**：读 mimo 用内置 `bun:sqlite`（只读）；价格表是带类型的内置常量（[src/pricing.ts](src/pricing.ts)），无配置解析
- 开发期仅 `typescript` + `@types/bun`
- 价格表 [src/pricing.ts](src/pricing.ts)：统一按高峰价；`cache_write` 并入 `input_cache_miss` 计价

## 命令

```bash
bun run typecheck   # tsc --noEmit

# 查询（也可 bun run start -- <args>）
bun src/index.ts dsh --last 7
bun src/index.ts mimo --from 2026-10-01 --to 2026-10-08
bun src/index.ts --help
```

## MVP 范围

1. 参数解析：位置参数选源（`mimo`/`dsh`）+ 日期（`--from/--to`、`--last N`）
2. 被选中的只读 adapter（mimo SQL 按时间过滤；dsh 解压 zstd 按版本解析）产出归一化事件
3. 内存按 (模型, 日期) 聚合 + 计价 → 按模型分组的对齐表格输出（token 分项与占比、费用、TTFT、tokens/s）

## 开工清单

- [ ] 确认 mimo 历史数据的真实位置（当前库仅 2026-10-08 起的 3 个 session）
- [x] 确认 dsh 文件名版本与首行 `session.version` 的对应关系（实测：旧文件名 ↔ version 0，`.v3`/`.v4` ↔ 3/4；以内容 version 字段为准）
- [x] 定归一化事件类型 + 速度口径（mimo TTFT 为 step 级近似，见 design §7）
- [x] 实现 mimo adapter → dsh adapter → 聚合与输出（已用真数据对账：mimo/dsh 新旧格式均与独立脚本一致）
