# token-counter CLI 输出设计

> 2026-10-23。当前只做纯 CLI 输出：先总计，再简化分组表；图表暂不实现。

## 1. 目标

给普通输出增加一点层次，但不做报表系统：

- 最前面是**跨 provider / 模型的总计**，回答“这段时间整体花了多少、用了多少 token”。
- 后面是**按 (provider, 统一模型名) 分组**的明细，保留现有分组逻辑。
- token 统计合并成一列，不再拆 input / output / cache，也不再输出 Per 1M。
- 输出全英文、纯空格对齐、无装饰线；费用一律人民币 `¥`。

不做：图表、交互式 TUI、Web、会话/预算/质量等报表。

## 2. 输出结构

```text
all models
Date           Tokens  Chart                     Cache%    Cost  TTFT  Speed
2026-10-17    434,971  ████████████████           95.0%  ¥0.124  2.9s   33/s
2026-10-18    636,402  ████████████████████████   88.7%  ¥0.233  5.3s   43/s
Total       1,071,373  —                          91.3%  ¥0.357  4.1s   38/s

deepseek-official · deepseek-v4.1-flash
price: in miss ¥2.00 / hit ¥0.04 / out ¥8.00 per 1M tok
Date           Tokens  Chart                     Cache%    Cost  TTFT  Speed
2026-10-17    434,971  ████████████████           95.0%  ¥0.124  2.9s   33/s
2026-10-18    636,402  ████████████████████████   88.7%  ¥0.233  5.3s   43/s
Total       1,071,373  —                          91.3%  ¥0.357  4.1s   38/s
```

- 总计表标题固定为 `all models`：它合并了本次查询全部有价 provider / 模型。
- 总计表与分组表使用同一套列，并**共用列宽**，所以整段输出的数字纵向对齐。
- `Chart` 列：所有表共用同一最大日 token 值，最大值填满整条；`Total` 不是时间点，显示 `—`。
- 组与组之间空一行；总计表与第一组之间也空一行。

## 3. 列定义

| 列 | 含义 |
|---|---|
| `Date` | 本地日期，统一 `YYYY-MM-DD` |
| `Tokens` | 合并后的总 token，见 §4 |
| `Chart` | Tokens 柱状图；所有表共用同一最大日 token 值，最大值填满；`Total` 显示 `—` |
| `Cache%` | 缓存 token 占总 token 比例；无 token 时为 `—` |
| `Cost` | 当日人民币费用，3 位小数；按价目表在 adapter 入口结算 |
| `TTFT` | 当日平均首 token 延迟；无样本时为 `—` |
| `Speed` | 当日平均输出速度（tokens/s）；无样本时为 `—` |

- 数字列右对齐，日期左对齐，列间距 2 个空格。
- 末行固定为 `Total`，按未取整的原始费用累加后格式化。

## 4. Token 口径

```text
Tokens = input + output + reasoning + cache_read + cache_write
```

- `output` 与 `reasoning` 互斥，两者相加就是用户可见的输出量。
- `cache_read` 与 `cache_write` 合并为缓存 token。
- `Cache% = (cache_read + cache_write) / Tokens`，和旧表的 `Cache%` 口径一致。
- Per 1M 已删除：它属于更细的“价格明细”视角，不在当前普通输出里出现。

## 5. 价格行

每个分组的组头下保留一行：

```text
price: in miss ¥2.00 / hit ¥0.04 / out ¥8.00 per 1M tok
```

这是该模型的单价，不是每行费用；总计表没有价格行，因为合并了不同价目。

## 6. 未知模型

查不到人民币价的模型不进入总计表与分组表，只在末尾输出一行：

```text
unknown models (no CNY pricing): provider/model, ...
```

保持现有行为，便于补价。

## 7. 颜色

- `--color auto|always|never`，默认 `auto`；每个子命令都支持。
- `auto` 只在 stdout 是 TTY 时上色，并尊重 `NO_COLOR` / `FORCE_COLOR`；`always` / `never` 显式覆盖。
- 颜色只用于结构：`all models` 与组头 bold cyan，表头与 `Total` bold，Chart 柱 cyan，`price:` dim，未知模型 yellow，错误 red；数据值不上色。
- 所有颜色都在表格补齐之后包裹，ANSI 码不参与列宽。
- `--color never`、`NO_COLOR`、管道输出必须与无色版本逐字节一致。

## 8. 实现位置

- `src/cli.ts`：Commander 子命令、选项与帮助。
- `src/color.ts`：`--color` 解析与语义化样式。
- `src/aggregate.ts` 的 `overall()`：把各分组的每日行按日期合并，折出总计。
- `src/render.ts` 的 `renderOutput()`：总计表 + 分组表，共用列宽、共用 Chart 刻度并应用颜色。
- `src/index.ts`：分发子命令，扫描、聚合、渲染。
- 数据源、日期区间、计价与聚合口径不变。

## 9. 后续

- Chart 列已实现；后续可考虑按小时/周聚合，或给总计表增加独立趋势图。
- 需要更细的 token 拆分时，再加独立的详细模式（input / cache read / cache write / output / reasoning / total + Per 1M），不要把这些列塞回当前普通表。
