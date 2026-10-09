// 分组表格渲染：全英文文案、纯空格对齐、无装饰线
// 价格行只读 ModelGroup.pricing（入口结算时带来），渲染层不感知 provider

import type { DayRow, ModelGroup, Source } from "./types";
import type { ModelPricing } from "./pricing";

const HEADERS = ["Date", "Input", "Output", "Cache", "Cache%", "Cost", "Per 1M", "TTFT", "Speed"] as const;
/** Date 左对齐，其余右对齐 */
const RIGHT = [false, true, true, true, true, true, true, true, true];

const fmtInt = (n: number): string => n.toLocaleString("en-US");

/** 一行的展示值；Output 含 reasoning，Cache 含 read+write */
function rowCells(row: DayRow, label: string): string[] {
  const output = row.output + row.reasoning;
  const cache = row.cache_read + row.cache_write;
  const total = row.input + output + cache;
  return [
    label,
    fmtInt(row.input),
    fmtInt(output),
    fmtInt(cache),
    total > 0 ? `${((cache / total) * 100).toFixed(1)}%` : "—",
    `¥${row.cost.toFixed(3)}`,
    total > 0 ? `¥${((row.cost / total) * 1e6).toFixed(2)}` : "—",
    row.ttftCount > 0 ? `${(row.ttftSumMs / row.ttftCount / 1000).toFixed(1)}s` : "—",
    row.genMs > 0 ? `${Math.round((output / row.genMs) * 1000)}/s` : "—",
  ];
}

function joinRow(cells: readonly string[], widths: readonly number[]): string {
  return cells
    .map((c, i) => {
      const w = widths[i] ?? c.length;
      return (RIGHT[i] ?? false) ? c.padStart(w) : c.padEnd(w);
    })
    .join("  ")
    .trimEnd();
}

/** 组头下的价格行：单位 元 / 百万 token；未知模型（入口未取到价）显示 — */
function priceLine(pricing: ModelPricing | undefined): string {
  if (!pricing) return "price: —";
  return (
    `price: in miss ¥${pricing.input_cache_miss.toFixed(2)}` +
    ` / hit ¥${pricing.input_cache_hit.toFixed(2)}` +
    ` / out ¥${pricing.output.toFixed(2)} per 1M tok`
  );
}

export function render(groups: readonly ModelGroup[], source: Source): string {
  const lines: string[] = [];

  // 年份一致时日期显示 MM-DD，跨年则显示完整 YYYY-MM-DD
  const years = new Set<string>();
  for (const g of groups) for (const d of g.days) years.add(d.date.slice(0, 4));
  const shortDates = years.size <= 1;

  groups.forEach((g, gi) => {
    if (gi > 0) lines.push("");
    lines.push(`${source} · ${g.model}`);
    lines.push(priceLine(g.pricing));
    lines.push("");

    const rows: string[][] = [];
    for (const d of g.days) {
      const label = shortDates ? d.date.slice(5) : d.date;
      rows.push(rowCells(d, label));
    }
    rows.push(rowCells(g.total, "Total"));

    const widths = HEADERS.map((h, i) =>
      Math.max(h.length, ...rows.map((r) => r[i]?.length ?? 0)),
    );
    lines.push(joinRow([...HEADERS], widths));
    for (const r of rows) lines.push(joinRow(r, widths));
  });

  return lines.join("\n");
}
