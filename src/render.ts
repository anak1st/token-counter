// 表格渲染：全英文文案、纯空格对齐、无装饰线
// 日期统一 YYYY-MM-DD；总计表与所有分组表共用同一套列宽，保证整段输出纵向对齐；
// 表格合并 token 统计（input + output + reasoning + cache_read + cache_write），不再输出 Per 1M；
// 颜色只包裹补齐后的行，不参与列宽计算；价格行只读 ModelGroup.pricing，渲染层不重算价格。

import type { DayRow, ModelGroup, Overall } from "./types";
import type { ModelPricing } from "./pricing";
import { plainStyle, type Style } from "./color";

const HEADERS = ["Date", "Tokens", "Cache%", "Cost", "TTFT", "Speed"] as const;
/** Date 左对齐，其余右对齐 */
const RIGHT = [false, true, true, true, true, true];

const fmtInt = (n: number): string => n.toLocaleString("en-US");

/** 展示用总 token：output 与 reasoning 互斥，合并计入；cache 含 read+write */
function totalTokens(row: DayRow): number {
  return row.input + row.output + row.reasoning + row.cache_read + row.cache_write;
}

/** 一行的展示值；空值用 —，不写 0 */
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

/** 表格主体：表头 + 每日行 + Total 行；列宽由调用方统一传入，颜色只包裹补齐后的行 */
function tableLines(
  days: readonly DayRow[],
  total: DayRow,
  widths: readonly number[],
  style: Style,
): string[] {
  const rows: string[][] = [];
  for (const day of days) rows.push(rowCells(day, day.date));
  rows.push(rowCells(total, "Total"));

  const lines = [style.header(joinRow([...HEADERS], widths))];
  for (const row of rows.slice(0, -1)) lines.push(joinRow(row, widths));
  lines.push(style.header(joinRow(rows[rows.length - 1]!, widths)));
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

/** 计算全段输出共用的列宽：总计表与全部分组表的行都参与取最大值 */
function sharedWidths(
  value: Overall,
  groups: readonly ModelGroup[],
): number[] {
  const rows: string[][] = [];
  for (const day of value.days) rows.push(rowCells(day, day.date));
  rows.push(rowCells(value.total, "Total"));
  for (const group of groups) {
    for (const day of group.days) rows.push(rowCells(day, day.date));
    rows.push(rowCells(group.total, "Total"));
  }

  return HEADERS.map((header, i) =>
    Math.max(header.length, ...rows.map((row) => row[i]?.length ?? 0)),
  );
}

/** 全段输出：总计表、空行、各分组表；所有表格共用同一套列宽 */
export function renderOutput(
  value: Overall,
  groups: readonly ModelGroup[],
  style: Style = plainStyle,
): string {
  const widths = sharedWidths(value, groups);

  const lines: string[] = [
    style.title("all models"),
    ...tableLines(value.days, value.total, widths, style),
  ];
  for (const group of groups) {
    lines.push("");
    lines.push(style.title(`${group.provider} · ${group.model}`));
    lines.push(style.dim(priceLine(group.pricing)));
    lines.push(...tableLines(group.days, group.total, widths, style));
  }

  return lines.join("\n");
}
