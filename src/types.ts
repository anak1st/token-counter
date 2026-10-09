// 归一化事件与聚合的类型定义

import type { ModelPricing } from "./pricing";

export type Source = "mimo" | "dsh";

export interface Usage {
  input: number;
  output: number;
  reasoning: number;
  cache_read: number;
  cache_write: number;
}

export interface UsageEvent {
  source: Source;
  session: string;
  model: string;
  /** 分桶时间戳（毫秒，按本地时区分天） */
  ts: number;
  usage: Usage;
  /** 入口已结算的人民币费用 */
  cost: number;
  /** 入口结算时所用的价格（未知模型为 undefined）；随事件流转，下游只读 */
  pricing?: ModelPricing;
  /** 首 token 延迟样本（毫秒） */
  ttftMs?: number;
  /** 该样本所属的生成耗时（毫秒） */
  durationMs?: number;
}

export interface DateRange {
  /** 含 */
  start: number;
  /** 不含 */
  end: number;
}

export interface DayRow {
  /** YYYY-MM-DD（本地时区） */
  date: string;
  input: number;
  output: number;
  reasoning: number;
  cache_read: number;
  cache_write: number;
  cost: number;
  ttftSumMs: number;
  ttftCount: number;
  genMs: number;
}

export interface ModelGroup {
  model: string;
  /** 该模型结算用的价格（组内事件共享），供渲染展示 */
  pricing?: ModelPricing;
  days: DayRow[];
  total: DayRow;
}
