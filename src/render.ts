// 表格渲染：全英文文案、纯空格对齐、无装饰线
// 日期统一 YYYY-MM-DD；总计表与所有分组表共用同一套列宽，保证整段输出纵向对齐；
// Tokens 后新增 Chart 列：所有表共用同一最大值，最大 token 行填满整条；
// 颜色只包裹补齐后的单元格/行，不参与列宽计算。

import type { DayRow, ModelGroup, Overall } from "./types";
import type { ModelPricing } from "./pricing";
import { plainStyle, type Style } from "./color";

const HEADERS = ["Date", "Tokens", "Chart", "Cache%", "Cost", "TTFT", "Speed"] as const;
/** Date 左对齐，Chart 左对齐，其余右对齐 */
const RIGHT = [false, true, false, true, true, true, true];
/** 计算非 Chart 列宽度时使用的表头 */
const BASE_HEADERS = ["Date", "Tokens", "Cache%", "Cost", "TTFT", "Speed"] as const;

const fmtInt = (n: number): string => n.toLocaleString("en-US");

/** 展示用总 token：output 与 reasoning 互斥，合并计入；cache 含 read+write */
function totalTokens(row: DayRow): number {
  return row.input + row.output + row.reasoning + row.cache_read + row.cache_write;
}

/** 一行的基础展示值（不含 Chart 列）；空值用 —，不写 0 */
function rowCells(row: DayRow, label: string): string[] {
  const tokens = totalTokens(row);
  const cache = row.cache_read + row.cache_write;
  return [
    label,
    fmtInt(tokens),
    tokens > 0 ? `${((cache / tokens) * 100).toFixed(1)}%` : "—",
    `¥${row.cost.toFixed(3)}`,
    row.ttftCount > 0 ? `${(row.ttftSumMs / row.ttftCount / 1000).toFixed(1)}s` : "—",
    row.genMs > 0 ? `${Math.round((row.speedTokens / row.genMs) * 1000)}/s` : "—",
  ];
}

/** 在 Tokens 后插入 Chart 列 */
function withChart(row: DayRow, label: string, chart: string): string[] {
  const cells = rowCells(row, label);
  return [cells[0]!, cells[1]!, chart, ...cells.slice(2)];
}

/** 按列宽拼一行：RIGHT 决定左右对齐，两端补齐后去掉行尾空格 */
function joinRow(cells: readonly string[], widths: readonly number[]): string {
  return cells
    .map((c, i) => {
      const w = widths[i] ?? c.length;
      return (RIGHT[i] ?? false) ? c.padStart(w) : c.padEnd(w);
    })
    .join("  ")
    .trimEnd();
}

/** 表格行：总计表与分组表都使用同一套行序 */
interface TableEntry {
  row: DayRow;
  label: string;
  /** 只有每日行参与 Chart 的最大值计算 */
  daily: boolean;
}

/** 收集全段输出的行，用于统一列宽与 Chart 最大值 */
function collectEntries(value: Overall, groups: readonly ModelGroup[]): TableEntry[] {
  const entries: TableEntry[] = [];
  for (const day of value.days) entries.push({ row: day, label: day.date, daily: true });
  entries.push({ row: value.total, label: "Total", daily: false });

  for (const group of groups) {
    for (const day of group.days) entries.push({ row: day, label: day.date, daily: true });
    entries.push({ row: group.total, label: "Total", daily: false });
  }
  return entries;
}

/** 全段共用的列宽与 Chart 刻度 */
interface Layout {
  widths: number[];
  chartWidth: number;
  maxTokens: number;
}

/** 计算统一列宽：Chart 宽度随终端剩余空间变化，最大 30 列 */
function makeLayout(value: Overall, groups: readonly ModelGroup[], width: number): Layout {
  const entries = collectEntries(value, groups);

  const baseWidths = BASE_HEADERS.map((header, i) =>
    Math.max(
      header.length,
      ...entries.map((entry) => rowCells(entry.row, entry.label)[i]?.length ?? 0),
    ),
  );

  // 7 列共 6 个列间距；剩余空间给 Chart，夹在 8..30
  const fixed = baseWidths.reduce((sum, w) => sum + w, 0) + (HEADERS.length - 1) * 2;
  const chartWidth = Math.max(8, Math.min(30, width - fixed));

  const widths = [baseWidths[0]!, baseWidths[1]!, chartWidth, ...baseWidths.slice(2)];
  let maxTokens = 0;
  for (const entry of entries) {
    if (!entry.daily) continue;
    maxTokens = Math.max(maxTokens, totalTokens(entry.row));
  }

  return { widths, chartWidth, maxTokens };
}

/** 生成一行 token 柱：所有表共用 maxTokens，最大值填满 chartWidth */
function tokenBar(tokens: number, layout: Layout, style: Style): string {
  if (layout.maxTokens <= 0) return "—";
  if (tokens <= 0) return style.bar(" ".repeat(layout.chartWidth));

  const filled = Math.max(
    1,
    Math.min(layout.chartWidth, Math.round((tokens / layout.maxTokens) * layout.chartWidth)),
  );
  return style.bar("█".repeat(filled).padEnd(layout.chartWidth));
}

/** 表格主体：表头、每日行、Total 行；列宽由 makeLayout 统一提供 */
function tableLines(
  days: readonly DayRow[],
  total: DayRow,
  layout: Layout,
  style: Style,
): string[] {
  const rows: string[][] = [];
  for (const day of days) {
    rows.push(withChart(day, day.date, tokenBar(totalTokens(day), layout, style)));
  }
  rows.push(withChart(total, "Total", "—"));

  const lines = [style.header(joinRow([...HEADERS], layout.widths))];
  for (const row of rows.slice(0, -1)) lines.push(joinRow(row, layout.widths));
  lines.push(style.header(joinRow(rows[rows.length - 1]!, layout.widths)));
  return lines;
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

/** 全段输出：总计表、空行、各分组表；列宽与 Chart 刻度全局统一 */
export function renderOutput(
  value: Overall,
  groups: readonly ModelGroup[],
  style: Style = plainStyle,
  width = 80,
): string {
  const layout = makeLayout(value, groups, width);

  const lines: string[] = [
    style.title("all models"),
    ...tableLines(value.days, value.total, layout, style),
  ];
  for (const group of groups) {
    lines.push("");
    lines.push(style.title(`${group.provider} · ${group.model}`));
    lines.push(style.dim(priceLine(group.pricing)));
    lines.push(...tableLines(group.days, group.total, layout, style));
  }

  return lines.join("\n");
}
