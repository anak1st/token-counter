// 内存聚合：按 (模型, 本地日期) 分桶，每组末尾折出 Total 行

import type { DayRow, ModelGroup, UsageEvent } from "./types";
import type { ModelPricing } from "./pricing";
import { dayKey } from "./time";

function emptyRow(date: string): DayRow {
  return {
    date,
    input: 0,
    output: 0,
    reasoning: 0,
    cache_read: 0,
    cache_write: 0,
    cost: 0,
    ttftSumMs: 0,
    ttftCount: 0,
    genMs: 0,
    speedTokens: 0,
  };
}

function fold(target: DayRow, row: DayRow): void {
  target.input += row.input;
  target.output += row.output;
  target.reasoning += row.reasoning;
  target.cache_read += row.cache_read;
  target.cache_write += row.cache_write;
  target.cost += row.cost;
  target.ttftSumMs += row.ttftSumMs;
  target.ttftCount += row.ttftCount;
  target.genMs += row.genMs;
  target.speedTokens += row.speedTokens;
}

export function aggregate(events: Iterable<UsageEvent>): ModelGroup[] {
  const byModel = new Map<string, Map<string, DayRow>>();
  // 同一模型的事件共享同一份入口结算价格，取首个事件的即可
  const pricingByModel = new Map<string, ModelPricing | undefined>();

  for (const e of events) {
    let days = byModel.get(e.model);
    if (!days) {
      days = new Map();
      byModel.set(e.model, days);
      pricingByModel.set(e.model, e.pricing);
    }
    const key = dayKey(e.ts);
    let row = days.get(key);
    if (!row) {
      row = emptyRow(key);
      days.set(key, row);
    }
    row.input += e.usage.input;
    row.output += e.usage.output;
    row.reasoning += e.usage.reasoning;
    row.cache_read += e.usage.cache_read;
    row.cache_write += e.usage.cache_write;
    row.cost += e.cost;
    if (e.ttftMs !== undefined) {
      row.ttftSumMs += e.ttftMs;
      row.ttftCount += 1;
    }
    if (e.durationMs !== undefined && e.durationMs > 0) {
      // 速度分子与分母同源：只有带时长的样本才计入 speedTokens
      row.genMs += e.durationMs;
      row.speedTokens += e.usage.output + e.usage.reasoning;
    }
  }

  const groups: ModelGroup[] = [];
  for (const [model, days] of [...byModel.entries()].sort(([a], [b]) => a.localeCompare(b))) {
    const sorted = [...days.values()].sort((a, b) => a.date.localeCompare(b.date));
    const total = emptyRow("total");
    for (const row of sorted) fold(total, row);
    groups.push({ model, pricing: pricingByModel.get(model), days: sorted, total });
  }
  return groups;
}
