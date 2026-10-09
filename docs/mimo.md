# mimo 数据源探索

> 实测日期：2026-10-09，本机数据。所有查询用只读模式验证过。

## 位置与存储形式

> 项目内统一叫 **mimo**（adapter 名、类型名、文档名都用它）；磁盘路径和数据本身仍叫 `mimocode`，不改。

`~/.local/share/mimocode/mimocode.db` — SQLite（WAL 模式，同目录有 `.db-shm` / `.db-wal`）。

**必须只读打开**，不要锁住 mimo 自己的写入：

```
file:~/.local/share/mimocode/mimocode.db?mode=ro
```

## 相关表

| 表 | 作用 | 关键字段 |
|---|---|---|
| `project` | 项目注册 | `id`, `worktree`（项目目录）, `name` |
| `session` | 会话 | `id`, `project_id`, `directory`, `title`, `time_created/updated`（毫秒） |
| `message` | 消息（**token/cost 的主来源**） | `id`, `session_id`, `time_created`, `data` (JSON) |
| `part` | 消息内的分片/步骤事件 | `message_id`, `session_id`, `time_created`, `data` (JSON) |

`message` 有索引 `(session_id, time_created, id)`，按会话扫很顺。

## assistant 消息的 data 结构

`json_extract(data, '$.role') = 'assistant'` 的消息携带完整用量：

```json
{
  "role": "assistant",
  "cost": 0.000960848,
  "tokens": {
    "total": 24690, "input": 5768, "output": 157, "reasoning": 205,
    "cache": { "read": 18560, "write": 0 }
  },
  "modelID": "mimo-v2.6-flash",
  "providerID": "xiaomi",
  "time": { "created": 1791451000264, "completed": 1791451021655 },
  "finish": "tool-calls"
}
```

- **`cost` 字段是美元，不采信**（见下方「cost 字段实测」），RMB 费用一律走 `src/pricing.ts` 人民币价 × token 数
- `time.created → time.completed` 可算单步耗时；`output ÷ 耗时` 即 tokens/s
- **没有首 token 延迟**：库里不存流式 chunk，可用 `part` 表的 `step-start` → 对应 `step-finish` 时间差做近似

## cost 字段实测：是美元，不是人民币

对库内 127 条带 cost 的 assistant 消息做最小二乘拟合（`cost = a·input + b·output + c·reasoning + d·cache_read`，单位 per token），结果**精确拟合**（最大相对误差 ~1e-15）：

| 分项 | 拟合单价 | 官方价对照 |
|---|---|---|
| input（未命中） | **$0.14 / M** | 官方海外价 $0.14 ✅ |
| output | **$0.28 / M** | 官方海外价 $0.28 ✅ |
| reasoning | $0.28 / M | 按 output 价计 |
| cache read | $0.0028 / M | 恰为 input 的 2% |

而小米官方**大陆人民币价**是：输入 **¥1**、输出 **¥2**、缓存命中 **¥0.02**（0.14 × ~7.1 ≈ 1，比值吻合汇率）。拟合值与人民币价数量级对不上 → **cost 存的是官方海外美元价**。

**结论**：输出只要 RMB，所以**不采信 cost 字段，也不做汇率换算**——直接用 pricing.ts 里的人民币单价 × token 数计算，和大陆官方计价口径精准一致。cache_write 按未命中输入价计（源数据 write 全为 0，该口径未实测）。

## part 表的 step-finish 事件

```json
{"type":"step-finish","reason":"tool-calls",
 "tokens":{"total":24690,"input":5768,"output":157,"reasoning":205,"cache":{"write":0,"read":18560}},
 "cost":0.000960848}
```

part 类型分布（当前库）：`tool` 48、`step-start` 44、`reasoning` 44、`step-finish` 43、`text` 22。

## ⚠️ 关键注意事项

1. **message 与 part 不要双计**。实测两者 cost / input / output 总和**完全相同**（各 $0.01819、61259 in、11760 out），即每个 assistant 消息对应一个 step-finish，是同一份数据的两个视角。**adapter 只取 `message` 表即可**（它额外带 modelID 和时间戳）。
2. **历史数据疑似只有近期**。当前库：3 个 session、53 条 message、1 个 project，最早数据 2026-10-08（即调研前一天）；`claude_import`、`external_import` 表均为空。老数据可能被清过或另有存储——写代码前可再确认一次。
3. 用户消息（`role: user`）没有 tokens/cost，统计时过滤即可。
4. 同一 `parentID` 下会有多条 assistant 消息（每个 step 一条），**每条都是独立的一次计费**，直接求和。

## 验证过的查询

```sql
-- 各模型用量（cost 列是美元，仅作参考；RMB 费用走 pricing.ts 另算）
SELECT json_extract(data,'$.modelID') AS model,
       sum(json_extract(data,'$.cost')) AS cost_usd,
       sum(json_extract(data,'$.tokens.input')) AS input,
       sum(json_extract(data,'$.tokens.output')) AS output
FROM message
WHERE json_extract(data,'$.role') = 'assistant'
GROUP BY model;
```
