# token-counter 设计文档

> 2026-10-09。数据源细节不在本文，见 [mimo.md](mimo.md)、[dsh.md](dsh.md) 与 [zcode.md](zcode.md)；本文只谈数据流与取舍。

## 1. 目标

一个命令行查询脚本。**先选数据源**，再给日期范围：

```
token-counter dsh --last 7                     # 指定源 + 最近 7 天（含今天）
token-counter dsh --from 2026-10-01 --to 10-08 # 指定源 + 起止日期
token-counter mimo --last 30
```

- 数据源是**位置参数**（`mimo` / `dsh` / `zcode` / `opencode`）：只跑被选中的那个 adapter，不查的源完全不访问
- **无本地存储**：每次查询直接读数据源，算完即走，不留库、不留状态

输出为**分组多表**：一个 (provider, 模型) 一组，组内每天一行，竖向排列。**输出文案全部用英文**（中文双字节不利于对齐），仅费用用人民币 `¥`；纯文本对齐，不画装饰线：

```
deepseek-official · deepseek-flash
price: in miss ¥2.00 / hit ¥0.04 / out ¥8.00 per 1M tok

Date       Input      Output       Cache    Cache%     Cost   Per 1M   TTFT   Speed
10-02     12,480     38,201   1,024,512     94.2%   ¥0.087   ¥0.08    1.2s    42/s
10-03      8,102     15,993     486,208     94.6%   ¥0.041   ¥0.08    0.9s    38/s
10-08     21,340     52,118   2,104,832     96.1%   ¥0.134   ¥0.06    1.5s    45/s
Total     41,922    106,312   3,615,552     95.0%   ¥0.262   ¥0.07    1.2s    42/s

opencode-go · deepseek-v4.1-flash
price: in miss ¥2.00 / hit ¥0.04 / out ¥8.00 per 1M tok
10-05      4,096      9,810     128,000     89.4%   ¥0.172   ¥1.21    2.1s    31/s
```

- **分组键 = (provider, 模型名)**：同一模型可能来自多个 provider（网关 / 套餐），分开成组；组头显示 `provider · 模型`（provider 源里没有则归为 `unknown`）。一次查询只选一个源，但源内可能有多个 provider/模型 → 多组
- 组内每行一天（按日期升序，只列区间内有数据的天），组末附合计行
- token 分项：input / output / cache（read+write）；百分比为各分项占当日总 token 的比例（示例展示缓存%，实际可扩展为 input%/output%/cache% 三列）
- **Per 1M（性价比）**：当日总费用 ÷ 当日总 token × 10⁶，即混合有效单价（¥ / 百万 token）——缓存占比越高越低
- 速度：首 token 延迟（TTFT）、平均 token 输出速度（tokens/s）

**非目标**：不做常驻服务、不做历史留存、不做 Web UI。数据源本身被清理就接受结果变短（见 §9 风险）。

## 2. 硬约束（来自数据源探索）

| # | 约束 | 来源 | 影响 |
|---|---|---|---|
| 1 | 两源形态迥异：活的 SQLite vs zstd 压缩事件流 | mimo / dsh | adapter 层隔离差异，核心只认归一化事件 |
| 2 | mimo/opencode 的 cost 字段是美元，dsh 没有 cost | 各源 | 一律查 `pricing.ts` 人民币价在入口结算，不采信源 cost、不做汇率换算 |
| 3 | dsh 历史上有三种格式，仅 v4 在持续写入 | docs/dsh.md | adapter 只解析 v4，旧格式静默跳过 |
| 4 | 同一事实存在双视角（mimo 的 message vs part；dsh 事件流 vs 聚合缓存） | 两侧 | 每源指定唯一主源，只从一处取数 |
| 5 | 数据源必须只读：mimo 是 WAL 活库，dsh 正被 desk 写入 | 两侧 | 只读打开，写操作不存在（连本地库都没有） |
| 6 | dsh 的 zstd 整流压缩，无法只读尾部 | docs/dsh.md | 查询期内全量解压（文件均 <1 MB，可接受） |
| 7 | 按天维度只有事件流能提供 | docs/dsh.md | dsh 走事件流全路径，聚合缓存仅作校验参考 |
| 8 | 模型命名会变、价格会调 | pricing.ts | 按模型名平铺查价（含 aliases）；无存储反而让价格永远最新 |

## 3. 数据流

```
 CLI：位置参数选源（mimo|dsh）+ 日期参数（--from/--to/--last N）
        │  解析为 [start_ms, end_ms)（本地时区）
        ▼
        │  按选中的源分发，只跑一个 adapter（另一个完全不访问）
        ▼
 ┌───────────────────────────────────┐
 │ 选中的 adapter（只读）              │
 │ mimo: SQL 按时间过滤               │
 │ dsh:  解压 zstd → 按版本解析        │
 └──────────────┬────────────────────┘
                │  归一化事件（惰性迭代，携带已结算 cost 与所用 pricing）
                ▼
       内存聚合：按 (provider, 模型, 本地日期) 三键分桶
       Map<provider, Map<model, Map<day, {input, output, cache_*, cost, ttft_samples, durations}>>>
                │  pricing 挂到 ModelGroup（组内共享）
                ▼
       每 (provider, 模型) 一组：补算百分比与速度 → 格式化表格输出（价格行只读 group.pricing，见 §6）
```

一次查询 = 解析参数 → 跑一个 adapter → 内存里聚合 → 按 (provider, 模型) 分组打印。**没有中间落盘，没有需要同步的状态。**

## 4. 核心抽象

### 4.1 归一化事件

整个系统唯一流通的数据结构：

必填：`source, session, model, ts, usage{input, output, reasoning, cache_read, cache_write}, cost`；
随源可选：`provider`（分组键，源给出网关/套餐名才填充，缺省归为 `unknown`）、`pricing, ttftMs, durationMs`（速度原料）、`agent, turn, aborted`（turn 级统计的能力字段，源能直接给出才填充，mimo 的 turn 暂缺）

- 用量维度源里没有就缺省（dsh v4 无 reasoning），**不编造**
- **reasoning 必须与 output 互斥**：源为包含关系时 adapter 归一扣除；拿不到则为 0 且留在 output 内（渲染以 `output + reasoning` 展示，依赖此不变量）
- **cost 与 pricing 在入口一次结算、一次取定**：各源统一按**模型名**查 `src/pricing.ts` 的**人民币单价** × token 数——**厂商只是价目表的分组标签，不是查价依据**；模型名先精确匹配、再走 aliases 归一。源里带 cost 的（mimo/opencode，均为美元）不采信、不换汇（见 mimo.md「cost 字段实测」）；dsh/zcode 本身无 cost。**查不到 model id 的模型视为未知，不出表格，仅在渲染末尾列出 provider/model**（便于补价）。结算用的 `pricing` 对象随事件携带 → 聚合挂到 `ModelGroup` → 渲染的价格行只读展示。**下游任何环节不再二次查价**，事件只携带结算结果
- **速度存原料不存派生值**：事件带 `ttftMs` / `durationMs`（纯解码窗口），TTFT / tokens/s 在聚合期按当次查询的口径计算——口径以后想改，改一处聚合逻辑即可；速度分子只累计带时长的样本（`DayRow.speedTokens`），与分母保证同源

### 4.2 Adapter

每个源一个 adapter，概念上是一个**带日期区间的事件迭代器**：

- 输入：`[start, end)` 区间
- 输出：区间内归一化事件的惰性迭代器
- **无状态**：不知道上次查过什么，也不记住这次查了什么。正确性不依赖任何持久化

### 4.3 取数的唯一性（无存储下的去重）

没有本地库，去重从"upsert 冲突策略"简化为**读取路径唯一**：

- mimo：只读 `message` 表（与 part 是同一份数据，双计问题被主源原则消解）
- dsh：只读事件流，usage 只取 `data.usage` 一处（忽略 `data.stream` 内的重复副本）；聚合缓存不参与取数
- zcode：只读 `model_usage` 表（与 `turn_usage` 是双视角，只取前者）
- opencode：只读 `session_message` 的 assistant 行；会话级汇总 `session_v2` 不参与（与消息求和不等，见 opencode.md）

一次查询内每个事实只被读一次，无需事件级去重。

## 5. 日期与聚合口径

- **时区**：按本机本地时区分桶（事件时间戳均为毫秒）
- **区间语义**：`--from` 含、`--to` 不含（左闭右开），`--last N` = 今天起往前 N 个自然日
- **分桶粒度**：天；聚合键 = (provider, 模型, 日期)，值为累加器
- **百分比**：各分项 ÷ 当日 (input + output + cache_read + cache_write)
- **费用**：逐事件计价后按天累加（而不是拿当日总量事后乘单价——单价对所有事件相同，结果等价，逐事件算对将来的分层计价更稳）

## 6. 输出格式

排版原则：**全英文、对齐、留白、无装饰**。

- **语言**：所有输出文案（表头、组头、合计行、提示与告警）一律英文——中文是双字节字符，与数字混排时对齐规则复杂且终端表现不一致；**唯独费用用人民币 `¥`**（如 `¥0.087`），这是数值格式的一部分，不是文案
- **分组**：每 (provider, 模型) 一组；组头一行 `provider · 模型`（如 `deepseek-official · deepseek-flash`，provider 源里没有则为 `unknown`），紧接一行该模型的 `price:`（三项单价，元/百万 token，查不到显示 `—`），组间空一行
- **组内**：首行表头（`Date / Input / Output / Cache / Cache% / Cost / Per 1M / TTFT / Speed`），然后每天一行（日期升序），末行 `Total`
- **对齐**：文本列左对齐（日期），数字列一律右对齐（千分位分隔），表头与所在列对齐方式一致；列间距 2 个空格
- **列宽**：按组内实际数据的最大宽度自适应，组与组之间允许不同宽（各组独立排版）
- **空值**：无数据的指标输出 `—`，不留空、不写 0
- **未知模型**：查不到人民币价的模型**不出分组表格**，只在全部组之后一行列出 provider/model（`unknown models (no CNY pricing): provider/model, ...`），便于补价
- **精度**：token 整数带千分位；费用 3 位小数带 `¥`；Per 1M 2 位小数带 `¥`；TTFT 1 位小数带 `s`；速度整数带 `/s`；百分比 1 位小数带 `%`
- 终端输出纯文本；将来 `--json` 直接吐结构化数据，格式化只属于人类输出层

## 7. 速度指标口径

| 指标 | 定义 | mimo 原料 | dsh 原料 | zcode 原料 | opencode 原料 |
|---|---|---|---|---|---|
| TTFT | 首 token 延迟，取当日均值 | step-start part 时间 − 消息 `time.created`（**近似**：含请求构建，库不存流式首包） | `stream[]` 首个分块 time − 同 step 的 `step/start` time | `time_to_first_token_ms`（真实流式测量） | 首个 `reasoning` part 的 `time.created` − 消息 `time.created`（**近似**） |
| 平均输出速度 | 带时长样本的 output tokens ÷ 这些样本的纯解码耗时（剔除 TTFT 与工具执行，分子分母同源） | step-start part → 流结束（reasoning/text/patch 的 `time_updated`、tool 的 `time_created`） | `stream[]` 末块 − 首块（无分块时退回 step 跨度） | `duration_ms − time_to_first_token_ms`（无 TTFT 时退回 `duration_ms`） | `time.streamed` − 首个 part 的 `time.created`（`streamed` 是流结束时刻；`text` part 无时间戳，无 reasoning 的消息无样本） |

- 各源的 Speed 统一为纯解码速度，可与 dsh 自带的 `decodeTokens ÷ decodeMs` 直接对比（实测差约 10%，来自分块标记粒度）；mimo 与 opencode 的 TTFT 是近似值
- 当日没有速度原料的行（如纯缓存命中、零输出）输出 `—` 而非 0

## 8. 性能预期

- mimo：一条带时间过滤的 SQL，毫秒级
- dsh：解压 + 解析区间内涉及的全部会话文件；本机 76 个会话、单文件 <0.5 MB，全量扫也是亚秒到几秒级
- 若将来变慢，优化顺序：先按文件 mtime/首行 createdAt 剪枝明显不相交的会话，再考虑内存 LRU 缓存解压结果——**在证明需要之前不引入**

## 9. 关键权衡

| 决策 | 备选 | 取舍 |
|---|---|---|
| **无本地存储** | SQLite 明细 + rollup | 省掉同步水位、幂等、重算三大块复杂度；价格每次查询都是最新。代价：每次查询都重读源（可接受，见 §8）；**源数据被清理则无法追溯**（接受） |
| cost 入口一次结算 | 查询期算 | 无存储下两者成本相同，入口结算让聚合逻辑完全无价格感知 |
| **源作为必选位置参数** | `--source` 可选、默认全查 | 一次只查一个源：实现只需跑一个 adapter（组头显示 provider · 模型，源本身无需出现在输出里）；要全查就查两次 |
| dsh 走事件流全路径 | 用聚合缓存做快路径 | 输出要按天拆分，会话级缓存本来就满足不了；减少一条路径即减少一类故障（缓存过期、格式变更） |
| 速度存原料 | 落库派生值 | 无存储下天然如此；口径演进只需改聚合期 |

## 10. 演进路线

1. **MVP**：两个 adapter + 内存聚合 + 表格输出（本文目标）
2. **加固**：`--json` 输出便于管道组合；未知模型 / 缺价时显式告警而不是静默算 0
3. **仅当需要时**：查询结果缓存（仍是无状态的文件缓存，不是数据库）
