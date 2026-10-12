// mimo adapter：只读打开 mimocode.db，从 message 表取 assistant 消息
// 速度原料：step-start part 时间 = 流首包时刻 —— TTFT = step-start - 消息创建，
// 生成窗口 = step-start → 流结束（reasoning/text/patch 的更新时间、tool 调用的创建时间），
// 不含工具执行与请求前空档

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
  time_updated: number | null;
  data: string;
}

const warned = new Set<string>();

function warnOnce(model: string): void {
  if (warned.has(model)) return;
  warned.add(model);
  console.warn(`warning: no CNY pricing for model "${model}" (mimo), cost counted as 0`);
}

/** 消息内的流时间线：step-start（首包）与流结束时刻 */
function streamWindow(parts: readonly PartRow[]): { start?: number; end?: number } {
  let start: number | undefined;
  let end: number | undefined;
  const bump = (t: number): void => {
    end = end === undefined ? t : Math.max(end, t);
  };
  for (const p of parts) {
    const type = (JSON.parse(p.data) as { type?: string }).type;
    if (type === "step-start") {
      start = start === undefined ? p.time_created : Math.min(start, p.time_created);
    } else if (type === "reasoning" || type === "text" || type === "patch") {
      bump(p.time_updated ?? p.time_created);
    } else if (type === "tool") {
      bump(p.time_created);
    }
  }
  return { start, end };
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
        `SELECT message_id, time_created, time_updated, data FROM part
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
      const win = streamWindow(partsByMsg.get(row.id) ?? []);
      const ttft = win.start !== undefined ? win.start - ts : undefined;
      const genMs =
        win.start !== undefined && win.end !== undefined && win.end > win.start
          ? win.end - win.start
          : completed !== undefined && completed > ts
            ? completed - ts
            : undefined;

      yield {
        source: "mimo",
        session: row.session_id,
        model,
        ts,
        usage,
        cost,
        ...(pricing && { pricing }),
        ...(ttft !== undefined && ttft >= 0 && { ttftMs: ttft }),
        ...(genMs !== undefined && genMs > 0 && { durationMs: genMs }),
      };
    }
  } finally {
    db.close();
  }
}
