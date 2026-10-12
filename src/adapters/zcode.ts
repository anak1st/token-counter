// zcode adapter：只读打开 ~/.zcode/cli/db/db.sqlite，从 model_usage 表取请求粒度用量
// input_tokens 含 cache_read → 计价基数（未命中输入）需扣减
// 速度原料：TTFT 为源生字段 time_to_first_token_ms；生成耗时 = duration_ms - TTFT（纯解码）
// 字段映射：turn/agent/status 为源生列；provider 恒 zhipu（源 provider_id 是计费套餐名）；
// status=cancelled 或用户取消记 aborted（此类行 usage 通常全 0，会被零用量过滤跳过）

import { Database } from "bun:sqlite";
import type { DateRange, Usage, UsageEvent } from "../types";
import { calcCost, resolveModel } from "../pricing";

/** model_usage 表行（列名同库，data 语义见 docs/zcode.md） */
interface UsageRow {
  session_id: string;
  model_id: string;
  turn_id: string | null;
  agent: string | null;
  status: string;
  cancelled_by_user: number;
  started_at: number;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cache_read_input_tokens: number;
  cache_creation_input_tokens: number;
  time_to_first_token_ms: number | null;
  duration_ms: number | null;
}

const warned = new Set<string>();

/** 无人民币报价的模型每进程只警告一次，费用按 0 计 */
function warnOnce(model: string): void {
  if (warned.has(model)) return;
  warned.add(model);
  console.warn(`warning: no CNY pricing for model "${model}" (zcode), cost counted as 0`);
}

/** 只读扫描 model_usage 表，惰性产出区间内的请求事件 */
export async function* scanZcode(range: DateRange): AsyncGenerator<UsageEvent> {
  const path = `${process.env.HOME}/.zcode/cli/db/db.sqlite`;
  let db: Database;
  try {
    db = new Database(path, { readonly: true });
  } catch (e) {
    throw new Error(`cannot open zcode db at ${path}: ${e instanceof Error ? e.message : e}`);
  }

  try {
    const rows = db
      .query(
        `SELECT session_id, model_id, turn_id, agent, status, cancelled_by_user,
                started_at,
                input_tokens, output_tokens, reasoning_tokens,
                cache_read_input_tokens, cache_creation_input_tokens,
                time_to_first_token_ms, duration_ms
         FROM model_usage
         WHERE started_at >= ? AND started_at < ?
         ORDER BY started_at`,
      )
      .all(range.start, range.end) as UsageRow[];

    for (const row of rows) {
      const cacheRead = row.cache_read_input_tokens;
      const usage: Usage = {
        input: row.input_tokens - cacheRead,
        output: row.output_tokens,
        reasoning: row.reasoning_tokens,
        cache_read: cacheRead,
        cache_write: row.cache_creation_input_tokens,
      };
      // cancelled 行 usage 全为 0，跳过以免生成空日期行
      if (usage.input + usage.output + usage.cache_read + usage.cache_write === 0) continue;

      const model = row.model_id;
      // 源 provider_id 是计费套餐名（builtin:bigmodel-coding-plan），计价 provider 恒为 zhipu
      const provider = "zhipu";
      const pricing = resolveModel(provider, model);
      let cost = 0;
      if (pricing) cost = calcCost(pricing, usage);
      else warnOnce(model);

      // status=cancelled 或用户主动取消的行保留部分用量，标记为中止
      const aborted = row.status === "cancelled" || row.cancelled_by_user === 1;

      // 速度分母用纯解码窗口：duration_ms 含 TTFT，有则扣除；无 TTFT 的行按全时长计
      let decodeMs: number | undefined;
      if (row.duration_ms != null && row.duration_ms > 0) {
        const ttft = row.time_to_first_token_ms ?? 0;
        decodeMs = row.duration_ms - ttft;
      }

      const event: UsageEvent = {
        source: "zcode",
        session: row.session_id,
        model,
        provider,
        ts: row.started_at,
        usage,
        cost,
      };
      if (pricing) event.pricing = pricing;
      if (row.agent !== null) event.agent = row.agent;
      if (row.turn_id !== null) event.turn = row.turn_id;
      if (aborted) event.aborted = true;
      if (row.time_to_first_token_ms != null) event.ttftMs = row.time_to_first_token_ms;
      if (decodeMs !== undefined && decodeMs > 0) event.durationMs = decodeMs;
      yield event;
    }
  } finally {
    db.close();
  }
}
