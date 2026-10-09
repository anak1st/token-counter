# zcode（ZCode）数据源探索

> 实测日期：2026-10-09，本机数据。

## 位置与存储形式

> 项目内统一叫 **zcode**（adapter 名、类型名、文档名都用它）。

```
~/.zcode/cli/db/db.sqlite   ← 用量主库（SQLite，14 MB）
```

**必须只读打开**（ZCode 可能正在写）：

```
file:~/.zcode/cli/db/db.sqlite?mode=ro
```

周边目录（均**不是**用量数据源）：

| 路径 | 内容 | 判断 |
|---|---|---|
| `~/.zcode/cli/rollout/model-io-sess_*.jsonl` | 3 个会话的原始 model-io 流 | 与 db 双视角，忽略 |
| `~/.zcode/cli/exec/`、`artifacts/`、`log/` | shell 快照、产物、日志 | 无 token 字段 |
| `~/.zcode/v2/tasks-index.sqlite` | 任务/自动化索引 | 无 token 字段 |
| `~/.zcode/v2/config.json`、`credentials.json` | 配置与凭证 | 不读取 |
| `~/Library/Application Support/ZCode/` | Electron 壳（Cookies/Cache/leveldb） | 无会话数据，dsh desk 同款情况 |

## 相关表

| 表 | 作用 | 关键字段 |
|---|---|---|
| **`model_usage`** | **每次模型请求一行（token/TTFT 主来源）** | 见下 |
| `turn_usage` | 每 turn 一行的汇总 | 同名 token 列 |
| `tool_usage` | 每次工具调用一行 | 无 token |
| `session` / `message` / `part` | 会话与消息 | 与 mimo 同构，但**不从这里取数** |
| `dwf_*` / `workflow_*` | 工作流引擎 | 实测 0 行，无关 |

`model_usage` 有索引 `(started_at, provider_id, model_id)`，按时间范围查天然走索引。

## `model_usage` 结构（主源）

```sql
CREATE TABLE model_usage (
  id text primary key,
  logical_request_id text not null,
  attempt_index integer default 0,
  session_id text, turn_id text, trace_id text,
  query_source text,            -- main_turn(646) / session_title(15)
  provider_id text,             -- builtin:bigmodel-coding-plan（单一）
  model_id text,                -- GLM-5.3-Flash / GLM-5.3
  variant text,                 -- 思考强度：high(578) / max(68) / disabled(15)
  agent text, mode text,        -- zcode-agent / yolo
  status text,                  -- completed(625) / cancelled(36)
  started_at integer,           -- 毫秒
  first_token_at integer,
  completed_at integer,
  duration_ms integer,
  time_to_first_token_ms integer,   -- ★ 真实流式 TTFT
  input_tokens integer,             -- ★ 含 cache_read（见下）
  output_tokens integer,
  reasoning_tokens integer,         -- 实测全 0
  cache_creation_input_tokens integer, -- 实测全 0
  cache_read_input_tokens integer,
  computed_total_tokens integer,    -- = input + output
  retry_count integer,              -- 实测全 0（无重试双计）
  raw_usage_json text, provider_metadata_json text
);
```

样例行：

```
provider_id=builtin:bigmodel-coding-plan  model_id=GLM-5.3-Flash  status=completed
started_at=1787792480830  first_token_at=1787792485639  completed_at=1787792488111
duration_ms=7281  time_to_first_token_ms=4809
input_tokens=30307  output_tokens=125  cache_read_input_tokens=25856
raw_usage_json={"inputTokens":30307,"outputTokens":125,"totalTokens":30432,
                "cacheReadTokens":25856,"cacheWriteTokens":0}
```

## ⚠️ 关键口径

1. **`input_tokens` 含 `cache_read_input_tokens`，不是未命中价的计费基数**。
   实测（有 anthropic 回执的 15 行全部满足）：`input_tokens = 回执.input_tokens + cache_read_input_tokens`
   （如 `252 = 60 + 192`）。**计价时未命中输入 = `input_tokens - cache_read_input_tokens`**，
   直接拿 `input_tokens` 乘未命中价会把缓存命中部分按全价重算一遍。
   `reasoning_tokens` 与 `cache_creation_input_tokens` 实测全为 0（GLM 不单列 reasoning、缓存写不单独计）。
2. **`turn_usage` 与 `model_usage` 是双视角，只取 `model_usage`**。
   两表汇总不相等（input 62,350,943 vs 62,273,881，turn 略大，疑含子请求口径差异）——
   按 design §4.3 主源原则，**只读 `model_usage`**，turn/tool 表不参与取数。
3. **`status='cancelled'` 的 36 行 usage 全 0**，聚合天然归零；`session_title` 行是标题生成的真实请求，计入。
4. **无重试双计**：`attempt_index > 0` 与 `retry_count > 0` 的行均为 0，661 行 = 661 个逻辑请求。
5. **TTFT 是真实值**（`first_token_at - started_at` 的流式测量，非 step 近似），625 个 completed 行中 492 行有值
   （其余为无流式输出的请求）；与 mimo 的近似 TTFT 不同，表中无需标注。

## 速度原料

| 指标 | 取法 |
|---|---|
| TTFT | `time_to_first_token_ms`（现成，492/625 行有值） |
| 生成耗时 | `duration_ms`（请求级 start→completed，含排队与工具间隙——比 dsh 的 step 口径略宽） |
| tokens/s | 当日 `output_tokens` ÷ 当日 `duration_ms`（聚合口径与其他源一致） |

## 模型与价格

数据中仅两个模型（provider 单一：`builtin:bigmodel-coding-plan`）：

| model_id | 行数 | 说明 |
|---|---|---|
| `GLM-5.3-Flash` | 458 | 轻量款 |
| `GLM-5.3` | 203 | 旗舰款 |

**pricing.ts 需新增 provider（人民币，常规价，per 1M）**：

| 模型 | 未命中输入 | 缓存命中 | 输出 |
|---|---|---|---|
| GLM-5.3 | ¥8 | ¥2 | ¥28 |
| GLM-5.3-Flash | ¥0.8 | ¥0.23 | ¥2.8 |

来源：智谱开放平台定价页（多篇 2026-08/09 报道交叉一致，GLM-5.3 与 GLM-5.2 同价）。两个注意点：

- **本机数据期（08-27 → 09-04）恰逢 GLM-5.3-Flash 限时五折**（输入 ¥0.4 / 输出 ¥1.4 / 命中 ¥0.115，至 2026-09-09）。
  按项目惯例（统一按常规价、不分峰谷），**用常规价计**——算出的费用会约为账单的 2 倍，属预期口径。
- provider 是 **coding plan 订阅套餐**而非按量付费，此处计价是"折算参考值"（与 dsh 按刊例价计同性质）。

## 数据覆盖

- **时间范围**：2026-08-27 09:01 → 2026-09-04 17:14，共 661 行、16 个 session
- 数据止于 09-04 而 db 文件 mtime 为 09-19：该时段后无新用量写入，非查询窗口问题。
  查近期日期（如 `--last 7`）会得到空结果，需用 `--from/--to` 指定上述区间
- 有数据的共 8 天，按天 × 模型分布：

```
08-27  GLM-5.3-Flash   12 req   429,434 in    5,537 out
08-31  GLM-5.3-Flash  253 req 31,717,405 in  261,719 out
09-01  GLM-5.3         18 req 1,162,945 in   42,322 out
09-02  GLM-5.3        175 req 21,416,064 in 220,426 out
09-04  GLM-5.3-Flash   24 req   705,012 in   10,857 out
（08-28/29/30/09-03 有零星或无数据）
```

- 全量汇总：input 62.3M / output 658K / cache_read 61.7M（缓存命中率 ~99%）

## 验证过的查询

```sql
-- 各模型用量（input_tokens 含 cache，未命中 = input - cache_read）
SELECT model_id,
       count(*)                                    AS requests,
       sum(input_tokens - cache_read_input_tokens) AS input_miss,
       sum(cache_read_input_tokens)                AS cache_read,
       sum(output_tokens)                          AS output,
       avg(time_to_first_token_ms)                 AS avg_ttft_ms
FROM model_usage
WHERE status = 'completed'
  AND started_at >= ? AND started_at < ?
GROUP BY model_id;

-- 口径校验：input 是否含 cache（应全为 1）
SELECT count(*) FROM model_usage
WHERE status = 'completed'
  AND json_extract(provider_metadata_json, '$.anthropic.usage.input_tokens')
      + cache_read_input_tokens = input_tokens;
```
