// dsh adapter：扫描 ~/.dsh/sessions 下的 zstd JSONL 事件流
// 只解析最新格式（*.v4.jsonl.zstd）：usage 在 assistant/message 的 data.usage；
// 旧格式（session.jsonl.zstd / .v3.）静默跳过，不解析不告警。
// 速度原料：step/start → 首个 usage 事件 = TTFT；step/end - step/start = 生成耗时

import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import type { DateRange, Usage, UsageEvent } from "../types";
import { calcCost, resolveModel } from "../pricing";

const warned = new Set<string>();

interface StepState {
  start?: number;
  end?: number;
}

interface FileState {
  sessionId: string;
  model: string;
  steps: Map<string, StepState>;
  /** 等待 step/end 的事件（按 step key 分组） */
  pending: Map<string, UsageEvent[]>;
}

function warnOnce(model: string): void {
  if (warned.has(model)) return;
  warned.add(model);
  console.warn(`warning: no CNY pricing for model "${model}" (dsh), cost counted as 0`);
}

function stepKey(data: { turn?: number; step?: number } | undefined): string | undefined {
  if (data?.turn === undefined || data?.step === undefined) return undefined;
  return `${data.turn}:${data.step}`;
}

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
  const pricing = resolveModel("deepseek", state.model);
  let cost = 0;
  if (pricing) cost = calcCost(pricing, usage);
  else warnOnce(state.model);
  return {
    source: "dsh",
    session: state.sessionId,
    model: state.model,
    ts,
    usage,
    cost,
    ...(pricing && { pricing }),
  };
}

/** step 结束/收尾：给该 step 的第一个事件挂上 ttft 与 duration */
function flushStep(state: FileState, key: string): UsageEvent[] {
  const list = state.pending.get(key);
  if (!list || list.length === 0) return [];
  state.pending.delete(key);
  const step = state.steps.get(key);
  if (!step?.start) return list;
  const firstTs = Math.min(...list.map((e) => e.ts));
  const ttft = firstTs - step.start;
  const duration = step.end !== undefined ? step.end - step.start : undefined;
  return list.map((e, i) =>
    i === 0
      ? { ...e, ttftMs: ttft, ...(duration !== undefined && { durationMs: duration }) }
      : e,
  );
}

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
      const model = (ev.data as { model?: string } | undefined)?.model;
      if (typeof model === "string") state.model = model;
      return;
    }
    case "request/header": {
      const cfg = (ev.data as { header?: { config?: { model?: string } } } | undefined)?.header
        ?.config;
      if (typeof cfg?.model === "string") state.model = cfg.model;
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
  if (k !== undefined && state.steps.has(k)) {
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
    model: "unknown",
    steps: new Map(),
    pending: new Map(),
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
