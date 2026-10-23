// 内存聚合：按 (provider, 统一模型名, 本地日期) 分桶，每组末尾折出 Total 行
// provider 是分组键之一：同一模型来自不同 provider（网关/套餐）时分开成组

import type { DayRow, ModelGroup, Overall, UsageEvent } from "./types";
import type { ModelPricing } from "./pricing";
import { dayKey } from "./time";

/** 事件在聚合中的分组身份；源未给出 provider 时归为 unknown */
function groupKey(e: UsageEvent): { provider: string; model: string } {
  return { provider: e.provider ?? "unknown", model: e.model };
}

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
  // 外层 key = provider，内层 key = model；值为 日期 → 行
  const byProvider = new Map<string, Map<string, Map<string, DayRow>>>();
  // 同一 (provider, 统一模型名) 的事件共享同一份入口结算价格，取首个事件的即可
  const pricingByKey = new Map<string, ModelPricing | undefined>();

  for (const e of events) {
    const { provider, model } = groupKey(e);
    let byModel = byProvider.get(provider);
    if (!byModel) {
      byModel = new Map();
      byProvider.set(provider, byModel);
    }
    let days = byModel.get(model);
    if (!days) {
      days = new Map();
      byModel.set(model, days);
      pricingByKey.set(`${provider}\u0000${model}`, e.pricing);
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
  const providers = [...byProvider.entries()].sort(([a], [b]) => a.localeCompare(b));
  for (const [provider, byModel] of providers) {
    const models = [...byModel.entries()].sort(([a], [b]) => a.localeCompare(b));
    for (const [model, days] of models) {
      const sorted = [...days.values()].sort((a, b) => a.date.localeCompare(b.date));
      const total = emptyRow("total");
      for (const row of sorted) fold(total, row);
      groups.push({
        provider,
        model,
        pricing: pricingByKey.get(`${provider}\u0000${model}`),
        days: sorted,
        total,
      });
    }
  }
  return groups;
}

/** 跨分组合计：把所有 (provider, model) 的每日行按日期合并，末尾折出合计行 */
export function overall(groups: readonly ModelGroup[]): Overall {
  const byDay = new Map<string, DayRow>();
  for (const group of groups) {
    for (const row of group.days) {
      let merged = byDay.get(row.date);
      if (!merged) {
        merged = emptyRow(row.date);
        byDay.set(row.date, merged);
      }
      fold(merged, row);
    }
  }

  const days = [...byDay.values()].sort((a, b) => a.date.localeCompare(b.date));
  const total = emptyRow("total");
  for (const row of days) fold(total, row);
  return { days, total };
}
