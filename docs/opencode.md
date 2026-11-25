# opencode 数据源探索

> 实测日期：2026-10-14，本机数据（opencode v2.0.24）。

## 位置与存储形式

> 项目内统一叫 **opencode**（adapter 名、类型名、文档名都用它）。

```
~/.local/share/opencode/opencode.db   ← 用量主库（SQLite，WAL 模式，同目录有 .db-shm / .db-wal）
```

V2 起数据落在单个 SQLite 库里；V1 的 JSON `storage/` 目录本机不存在。**必须只读打开**（opencode 服务可能正在写）：

```
file:~/.local/share/opencode/opencode.db?mode=ro
```

周边目录（均**不是**用量数据源）：

| 路径 | 内容 | 判断 |
|---|---|---|
| `~/.local/share/opencode/log/` | 服务 / 客户端日志 | 无 token 字段 |
| `~/.local/share/opencode/shell/`、`snapshot/`、`repos/` | shell 快照、快照、仓库缓存 | 无 token 字段 |
| `~/.config/opencode/` | `cli.json` / `service.json`（配置、服务地址） | 不读取 |

## 相关表

| 表 | 作用 | 关键字段 |
|---|---|---|
| **`session_message`** | **消息流（token / cost / 速度主来源）** | `id`, `session_id`, `type`, `seq`, `time_created`, `time_updated`, `data` (JSON) |
| `session_v2` | 会话（含会话级汇总） | `id`, `title`, `model`, `tokens_input/output/reasoning/cache_read/cache_write`, `cost` |
| `project` / `workspace` / `worktree` | 项目与工作区 | 无 token 字段 |
| `event` | 事件流 | 本机空表 |
| `account` / `credential` 等 | 账户与凭证 | 不读取 |
| `kv` | 键值缓存（含 `models-dev:catalog` 模型目录） | 取数不用；目录仅作调研（见「模型详细信息存哪」） |

`session_message` 有两个可用索引：`(time_created)` 与 `(session_id, type, seq)`；按时间范围查走前者。

## assistant 消息的 data 结构（主源）

`type='assistant'` 的行 = **一次模型请求**，`data` 携带完整用量，例如：

```json
{
  "time": { "created": 1791941537553, "streamed": 1791941541209, "completed": 1791941541282 },
  "agent": "build",
  "model": { "id": "deepseek-v4.1-flash", "providerID": "opencode-go", "variant": "high" },
  "content": [
    { "type": "reasoning", "text": "...", "time": { "created": 1791941540529, "completed": 1791941541192 } },
    { "type": "text", "text": "..." },
    { "type": "tool", "name": "read", "time": { "created": 1791941540895, "ran": 1791941541200, "completed": 1791941541218 } }
  ],
  "finish": "tool-calls",
  "cost": 0.000925974,
  "tokens": { "input": 5461, "output": 134, "reasoning": 37,
              "cache": { "read": 1408, "write": 0 } }
}
```

- **`tokens.input` 是未命中输入，不含 cache read** —— 计价时**不需要**扣减
- **`cost` 字段是网关美元价，不采信**（见下），费用一律走 `config/models.json` 官方美元价 × token 数
- `type` 取值：`assistant`（用量主源）/ `user`（仅用于切分 Turn 边界）/ `synthetic` / `idle`（后两者无 tokens，过滤即可）
- `model.variant` 是思考强度（`low` / `high` / `max`），不影响计价与分组

## 模型详细信息存哪（会话里没有）

- `session_message.data.model` 与 `session_v2.model` **只有三个字段**：`{ id, providerID, variant }`
  —— 没有厂商、没有 canonical id、没有价格 / 限额 / 上下文窗口
- 详细的模型元数据在 **`kv` 表的 `models-dev:catalog` 键**里（6.3 MB 的 models.dev 全量目录；
  `value` 是 `{ updatedAt, digest, body }`，`body` 是 JSON 字符串）。每个 provider → model 带
  `name`、`family`、`canonical_model_id`、`cost`（**美元**）、`limit`、`modalities`、`reasoning` 等
- **`canonical_model_id` 是跨 provider 的全局身份**：如 `opencode-go/deepseek-v4.1-flash` 的
  `canonical_model_id = "deepseek/deepseek-v4.1-flash"`，与官方 `deepseek` provider 的 `deepseek-flash` 同源
  （二者在 `config/models.json` 里都归一到统一名 `deepseek-v4.1-flash`）
- `opencode-go` / `opencode` 本身是**多模型网关**（catalog 里分别 37 / 122 个模型，
  涵盖 claude / gpt / gemini / deepseek / mimo / glm 等），所以**计价只认 `model.id`，不看 providerID**
- `credential` 表存 provider 凭证（不读取）

## ⚠️ 关键口径

1. **`tokens.input` 不含 `cache.read`**：用 cost 反推验证——对全部带 tokens 的 assistant 消息，
   `cost × 1e6 = input×0.15 + cache.read×0.003 + (output+reasoning)×0.6` **逐行精确成立**（误差 0），
   说明 input 就是未命中价基数、output 与 reasoning 互斥。
2. **`cost` 是网关美元价，不采信**：拟合出的是 opencode-go 网关的美元价（见「模型与价格」），
   按项目硬规则一律用 `config/models.json` 官方美元价结算。
3. **`session_v2` 的会话级汇总与消息求和不完全相等**（实测某会话 `tokens_input` 42679 vs 消息求和 42148，
   `tokens_output` 也略有出入，`tokens_cache_read` 相等），疑为汇总更新路径不同。
   因此 **用量只读 `session_message`**，`session_v2` 仅用于读取会话标题 `title`（`session_name`）与工作目录 `directory` 末级名（`folder`），不参与 token 聚合。
4. **`finish='error'` 或 `data.error` 非空记 aborted**；空 `finish` 是进行中的消息（`streamed` / `completed` 可能缺失）。
5. **`providerID` 是 opencode 的网关名**（如 `opencode-go`），不是模型厂商；计价按 `model.id` **平铺查价**，
   `providerID` 不参与计价，只作分组键（组头 `agent · provider · model`，见「模型与价格」）。
6. **`session_message` 是会话内追加写的活数据**（当前会话本身也在其中），必须只读打开。

## 速度原料

| 指标 | 取法 |
|---|---|
| TTFT | 首个 `reasoning` part 的 `time.created` − 消息 `time.created`（**近似**：含请求构建，库不存流式首包） |
| 生成耗时 | `time.streamed` − 首个 part 的 `time.created`（`streamed` = 流结束时刻，实测 ≈ 末个 reasoning 的 `completed`；纯解码，不含 TTFT 与工具执行） |
| tokens/s | 带时长样本的 `output + reasoning` ÷ 上述纯解码耗时（与其他源同口径） |

- `text` part **没有时间戳**，只有 `reasoning` / `tool` 带时间 → **没有 reasoning 的消息拿不到速度样本**（该行输出 `—`，不编造）
- `time.completed` 在工具执行完之后，**不可**用作生成耗时

## 模型与价格

本机数据（截至 2026-10-17）有以下组合：

| providerID | model.id | 说明 |
|---|---|---|
| `opencode-go` | `deepseek-v4.1-flash` | OpenCode Go 网关代理的 DeepSeek 模型 |
| `opencode` | `deepseek-v4.1-flash-free` | 免费档，归一到 `deepseek-v4.1-flash` |
| `opencode` | `glm-5.3-flash-free` | 免费档，归一到 `glm-5.3-flash` |

- **网关美元价（仅参考，不采信）**：`opencode-go` 的该模型与 catalog 里官方 `deepseek` 条目**同价**
  （$0.15 / $0.6 / $0.003，reasoning 按 output 计；`opencode` 网关为其两倍）
- **模型名归一**：`PricingCatalog::identify` 先统一转小写，先匹配价目表的键、再走 `aliases`；免费档别名显式写在
  对应模型的 `aliases` 里（`deepseek-v4.1-flash-free` 在 DeepSeek 条目、`glm-5.3-flash-free` 在 GLM 条目），
  不做后缀推断，因此二者分别归一到 `deepseek-v4.1-flash`、`glm-5.3-flash`
- **美元结算**：按统一名平铺查 `config/models.json`，统一使用官方美元标准价结算（不采信网关源 cost，免费档与付费档合并展示并按标准价估算）
- 归一后仍查不到价的 model id 不出表格，只在输出末尾一行列出

## 数据覆盖

- 库于本机 **2026-10-14** 首次创建（`migration` 表时间戳即创建时刻），**无历史数据**；查询更早日期会得到空结果
- 只读 WAL 库经 `rusqlite`（`SQLITE_OPEN_READ_ONLY`）实测可正常打开

## 验证过的查询

```sql
-- 各模型用量与源美元 cost（cost 仅参考，费用走 config/models.json 官方价另算）
SELECT json_extract(data,'$.model.providerID')        AS provider,
       json_extract(data,'$.model.id')                AS model,
       count(*)                                       AS requests,
       sum(json_extract(data,'$.tokens.input'))       AS input,
       sum(json_extract(data,'$.tokens.cache.read'))  AS cache_read,
       sum(json_extract(data,'$.tokens.output'))      AS output,
       sum(json_extract(data,'$.cost'))               AS cost_usd
FROM session_message
WHERE type = 'assistant' AND json_extract(data,'$.tokens') IS NOT NULL
GROUP BY 1, 2;

-- 口径校验：cost 是否等于 input×0.15 + cache_read×0.003 + (output+reasoning)×0.6（应返回 0）
SELECT count(*) FROM session_message
WHERE type = 'assistant' AND json_extract(data,'$.tokens') IS NOT NULL
  AND abs(json_extract(data,'$.cost') * 1000000
        - (json_extract(data,'$.tokens.input') * 0.15
         + json_extract(data,'$.tokens.cache.read') * 0.003
         + (json_extract(data,'$.tokens.output') + json_extract(data,'$.tokens.reasoning')) * 0.6)) > 1e-6;
```
