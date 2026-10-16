// dsh adapter：扫描 ~/.dsh/sessions 下的 zstd JSONL 事件流
// 只解析最新格式（*.v4.jsonl.zstd）：usage 在 assistant/message 的 data.usage；
// 旧格式（session.jsonl.zstd / .v3.）静默跳过，不解析不告警。
// 速度原料：assistant/message.data.stream[] 分块时间戳 —— 首块 - step/start = TTFT，
// 末块 - 首块 = 纯解码窗口（不含工具执行与 TTFT）；无 stream 时退回 step 跨度。
// 字段映射：turn 取 data.turn；aborted 由 turn/end 的 reason.kind 在 turn 收尾时回填；
// provider/模型从 request/context 或 request/header 取（会话内可能变化），计价按模型名结算 cost。

import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import type { DateRange, Usage, UsageEvent } from "../types";
import { calcCost, resolveModel } from "../pricing";

/** 单步的起止时刻（step/start 记 start，step/end 记 end） */
interface StepState {
  start?: number;
  end?: number;
}

interface FileState {
  sessionId: string;
  provider: string;
  model: string;
  /** 活跃 step 的起止（key = turn:step） */
  steps: Map<string, StepState>;
  /** 等待 step/end 的事件（按 step key 分组，缺 stream 时间戳的兜底路径） */
  pending: Map<string, UsageEvent[]>;
  /** 活跃 turn 的事件引用（turn/end 时补 aborted 标记后释放） */
  openTurns: Map<string, UsageEvent[]>;
}

/** step 定位键（turn:step）；事件缺任一字段则不参与 step 关联 */
function stepKey(data: { turn?: number; step?: number } | undefined): string | undefined {
  if (data?.turn === undefined || data?.step === undefined) return undefined;
  return `${data.turn}:${data.step}`;
}

/** usage → 归一化事件：按模型名结算 cost 与 pricing */
function buildEvent(
  state: FileState,
  ts: number,
  u: Record<string, unknown>,
): UsageEvent {
  const usage: Usage = {
    input: Number(u.inputTokens ?? 0),
    output: Number(u.outputTokens ?? 0),
    reasoning: Number(u.reasoningTokens ?? 0),
    cache_read: Number(u.cacheReadTokens ?? 0),
    cache_write: Number(u.cacheWriteTokens ?? 0),
  };
  const pricing = resolveModel(state.model);
  let cost = 0;
  if (pricing) cost = calcCost(pricing, usage);
  const event: UsageEvent = {
    source: "dsh",
    session: state.sessionId,
    provider: state.provider,
    model: state.model,
    ts,
    usage,
    cost,
  };
  if (pricing) event.pricing = pricing;
  return event;
}

/** step 结束/收尾：给缺时间字段的事件补上 ttft 与 duration（stream 缺失时的兜底）
 * 原地补字段而不复制——openTurns 持有同一引用，aborted 回填要能落到最终产出的事件上 */
function flushStep(state: FileState, key: string): UsageEvent[] {
  const list = state.pending.get(key);
  if (!list || list.length === 0) return [];
  state.pending.delete(key);

  const step = state.steps.get(key);
  const stepStart = step?.start;
  if (stepStart === undefined) return list;

  const first = list[0]!;
  const firstTs = Math.min(...list.map((e) => e.ts));
  if (first.ttftMs === undefined) first.ttftMs = firstTs - stepStart;
  if (first.durationMs === undefined && step?.end !== undefined) {
    first.durationMs = step.end - stepStart;
  }
  return list;
}

/** 文件收尾：把所有还挂在 step 上的 pending 事件放出去 */
function flushAll(state: FileState): UsageEvent[] {
  const out: UsageEvent[] = [];
  for (const key of [...state.pending.keys()]) out.push(...flushStep(state, key));
  return out;
}

/** 处理一行，返回本次可产出的事件 */
function processLine(
  line: string,
  state: FileState,
  range: DateRange,
  out: UsageEvent[],
): void {
  if (line.length === 0) return;
  let ev: {
    type?: string;
    time?: number;
    seq?: number;
    id?: string;
    version?: number;
    data?: Record<string, unknown>;
  };
  try {
    ev = JSON.parse(line);
  } catch {
    return;
  }

  switch (ev.type) {
    case "session":
      if (typeof ev.id === "string") state.sessionId = ev.id;
      return;
    case "request/context": {
      const d = ev.data as { provider?: string; model?: string } | undefined;
      if (typeof d?.provider === "string") state.provider = d.provider;
      if (typeof d?.model === "string") state.model = d.model;
      return;
    }
    case "request/header": {
      const cfg = (ev.data as { header?: { config?: { provider?: string; model?: string } } } | undefined)
        ?.header?.config;
      if (typeof cfg?.provider === "string") state.provider = cfg.provider;
      if (typeof cfg?.model === "string") state.model = cfg.model;
      return;
    }
    case "turn/end": {
      // turn 收尾：aborted 则回填本 turn 已产出的全部事件，随后释放引用
      const d = ev.data as { turn?: number; reason?: { kind?: string } } | undefined;
      if (d?.turn !== undefined) {
        const key = String(d.turn);
        const list = state.openTurns.get(key);
        if (list !== undefined) {
          if (d.reason?.kind === "aborted") {
            for (const e of list) e.aborted = true;
          }
          state.openTurns.delete(key);
        }
      }
      return;
    }
    case "step/start": {
      const k = stepKey(ev.data as { turn?: number; step?: number });
      if (k !== undefined) state.steps.set(k, { start: ev.time });
      return;
    }
    case "step/end": {
      const k = stepKey(ev.data as { turn?: number; step?: number });
      if (k !== undefined) {
        const step = state.steps.get(k);
        if (step) step.end = ev.time;
        out.push(...flushStep(state, k));
      }
      return;
    }
    default:
      break;
  }

  // usage 事件（v4：assistant/message.data.usage）
  let u: Record<string, unknown> | undefined;
  if (ev.type === "assistant/message") {
    u = (ev.data as { usage?: Record<string, unknown> } | undefined)?.usage;
  }
  if (!u || typeof ev.time !== "number") return;
  if (ev.time < range.start || ev.time >= range.end) return;

  const k = stepKey(ev.data as { turn?: number; step?: number });
  const event = buildEvent(state, ev.time, u);

  // 登记到活跃 turn，turn/end 若为 aborted 会回填标记
  const turn = (ev.data as { turn?: number } | undefined)?.turn;
  if (turn !== undefined) {
    event.turn = String(turn);
    let opened = state.openTurns.get(event.turn);
    if (!opened) {
      opened = [];
      state.openTurns.set(event.turn, opened);
    }
    opened.push(event);
  }

  // stream[] 分块时间戳：首块 - step/start = TTFT，末块 - 首块 = 纯解码窗口
  const stream = (ev.data as { stream?: { time?: unknown }[] } | undefined)?.stream;
  const times: number[] = [];
  if (Array.isArray(stream)) {
    for (const chunk of stream) {
      if (typeof chunk?.time === "number") times.push(chunk.time);
    }
  }
  if (times.length >= 2) {
    const first = Math.min(...times);
    const last = Math.max(...times);
    if (last > first) {
      event.durationMs = last - first;
      const stepStart = k !== undefined ? state.steps.get(k)?.start : undefined;
      if (stepStart !== undefined && first >= stepStart) {
        event.ttftMs = first - stepStart;
      }
    }
  }

  if (k !== undefined && state.steps.has(k) && event.durationMs === undefined) {
    // 无分块时间戳：挂到 pending，等 step/end 用 step 跨度兜底
    let list = state.pending.get(k);
    if (!list) {
      list = [];
      state.pending.set(k, list);
    }
    list.push(event);
  } else {
    out.push(event);
  }
}

/** 解压并逐行解析单个会话文件，惰性产出区间内的 usage 事件 */
async function* scanFile(path: string, filename: string, range: DateRange): AsyncGenerator<UsageEvent> {
  let proc: ReturnType<typeof Bun.spawn>;
  try {
    proc = Bun.spawn(["zstd", "-dc", path], {
      stdout: "pipe",
      stderr: "pipe",
    });
  } catch {
    throw new Error("`zstd` not found in PATH — install it first (brew install zstd)");
  }

  const state: FileState = {
    sessionId: filename,
    provider: "unknown",
    model: "unknown",
    steps: new Map(),
    pending: new Map(),
    openTurns: new Map(),
  };
  const out: UsageEvent[] = [];

  const decoder = new TextDecoder();
  let buf = "";
  for await (const chunk of proc.stdout as AsyncIterable<Uint8Array>) {
    buf += decoder.decode(chunk, { stream: true });
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      processLine(line, state, range, out);
    }
    if (out.length > 0) {
      yield* out.splice(0);
    }
  }
  if (buf.length > 0) processLine(buf, state, range, out);

  const exitCode = await proc.exited;
  if (exitCode !== 0) {
    const stderr = proc.stderr;
    const err =
      stderr && typeof stderr !== "number" ? (await new Response(stderr).text()).trim() : "";
    console.warn(`warning: zstd failed on ${path} (exit ${exitCode})${err ? `: ${err}` : ""}`);
  }

  out.push(...flushAll(state));
  if (out.length > 0) yield* out;
}

/** 遍历 sessions 目录下所有会话，只读 v4 文件；mtime 与区间不相交的直接剪掉 */
export async function* scanDsh(range: DateRange): AsyncGenerator<UsageEvent> {
  const root = `${process.env.HOME}/.dsh/sessions`;
  let projects: string[];
  try {
    projects = readdirSync(root);
  } catch {
    throw new Error(`dsh sessions dir not found: ${root}`);
  }

  for (const proj of projects) {
    const projDir = join(root, proj);
    let sessions: string[];
    try {
      sessions = readdirSync(projDir);
    } catch {
      continue;
    }
    for (const sess of sessions) {
      const dir = join(projDir, sess);
      let files: string[];
      try {
        files = readdirSync(dir);
      } catch {
        continue;
      }
      for (const f of files) {
        // 只读最新格式（v4）；旧格式静默跳过
        if (!f.endsWith(".v4.jsonl.zstd")) continue;
        const path = join(dir, f);
        try {
          // 事件时间不可能晚于文件最后写入时间，剪掉明显不相交的文件
          if (statSync(path).mtimeMs < range.start) continue;
        } catch {
          continue;
        }
        yield* scanFile(path, f, range);
      }
    }
  }
}
