// 内存聚合：按 (模型, 本地日期) 分桶

import type { DayRow, ModelGroup, UsageEvent } from "./types";
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
}

export function aggregate(events: Iterable<UsageEvent>): ModelGroup[] {
  const byModel = new Map<string, Map<string, DayRow>>();

  for (const e of events) {
    let days = byModel.get(e.model);
    if (!days) {
      days = new Map();
      byModel.set(e.model, days);
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
    if (e.durationMs !== undefined && e.durationMs > 0) row.genMs += e.durationMs;
  }

  const groups: ModelGroup[] = [];
  for (const [model, days] of [...byModel.entries()].sort(([a], [b]) => a.localeCompare(b))) {
    const sorted = [...days.values()].sort((a, b) => a.date.localeCompare(b.date));
    const total = emptyRow("total");
    for (const row of sorted) fold(total, row);
    groups.push({ model, days: sorted, total });
  }
  return groups;
}
