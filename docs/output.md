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
Date           Tokens  Cache%    Cost  TTFT  Speed
2026-10-17    434,971   95.0%  ¥0.124   2.9s   33/s
2026-10-18    636,402   88.7%  ¥0.233   5.3s   43/s
Total       1,071,373   91.3%  ¥0.357   4.1s   38/s

deepseek-official · deepseek-v4.1-flash
price: in miss ¥2.00 / hit ¥0.04 / out ¥8.00 per 1M tok

Date           Tokens  Cache%    Cost  TTFT  Speed
2026-10-17    434,971   95.0%  ¥0.124   2.9s   33/s
2026-10-18    636,402   88.7%  ¥0.233   5.3s   43/s
Total       1,071,373   91.3%  ¥0.357   4.1s   38/s
```

- 总计表标题固定为 `all models`：它合并了本次查询全部有价 provider / 模型。
- 总计表与分组表使用同一套列，并**共用列宽**，所以整段输出的数字纵向对齐。
- 组与组之间空一行；总计表与第一组之间也空一行。

## 3. 列定义

| 列 | 含义 |
|---|---|
| `Date` | 本地日期，统一 `YYYY-MM-DD` |
| `Tokens` | 合并后的总 token，见 §4 |
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

## 7. 实现位置

- `src/aggregate.ts` 的 `overall()`：把各分组的每日行按日期合并，折出总计。
- `src/render.ts` 的 `renderOverall()`：总计表；`render()`：分组表。
- `src/index.ts`：先打印总计，空一行，再打印分组。
- 数据源、日期区间、计价与聚合口径不变。

## 8. 后续

- 总计表上方加横向 token bar，作为概览；分组表不加图表。
- 需要更细的 token 拆分时，再加独立的详细模式（input / cache read / cache write / output / reasoning / total + Per 1M），不要把这些列塞回当前普通表。
