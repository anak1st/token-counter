// mimo adapter：只读打开 mimocode.db，从 message 表取 assistant 消息
// TTFT 近似：step-start → 该 step 内第一个 reasoning/text part（库不存流式首包）

import { Database } from "bun:sqlite";
import type { DateRange, Usage, UsageEvent } from "../types";
import { calcCost, resolveModel } from "../pricing";

interface MsgRow {
  id: string;
  session_id: string;
  time_created: number;
  data: string;
}

interface PartRow {
  message_id: string;
  time_created: number;
  data: string;
}

const warned = new Set<string>();

function warnOnce(model: string): void {
  if (warned.has(model)) return;
  warned.add(model);
  console.warn(`warning: no CNY pricing for model "${model}" (mimo), cost counted as 0`);
}

/** 每个 step 的 TTFT 样本 = 第一个 content part 时间 - step-start 时间；消息内取均值 */
function ttftFromParts(parts: readonly PartRow[]): number | undefined {
  const samples: number[] = [];
  let stepStart: number | undefined;
  for (const p of parts) {
    const type = (JSON.parse(p.data) as { type?: string }).type;
    if (type === "step-start") {
      stepStart = p.time_created;
    } else if (stepStart !== undefined && (type === "reasoning" || type === "text")) {
      samples.push(p.time_created - stepStart);
      stepStart = undefined;
    }
  }
  if (samples.length === 0) return undefined;
  return samples.reduce((a, b) => a + b, 0) / samples.length;
}

export async function* scanMimo(range: DateRange): AsyncGenerator<UsageEvent> {
  const path = `${process.env.HOME}/.local/share/mimocode/mimocode.db`;
  let db: Database;
  try {
    db = new Database(path, { readonly: true });
  } catch (e) {
    throw new Error(`cannot open mimo db at ${path}: ${e instanceof Error ? e.message : e}`);
  }

  try {
    const msgs = db
      .query(
        `SELECT id, session_id, time_created, data FROM message
         WHERE json_extract(data, '$.role') = 'assistant'
           AND time_created >= ? AND time_created < ?
         ORDER BY time_created`,
      )
      .all(range.start, range.end) as MsgRow[];
    if (msgs.length === 0) return;

    const ids = msgs.map((m) => m.id);
    const placeholders = ids.map(() => "?").join(",");
    const parts = db
      .query(
        `SELECT message_id, time_created, data FROM part
         WHERE message_id IN (${placeholders})
         ORDER BY time_created`,
      )
      .all(...ids) as PartRow[];

    const partsByMsg = new Map<string, PartRow[]>();
    for (const p of parts) {
      let arr = partsByMsg.get(p.message_id);
      if (!arr) {
        arr = [];
        partsByMsg.set(p.message_id, arr);
      }
      arr.push(p);
    }

    for (const row of msgs) {
      const data = JSON.parse(row.data) as {
        tokens?: {
          input?: number;
          output?: number;
          reasoning?: number;
          cache?: { read?: number; write?: number };
        };
        modelID?: string;
        time?: { created?: number; completed?: number };
      };
      const t = data.tokens ?? {};
      const usage: Usage = {
        input: t.input ?? 0,
        output: t.output ?? 0,
        reasoning: t.reasoning ?? 0,
        cache_read: t.cache?.read ?? 0,
        cache_write: t.cache?.write ?? 0,
      };
      const model = data.modelID ?? "unknown";
      const pricing = resolveModel("xiaomi", model);
      let cost = 0;
      if (pricing) cost = calcCost(pricing, usage);
      else warnOnce(model);

      const ts = data.time?.created ?? row.time_created;
      const completed = data.time?.completed;
      const ttft = ttftFromParts(partsByMsg.get(row.id) ?? []);

      yield {
        source: "mimo",
        session: row.session_id,
        model,
        ts,
        usage,
        cost,
        ...(ttft !== undefined && { ttftMs: ttft }),
        ...(completed !== undefined && completed > ts && { durationMs: completed - ts }),
      };
    }
  } finally {
    db.close();
  }
}
