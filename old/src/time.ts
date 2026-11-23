// 日期解析与本地时区分桶

import type { DateRange } from "./types";

const pad = (n: number): string => String(n).padStart(2, "0");

/** 解析 YYYY-MM-DD 为本地时区当天零点的毫秒时间戳 */
export function parseDate(s: string): number {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (!m) throw new Error(`invalid date "${s}" (expected YYYY-MM-DD)`);
  const y = Number(m[1]);
  const mo = Number(m[2]);
  const d = Number(m[3]);
  const t = new Date(y, mo - 1, d).getTime();
  const back = new Date(t);
  if (back.getFullYear() !== y || back.getMonth() !== mo - 1 || back.getDate() !== d) {
    throw new Error(`invalid date "${s}"`);
  }
  return t;
}

/** 最近 N 天（含今天）：[今天-(N-1), 明天) */
export function lastNDays(n: number): DateRange {
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const start = new Date(today);
  start.setDate(start.getDate() - (n - 1));
  const end = new Date(today);
  end.setDate(end.getDate() + 1);
  return { start: start.getTime(), end: end.getTime() };
}

/** 给定日期的次日零点：把含 --end 的区间转成内部左闭右开 */
export function nextDayStart(ts: number): number {
  const d = new Date(ts);
  d.setDate(d.getDate() + 1);
  return d.getTime();
}

/** 毫秒时间戳 → 本地时区日期键 YYYY-MM-DD */
export function dayKey(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}
