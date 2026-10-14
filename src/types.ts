// 归一化事件与聚合的类型定义

import type { ModelPricing } from "./pricing";

export type Source = "mimo" | "dsh" | "zcode" | "opencode";

export interface Usage {
  input: number;
  output: number;
  /** 思考 token：必须与 output 互斥——源若为包含关系（如 dsh 旧格式），adapter 归一扣除；拿不到则为 0 且留在 output 内 */
  reasoning: number;
  cache_read: number;
  cache_write: number;
}

export interface UsageEvent {
  source: Source;
  session: string;
  /** 计价模型名（价格表的键） */
  model: string;
  /** 执行体（main / 子代理），turn 级统计按此过滤；源无此概念则缺省 */
  agent?: string;
  /** 所属问题（turn）标识，turn 级统计分组用；源无法直接给出则缺省 */
  turn?: string;
  /** 被中止/出错（正常完成缺省） */
  aborted?: boolean;
  /** 分桶时间戳（毫秒，按本地时区分天） */
  ts: number;
  usage: Usage;
  /** 入口已结算的人民币费用 */
  cost: number;
  /** 入口结算时所用的价格（未知模型为 undefined）；随事件流转，下游只读 */
  pricing?: ModelPricing;
  /** 首 token 延迟样本（毫秒） */
  ttftMs?: number;
  /** 纯解码窗口（毫秒）：不含 TTFT 与工具执行 */
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
  /** 参与速度计算的 output+reasoning：只累计带时长的样本，与 genMs 保证同源 */
  speedTokens: number;
}

export interface ModelGroup {
  model: string;
  /** 该模型结算用的价格（组内事件共享），供渲染展示 */
  pricing?: ModelPricing;
  days: DayRow[];
  total: DayRow;
}
