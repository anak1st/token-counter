// zcode adapter：只读打开 ~/.zcode/cli/db/db.sqlite，从 model_usage 表取请求粒度用量
// input_tokens 含 cache_read → 计价基数（未命中输入）需扣减
// 速度原料：TTFT 为源生字段 time_to_first_token_ms；生成耗时 = duration_ms - TTFT（纯解码）

import { Database } from "bun:sqlite";
import type { DateRange, Usage, UsageEvent } from "../types";
import { calcCost, resolveModel } from "../pricing";

interface UsageRow {
  session_id: string;
  model_id: string;
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

function warnOnce(model: string): void {
  if (warned.has(model)) return;
  warned.add(model);
  console.warn(`warning: no CNY pricing for model "${model}" (zcode), cost counted as 0`);
}

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
        `SELECT session_id, model_id, started_at,
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
      const pricing = resolveModel("zhipu", model);
      let cost = 0;
      if (pricing) cost = calcCost(pricing, usage);
      else warnOnce(model);

      // 速度分母用纯解码窗口：duration_ms 含 TTFT，有 TTFT 时扣除
      const decodeMs =
        row.duration_ms != null && row.duration_ms > 0
          ? row.time_to_first_token_ms != null && row.time_to_first_token_ms > 0
            ? row.duration_ms - row.time_to_first_token_ms
            : row.duration_ms
          : undefined;

      yield {
        source: "zcode",
        session: row.session_id,
        model,
        ts: row.started_at,
        usage,
        cost,
        ...(pricing && { pricing }),
        ...(row.time_to_first_token_ms != null && { ttftMs: row.time_to_first_token_ms }),
        ...(decodeMs !== undefined && decodeMs > 0 && { durationMs: decodeMs }),
      };
    }
  } finally {
    db.close();
  }
}
