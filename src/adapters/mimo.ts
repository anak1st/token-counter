// mimo adapter：只读打开 mimocode.db，从 message 表取 assistant 消息，part 表取时间线
// 速度原料：step-start part 时间 = 流首包时刻 —— TTFT = step-start - 消息创建，
// 生成窗口 = step-start → 流结束（reasoning/text/patch 的更新时间、tool 调用的创建时间），
// 不含工具执行与请求前空档
// 字段映射：modelID/providerID/agent 取消息自带字段；error 非空记 aborted；
// turn 需按用户消息边界推导，暂不填充（留给 turn 模式）

import { Database } from "bun:sqlite";
import type { DateRange, Usage, UsageEvent } from "../types";
import { calcCost, identifyModel } from "../pricing";

/** message 表行（data 为 JSON，见 scanMimo 内的字段类型） */
interface MsgRow {
  id: string;
  session_id: string;
  time_created: number;
  data: string;
}

/** part 表行；time_updated 用于取流式内容的结束时刻 */
interface PartRow {
  message_id: string;
  time_created: number;
  time_updated: number | null;
  data: string;
}

/** 消息内的流时间线：step-start（首包）与流结束时刻；无对应 part 时字段缺省 */
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

/** 只读扫描 mimo 库，惰性产出区间内的 assistant 消息事件 */
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
        providerID?: string;
        agent?: string;
        error?: unknown;
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
      // 零用量消息（如出错未发出的请求）跳过，以免生成空日期行
      const total = usage.input + usage.output + usage.reasoning + usage.cache_read + usage.cache_write;
      if (total === 0) continue;

      const identity = identifyModel(model);
      let cost = 0;
      if (identity.pricing) cost = calcCost(identity.pricing, usage);

      const ts = data.time?.created ?? row.time_created;
      const completed = data.time?.completed;
      const win = streamWindow(partsByMsg.get(row.id) ?? []);

      // TTFT = 消息创建 → 流首包（含请求构建；库不存真正的流式首包）
      const ttft = win.start !== undefined ? win.start - ts : undefined;

      // 生成窗口优先取流时间线；part 时间线缺失时退回消息生命周期（含工具执行，欠准）
      let genMs: number | undefined;
      if (win.start !== undefined && win.end !== undefined && win.end > win.start) {
        genMs = win.end - win.start;
      } else if (completed !== undefined && completed > ts) {
        genMs = completed - ts;
      }

      const event: UsageEvent = {
        source: "mimo",
        session: row.session_id,
        model: identity.name,
        ts,
        usage,
        cost,
      };
      if (identity.pricing) event.pricing = identity.pricing;
      if (data.providerID !== undefined) event.provider = data.providerID;
      if (data.agent !== undefined) event.agent = data.agent;
      if (data.error != null) event.aborted = true;
      if (ttft !== undefined && ttft >= 0) event.ttftMs = ttft;
      if (genMs !== undefined && genMs > 0) event.durationMs = genMs;
      yield event;
    }
  } finally {
    db.close();
  }
}
