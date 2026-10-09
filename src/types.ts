// 归一化事件与聚合的类型定义

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
  days: DayRow[];
  total: DayRow;
}
