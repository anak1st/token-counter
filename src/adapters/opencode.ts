// opencode adapter：只读打开 ~/.local/share/opencode/opencode.db，从 session_message 取 assistant 消息
// 一条 assistant 消息 = 一次模型请求；tokens.input 不含 cache read，计价无需扣减
// 速度原料：首个 reasoning part 的 time.created − 消息 time.created = TTFT（近似）；
// time.streamed − 首个 part 的 time.created = 纯解码窗口（streamed 是流结束时刻，不含工具执行）。
// 字段映射：model.id 用于计价、model.providerID 作分组（网关名）、agent 取消息自带；
// finish=error 或 error 非空记 aborted；无显式 turn。

import { Database } from "bun:sqlite";
import type { DateRange, Usage, UsageEvent } from "../types";
import { calcCost, identifyModel } from "../pricing";

/** assistant 消息的 data（只列 adapter 用到的字段） */
interface MessageData {
  time?: { created?: number; streamed?: number; completed?: number };
  agent?: string;
  model?: { id?: string; providerID?: string; variant?: string };
  content?: { type?: string; time?: { created?: number; completed?: number } }[];
  finish?: string;
  error?: unknown;
  tokens?: {
    input?: number;
    output?: number;
    reasoning?: number;
    cache?: { read?: number; write?: number };
  };
}

/** session_message 表行（列名同库） */
interface MessageRow {
  session_id: string;
  time_created: number;
  data: string;
}

/** 首个 reasoning part 的创建时刻 = 流起点（text part 无时间戳，只有 reasoning 带） */
function firstPartTime(parts: MessageData["content"]): number | undefined {
  if (!parts) return undefined;
  let first: number | undefined;
  for (const p of parts) {
    if (p.type !== "reasoning") continue;
    const t = p.time?.created;
    if (t === undefined) continue;
    first = first === undefined ? t : Math.min(first, t);
  }
  return first;
}

/** 只读扫描 session_message，惰性产出区间内的 assistant 请求事件 */
export async function* scanOpencode(range: DateRange): AsyncGenerator<UsageEvent> {
  const path = `${process.env.HOME}/.local/share/opencode/opencode.db`;
  let db: Database;
  try {
    db = new Database(path, { readonly: true });
  } catch (e) {
    throw new Error(`cannot open opencode db at ${path}: ${e instanceof Error ? e.message : e}`);
  }

  try {
    const rows = db
      .query(
        `SELECT session_id, time_created, data FROM session_message
         WHERE type = 'assistant' AND time_created >= ? AND time_created < ?
         ORDER BY time_created`,
      )
      .all(range.start, range.end) as MessageRow[];

    for (const row of rows) {
      const data = JSON.parse(row.data) as MessageData;
      const t = data.tokens;
      // 无 tokens 的行是进行中/无用量（如空 finish），跳过以免生成空日期行
      if (!t) continue;

      const usage: Usage = {
        input: t.input ?? 0,
        output: t.output ?? 0,
        reasoning: t.reasoning ?? 0,
        cache_read: t.cache?.read ?? 0,
        cache_write: t.cache?.write ?? 0,
      };
      const total = usage.input + usage.output + usage.reasoning + usage.cache_read + usage.cache_write;
      if (total === 0) continue;

      const identity = identifyModel(data.model?.id ?? "unknown");
      let cost = 0;
      if (identity.pricing) cost = calcCost(identity.pricing, usage);

      const ts = data.time?.created ?? row.time_created;
      const first = firstPartTime(data.content);

      const event: UsageEvent = {
        source: "opencode",
        session: row.session_id,
        model: identity.name,
        ts,
        usage,
        cost,
      };
      if (identity.pricing) event.pricing = identity.pricing;
      if (data.model?.providerID !== undefined) event.provider = data.model.providerID;
      if (data.agent !== undefined) event.agent = data.agent;
      if (data.finish === "error" || data.error != null) event.aborted = true;
      // TTFT ≈ 消息创建 → 首个 reasoning part（近似：含请求构建，库不存流式首包）
      if (first !== undefined && first >= ts) event.ttftMs = first - ts;
      // 纯解码窗口 = 首个 part → 流结束（streamed），不含 TTFT 与工具执行
      const streamed = data.time?.streamed;
      if (first !== undefined && streamed !== undefined && streamed > first) {
        event.durationMs = streamed - first;
      }
      yield event;
    }
  } finally {
    db.close();
  }
}
