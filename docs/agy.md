# agy（Antigravity）数据源探索

> 实测日期：2026-11-22，本机数据。项目内统一称 **agy**（adapter、类型、CLI 参数名与文档保持一致）。

## 位置与存储形式

```text
~/.gemini/antigravity/conversations/<conversation-id>.db   ← 用量主库（SQLite，WAL 模式）
```

每个会话（含子代理独立会话）对应一个独立的 SQLite 库文件：

- **活跃会话**（当前正在运行）：同目录下存在 `.db-shm` 与非空 `.db-wal`，**必须只读打开**（`rusqlite` 以 `SQLITE_OPEN_READ_ONLY` 打开），才能读到尚未 checkpoint 的 `-wal` 增量。
- **已关闭会话**（Antigravity 退出/切换后 cleanly closed）：`-shm` 与 `-wal` 已合并清除，但库头仍为 WAL 模式（writer/read version = 2）。无 `-shm` 时 SQLite 普通只读打开会报 `SQLITE_CANTOPEN`（errno 14），因此当 `-wal` 不存在或为空时，需以 `file:<path>?immutable=1` 打开。

周边目录与文件（均**不是**用量主源）：

| 路径 | 内容 | 判断 |
|---|---|---|
| `~/.gemini/antigravity/brain/<id>/.system_generated/logs/transcript.jsonl` | 会话可读 JSONL 日志（`PLANNER_RESPONSE` 含 `model`、`input_tokens`、`cache_read_tokens`、`output_tokens`、`created_at`） | 与主库双视角，且**缺少 `reasoning` 拆分与亚秒级速度时间戳**，不取数 |
| `~/.gemini/antigravity/conversation_summaries.db` | 会话列表与 FTS5 搜索索引 | 无 token 字段 |
| `~/.gemini/antigravity/annotations/*.pbtxt` | 标题与视图时间戳 | 无 token 字段 |
| `~/Library/Application Support/Antigravity/` | Electron 壳缓存（Cookies/Cache/Preferences） | 无会话 token 数据 |

## 相关表

每个 `<conversation-id>.db` 结构相同：

| 表 | 作用 | 关键字段 |
|---|---|---|
| **`steps`** | **按步记录执行流（时间戳、状态、token 原料）** | `idx`, `step_type`, `status`, `metadata` (protobuf) |
| **`gen_metadata`** | **每次模型生成的元数据（模型 ID、TTFT/生成耗时、token）** | `idx`, `data` (protobuf) |
| `trajectory_meta` | 会话标识 | `trajectory_id`, `cascade_id` |
| `executor_metadata` / `parent_references` / `battle_mode_infos` | 执行器与辅助信息 | 取数不用 |

`steps` 中 `step_type = 15` 对应一次模型响应（`PLANNER_RESPONSE`），与 `gen_metadata` 一一对应（`14` 为用户输入，`132` 为工具调用/结果等通用步骤）。

## Protobuf 关键字段（实测 wire 解码）

### `steps.metadata`（`step_type = 15`）

| Field | 类型 | 含义 | 说明 |
|---|---|---|---|
| `1` | Timestamp `(f1=seconds, f2=nanos)` | 步骤创建时刻 | 作为事件时间戳 `ts`（毫秒） |
| `6` | Timestamp | 首包/流开始时刻 | 与 `f1` 差值等于 `gen_metadata` 的 `f1.11` |
| `7` | Timestamp | 流结束时刻 | 与 `f6` 差值等于 `gen_metadata` 的 `f1.12` |
| `9` | Message (Usage) | token 统计 | 见下表 |
| `12` | string | turn / 请求组 UUID | — |

`steps.metadata.f9`（以及 `gen_metadata.f1.4`）中的 token 字段：

| Field | 含义 | 口径验证 |
|---|---|---|
| `f2` | `input`（未命中缓存输入） | **不含 `cache_read`**（见下） |
| `f3` | 总 `output`（含思考） | 实测恒等于 `f9 + f10` |
| `f5` | `cache_read` | 0 时省略 |
| `f9` | `reasoning`（thinking token） | 与 `f10` 互斥 |
| `f10` | 非思考输出（正文 + 工具调用） | `f9 + f10 === f3` 逐条成立 |

### `gen_metadata.data`

外层 `f1`（Message）内含：

| Field | 类型 | 含义 | 样例 |
|---|---|---|---|
| `f4` | Message | 同 `steps.metadata.f9` 的 token 统计 | — |
| `f11` | Duration `(f1=seconds, f2=nanos)` | TTFT（与 `steps` 的 `f6 - f1` 一致） | `3.215s` |
| `f12` | Duration `(f1=seconds, f2=nanos)` | 生成耗时（与 `steps` 的 `f7 - f6` 一致） | `1.101s` |
| `f19` | string | 统一模型 ID | `"gemini-4-hydrogen"` |
| `f20` | repeated `(key, value)` | 元数据键值，含 `last_step_index`、`cascade_id`、`request_id` | `"last_step_index" -> "0"` |
| `f21` | string | 展示名（含思考档位） | `"Gemini 4 Hydrogen (High)"` |

`gen_metadata` 通过 `Number(last_step_index) + 1 === steps.idx` 与 `steps` 精确关联（若个别行缺失 `last_step_index`，退回按 `step_type = 15` 序号对齐）。

## ⚠️ 关键口径

1. **`input` 不含 `cache_read`，计价无需扣减**。
   实测连续步骤（会话 `228ca72a`）：
   - step 1：`input=16510, cache_read=0, out=438`
   - step 7：`input=7593, cache_read=16511`（`16510` 落入缓存，新增 `7593`）
   - step 13：`input=11270, cache_read=24105`（`16511 + 7593 = 24104` 落入缓存）
   - step 15：`input=3240, cache_read=35376`（`24105 + 11270 = 35375` 落入缓存）
   说明 `f2` 是精确的未命中输入基数。
2. **`f9` 与 `f10` 精确拆分 `reasoning` 与 `output`**。
   在 `228ca72a`（4 步）、`7ddb2cc5`（1 步）、`4c0d6b82`（33 步）全部样本中，`f9 + f10 === f3` 100% 成立；且与 `transcript.jsonl` 的 `thinking` / `tool_calls` 长度分布一致（长思考步 `f9` 占比高，无思考工具调用步 `f9` 极小）。若将来遇到不提供 `f9/f10` 的非推理模型，退回 `reasoning = 0, output = f3`。
3. **不与 `transcript.jsonl` 双计**：只读 `conversations/*.db`，`brain/` 下的 `transcript.jsonl` 不参与取数。
4. **文件 mtime 剪枝**：活跃库增量写在 `.db-wal` 中，`.db` 本体 mtime 可能滞后；按区间起点剪枝时需取 `max(mtime(.db), mtime(.db-wal))`。
5. **`status !== 3` 记为 `aborted`**（Antigravity 中 `3` 表示 `DONE`；若某未完成步零用量则直接跳过）。
6. **空 Protobuf Timestamp / Duration 不造假**：当 `f11` / `f12` 或 `f6` / `f7` 为空 submessage（全零）时返回 `None`，不记作 `0ms` TTFT。
7. **Turn 归属**：`steps.metadata.f12` 作为 `UsageEvent.turn`，用于会话级 Turn 聚合。

## 速度原料

| 指标 | 取法 | 说明 |
|---|---|---|
| TTFT | `gen_metadata.f1.11`（退回 `steps` 的 `f6 - f1`） | 毫秒精度 |
| 生成耗时 | `gen_metadata.f1.12`（退回 `steps` 的 `f7 - f6`） | 纯流式窗口；注：大段 `tool_call` 集中吐出时该值可能偏短（实测有 32ms 样本） |
| tokens/s | 当日带时长样本的 `(output + reasoning) ÷ genMs` | 与其他源同口径 |

## 模型与价格

本机数据与 Antigravity / Google Gemini API 常见模型（`provider` 统一记为 `google`，配置在 `config/models.json`，统一以官方美元价结算）：

| 模型 ID | 常规美元价（in miss / hit / out per 1M tok） | 备注 |
|---|---|---|
| `gemini-4-hydrogen` | `$1.50 / $0.075 / $7.50` | 含 `gemini-4-hydrogen-tiered-high` 等别名 |
| `gemini-4-neon` | `$3.00 / $0.15 / $15.00` | 本机实测已出现（`(Medium)`） |
| `gemini-4-argon` | `$4.00 / $0.20 / $20.00` | 2026-11-30 前首发价 `$2.00 / $0.10 / $10.00`，按项目惯例取常规价 |
| `gemini-4-carbon` | `$4.00 / $0.20 / $20.00` | 2026-11-30 前首发价 `$2.00 / $0.10 / $10.00`，按项目惯例取常规价 |
| `gemini-3.8-flash` | `$1.50 / $0.15 / $7.50` | 2026-12-31 前首发价 `$0.75 / $0.075 / $3.75`，按项目惯例取常规价；含 `gemini-3.7-flash` 别名 |
| `gemini-3.5-flash-lite` | `$0.30 / $0.03 / $2.50` | 轻量低延迟档 |
| `gemini-3.1-pro-preview` | `$2.00 / $0.20 / $12.00` | `<=200k` 上下文常规价 |

- 思考档位后缀名（如 `-low` / `-medium` / `-high` / `-tiered-high` 与展示名）均逐个显式列在 `config/models.json` 的 `aliases` 中，全部小写归一到对应统一模型名。

## 数据覆盖

- 本机现有 6 个会话库，始于 **2026-11-22**，已实测覆盖 `gemini-4-hydrogen` 与 `gemini-4-neon`。
- `has_subtrajectory` 实测均为 0；若后续产生子代理，Antigravity 会为子代理分配独立 `conversation_id` 与 `.db` 文件，遍历 `conversations/*.db` 可自然覆盖且不重计。

## 验证过的查询

```bash
# 查看活跃库与已关闭库的 step 统计（活跃库可普通只读，已关闭无 -shm/-wal 需 immutable=1）
sqlite3 "file:$HOME/.gemini/antigravity/conversations/228ca72a-72e4-466f-890b-429717aa60ca.db?immutable=1" \
  "SELECT count(*) AS total, sum(CASE WHEN step_type=15 THEN 1 ELSE 0 END) AS planner FROM steps;"
```
