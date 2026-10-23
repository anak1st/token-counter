# dsh（DeepSeek Harness）数据源探索

> 实测日期：2026-10-09，本机数据。zstd 解压用系统 `zstd -dc`（Bun 1.4 尚无内置 zstd API）。

## 位置与存储形式

```
~/.dsh/sessions/<project-dir>/session-<uuid>/session[.v3|.v4].jsonl.zstd
```

- **project-dir**：项目路径编码，如 `--Users-you-Code-my-project--`（`/` → `-`）
- **session 目录**：新旧命名混用——旧的叫 `session-<uuid>`，新的直接是 `<uuid>`
- **zstd 压缩的 JSONL 事件流**，每行一个 JSON 事件

三种格式文件并存（旧格式 / `.v3.` / `.v4.`）+ 0 字节 `session.lock`（标记会话目录）。同一目录出现两份 zstd 的情况罕见但存在。

> `~/.dsh/settings.yaml.imported` 里 `agent-default-model: deepseek-official / deepseek-flash`。

### dsh desk（桌面版）共用同一份数据

桌面版 `DeepSeek Harness.app` **不写自己的会话目录**——进程启动参数直接用 profile `~/.dsh/profiles/desktop`，会话仍落在 `~/.dsh/sessions/`；`~/Library/Application Support/@deepseek-ai/dsh-desktop/` 里只有纯 Electron 样板（Cache/Cookies/Local Storage），没有会话数据。**adapter 只需覆盖 `~/.dsh/sessions/`，desk 与 CLI 都会统计到。**

## 聚合缓存：`session_projcache`（推荐优先读）

```
~/.dsh/storages/session_projcache/sessions/<session-id>.json
```

dsh 自己维护的**会话级聚合缓存**，与会话目录一一对应（本机 76/76）。结构（`version: 7`）：

```json
{
  "record": {
    "identity": { "formatVersion": 4, "createdAt": 1791420849846, "cwd": "~/Code/my-project" },
    "rows": {
      "tokenUsage":   { "seq": 7405, "val": { "totals": { "uncachedInputTokens": 767074, "outputTokens": 1228257,
                                                          "cacheReadTokens": 307473664, "cacheWriteTokens": 0 } } },
      "sessionStats": { "val": { "turns":117, "steps":1062, "llmMs":7222722, "toolMs":1377201,
                                 "ttftMs":2131668, "ttftSteps":1057, "decodeMs":4933757, "decodeTokens":1228257 } },
      "modelSelection": { "val": { "lastUsed": { "provider":"deepseek-official", "model":"deepseek-flash", "reasoningEffort":"high" } } }
    }
  }
}
```

优点：

- **现成的会话级汇总**：token 四项总量、模型、`cwd`、创建时间，不用解压 zstd
- **自带速度指标**：`ttftMs`（首 token 延迟总和，÷`ttftSteps` 得均值）、`decodeTokens ÷ decodeMs` 即 tokens/s、`llmMs`/`toolMs` 耗时拆分
- 实测与事件流**完全同步**：某会话缓存 `seq=7405`，zstd 里最大 `seq` 也是 7405

注意：

- 它是**缓存**，带 `version` 字段，格式可能随 dsh 升级变化——解析要容错；丢失/损坏时回退到事件流
- **只有会话粒度**：按天 / 按小时的维度仍需从事件流（`seq` + `time`）重建
- 无 cost、无 reasoning token 分项（tokenUsage totals 只有 4 项）
- 另有旧版单文件 `~/.dsh/storages/session_projcache.json`（已废弃，可忽略）

## 三种格式版本

> **adapter 只解析 v4**（`*.v4.jsonl.zstd`，唯一仍在写入的格式）；旧格式与 v3 在扫描入口**静默跳过**——不解析、不告警。需要 v4 之前的历史数据时，legacy 描述仅作格式存档参考。v4 最早始于 2026-09-28。

首行是会话元数据：`{"type":"session","version":4,"id":"...","createdAt":...,"cwd":"~/Code/my-project",...}` —— **`cwd` 就是项目维度**。

### 旧格式（`session.jsonl.zstd`）

usage 在流式 chunk 事件里：

```json
{"type":"assistant/chunk","seq":412,"time":1786678849174,
 "data":{"turn":1,"step":2,"chunk":{"type":"usage",
 "usage":{"inputTokens":110,"outputTokens":156,"cacheReadTokens":12800}}}}
```

- usage key 集合有两种：`[inputTokens, outputTokens]` 或加 `cacheReadTokens`
- **无** reasoning / cacheWrite / total，**无** cost

### v3（`session.v3.jsonl.zstd`）

usage 挪到 `assistant/message` 事件上：

```json
{"type":"assistant/message","seq":...,"time":...,
 "data":{"usage":{"inputTokens":3577,"outputTokens":159,"totalTokens":11672,
                  "cacheReadTokens":7936,"reasoningTokens":0}, ...}}
```

### v4（`session.v4.jsonl.zstd`，当前主流）

同样在 `assistant/message`：

```json
{"type":"assistant/message","seq":19,"time":1790574838769,
 "data":{"turn":1,"step":1,
   "usage":{"inputTokens":3787,"outputTokens":1197,"cacheReadTokens":6400,
            "cacheWriteTokens":0,"totalTokens":11384},
   "message":{...}, "stream":[...]}}
```

- 有 `cacheWriteTokens`，**无** `reasoningTokens`
- `data.stream[].chunk.usage` 里还有一份重复 usage（同一响应的流式记录），**只读 `data.usage`，避免双计**

### 各版本 usage 字段对照

| 字段 | 旧 | v3 | v4 |
|---|---|---|---|
| `inputTokens` / `outputTokens` | ✅ | ✅ | ✅ |
| `cacheReadTokens` | 部分 | ✅ | ✅ |
| `cacheWriteTokens` | ❌ | ❌ | ✅ |
| `reasoningTokens` | ❌ | ✅ | ❌ |
| `totalTokens` | ❌ | ✅ | ✅ |

## 模型信息

usage 事件本身不带模型，模型在：

- `request/context`：`{"provider":"deepseek-official","model":"deepseek-flash","contextWindow":1000000}`
- `request/header`：`data.header.config` 里也有 `provider` / `model` / `reasoningEffort`

一个会话内模型可能变化（按 `request/context` 出现的 seq 分段归属）。

**模型命名**：源里会出现 `deepseek-flash`（2026-09-10 起的无版本名）、`deepseek-v4.1-flash`，以及更老的 `deepseek-v4-flash` / `deepseek-v4-flash-vision-exp`。adapter 入口用 `pricing.ts` 的 `identifyModel` 统一归到 `deepseek-v4.1-flash`（最新版本）；`deepseek-flash` 等旧名保留在 `aliases` 里，分组与展示都用统一名。

## 事件通用结构

```json
{"type":"...","seq":13,"time":1790574831902,"data":{...}}
```

- `time`：毫秒时间戳；`seq`：会话内单调递增
- **实测某 v4 文件 453 条带 seq 的事件无重复**
- `session-log-deepseek/delivery-accepted` 是投递确认，**同一 `throughSeq` 会重复出现多次**——聚合时按 `(session, seq)` 去重即可覆盖此类重复事件
- `step/start`、`step/end`、`turn/start`、`turn/end` 齐全 → 可算首 token 延迟（`step/start.time` → 首个 usage/chunk 的 time）和 tokens/s

## ⚠️ 关键注意事项

1. **没有任何 cost 字段** → 必须走 `src/pricing.ts` 价格表（当前统一按高峰价，见 README）。
2. **格式版本不统一**：adapter 只解析 v4，旧格式 / v3 静默跳过；文件名与首行 `session.version` 的对应关系为：`session.jsonl.zstd` ↔ version 0、`.v3.` ↔ 3、`.v4.` ↔ 4，**以内容 version 字段为准**。
3. **增量写入**：活跃会话文件持续追加写；文件整体是 zstd 流，**无法只读尾部**，每次需全量解压（单文件最大约 0.5 MB，可接受）。
4. `session.lock`（0 字节）出现在活跃会话目录，可作为"正在写"的信号，但读取本身不受影响。
5. 速度指标：
   - tokens/s = `outputTokens ÷ Σ(stream[] 末块 time − 首块 time)`（纯解码，剔除 TTFT 与工具执行；无分块时退回 step 跨度）
   - 首 token 延迟 = `stream[]` 首块 time − 同 step 的 `step/start` time（不能用 usage 事件 time——那是流结束时刻）

## 验证过的命令

```bash
# 事件类型分布
zstd -dc <file> | jq -r '.type' | sort | uniq -c | sort -rn

# 抽 usage（v3/v4）
zstd -dc <file> | jq -c 'select(.type=="assistant/message") | .data.usage'

# 旧格式抽 usage
zstd -dc <file> | jq -c 'select(.data.chunk.type?=="usage") | .data.chunk.usage'
```
