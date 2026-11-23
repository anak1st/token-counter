// agy（Antigravity）adapter：扫描 ~/.gemini/antigravity/conversations/*.db
// 主源：steps（step_type = 15 对应 PLANNER_RESPONSE）+ gen_metadata（模型 ID 与耗时）。
// token 口径：f2 为未命中输入（不含 cache_read，计价无需扣减），f5 为 cache_read，
// f9（reasoning）与 f10（非思考输出）互斥且 f9 + f10 === f3（总 output）。
// 速度原料：gen_metadata.f1.11（TTFT）与 f1.12（纯生成窗口）；缺失时退回 steps 的 f6 - f1 与 f7 - f6。
// 只读打开策略：有非空 -wal 时走普通只读以读取增量；已关闭无 -wal 的库走 ?immutable=1 绕过缺少 -shm 的报错。

import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { Database } from "bun:sqlite";
import type { DateRange, Usage, UsageEvent } from "../types";
import { calcCost, identifyModel, type Cost } from "../pricing";

const STEP_TYPE_PLANNER = 15;
const STEP_STATUS_DONE = 3;

interface StepRow {
  idx: number;
  status: number;
  metadata: Uint8Array | null;
}

interface GenRow {
  idx: number;
  data: Uint8Array | null;
}

interface MetaRow {
  cascade_id: string | null;
}

/** 轻量 protobuf 字段条目（仅保留 wire 解析所需的 varint 与字节片段） */
interface WireField {
  field: number;
  varint?: bigint;
  bytes?: Uint8Array;
}

interface ParsedUsage {
  input: number;
  output: number;
  reasoning: number;
  cacheRead: number;
}

interface GenInfo {
  model: string;
  lastStepIndex?: number;
  ttftMs?: number;
  durationMs?: number;
  usage?: ParsedUsage;
}

const decoder = new TextDecoder();

/** 顺序扫描 protobuf 二进制流，返回顶层字段列表；遇到异常 wire 类型则中止当前消息 */
function decodeWire(buf: Uint8Array): WireField[] {
  const fields: WireField[] = [];
  let pos = 0;

  while (pos < buf.length) {
    let tag = 0n;
    let shift = 0n;
    while (pos < buf.length) {
      const b = buf[pos++]!;
      tag |= BigInt(b & 0x7f) << shift;
      if ((b & 0x80) === 0) break;
      shift += 7n;
    }

    const field = Number(tag >> 3n);
    const wire = Number(tag & 7n);
    if (field <= 0) break;

    if (wire === 0) {
      let val = 0n;
      let valShift = 0n;
      while (pos < buf.length) {
        const b = buf[pos++]!;
        val |= BigInt(b & 0x7f) << valShift;
        if ((b & 0x80) === 0) break;
        valShift += 7n;
      }
      fields.push({ field, varint: val });
    } else if (wire === 1) {
      if (pos + 8 > buf.length) break;
      pos += 8;
    } else if (wire === 2) {
      let len = 0;
      let lenShift = 0;
      while (pos < buf.length) {
        const b = buf[pos++]!;
        len |= (b & 0x7f) << lenShift;
        if ((b & 0x80) === 0) break;
        lenShift += 7;
      }
      if (len < 0 || pos + len > buf.length) break;
      fields.push({ field, bytes: buf.subarray(pos, pos + len) });
      pos += len;
    } else if (wire === 5) {
      if (pos + 4 > buf.length) break;
      pos += 4;
    } else {
      break;
    }
  }

  return fields;
}

function getBytes(fields: readonly WireField[], num: number): Uint8Array | undefined {
  for (const item of fields) {
    if (item.field === num && item.bytes !== undefined) return item.bytes;
  }
  return undefined;
}

function getString(fields: readonly WireField[], num: number): string | undefined {
  const bytes = getBytes(fields, num);
  if (!bytes) return undefined;
  return decoder.decode(bytes);
}

function getNumber(fields: readonly WireField[], num: number): number | undefined {
  for (const item of fields) {
    if (item.field === num && item.varint !== undefined) return Number(item.varint);
  }
  return undefined;
}

/** 解析 protobuf Timestamp / Duration（f1=seconds, f2=nanos）为毫秒 */
function parseTimeMs(bytes: Uint8Array | undefined): number | undefined {
  if (!bytes) return undefined;
  const fields = decodeWire(bytes);
  const seconds = getNumber(fields, 1) ?? 0;
  const nanos = getNumber(fields, 2) ?? 0;
  return seconds * 1000 + Math.round(nanos / 1_000_000);
}

/** 解析 usage 子消息：f2=未命中输入，f3=总输出，f5=缓存命中，f9=思考，f10=非思考输出 */
function parseUsage(bytes: Uint8Array | undefined): ParsedUsage | undefined {
  if (!bytes) return undefined;
  const fields = decodeWire(bytes);
  const input = getNumber(fields, 2) ?? 0;
  const totalOut = getNumber(fields, 3) ?? 0;
  const cacheRead = getNumber(fields, 5) ?? 0;
  const reasoning = getNumber(fields, 9);
  const directOut = getNumber(fields, 10);

  // 仅当 f9 + f10 精确等于 f3 时拆分思考 token，否则全部留在 output 内保持互斥不变量
  if (reasoning !== undefined && directOut !== undefined && reasoning + directOut === totalOut) {
    return {
      input,
      output: directOut,
      reasoning,
      cacheRead,
    };
  }
  return {
    input,
    output: totalOut,
    reasoning: 0,
    cacheRead,
  };
}

/** 解析单条 gen_metadata 记录，提取模型 ID、关联 step 索引、TTFT 与生成时长 */
function parseGenMetadata(buf: Uint8Array): GenInfo {
  const outer = decodeWire(buf);
  const innerBytes = getBytes(outer, 1);
  if (!innerBytes) return { model: "unknown" };

  const inner = decodeWire(innerBytes);
  const modelId = getString(inner, 19);
  const displayName = getString(inner, 21);

  const info: GenInfo = {
    model: modelId ?? displayName ?? "unknown",
  };

  const ttftMs = parseTimeMs(getBytes(inner, 11));
  if (ttftMs !== undefined && ttftMs >= 0) {
    info.ttftMs = ttftMs;
  }
  const durationMs = parseTimeMs(getBytes(inner, 12));
  if (durationMs !== undefined && durationMs > 0) {
    info.durationMs = durationMs;
  }
  const usage = parseUsage(getBytes(inner, 4));
  if (usage) {
    info.usage = usage;
  }

  for (const item of inner) {
    if (item.field !== 20 || !item.bytes) continue;
    const kv = decodeWire(item.bytes);
    const key = getString(kv, 1);
    const val = getString(kv, 2);
    if (key === "last_step_index" && val !== undefined) {
      const n = Number(val);
      if (Number.isInteger(n)) info.lastStepIndex = n;
    }
  }

  return info;
}

/** 只读打开单个会话库；活跃库有 -wal 时走常规只读，已关闭无 -wal 时走 immutable=1 */
function openConversationDb(dbPath: string): Database {
  const walPath = `${dbPath}-wal`;
  let hasWal = false;
  try {
    hasWal = existsSync(walPath) && statSync(walPath).size > 0;
  } catch {
    hasWal = false;
  }

  if (hasWal) {
    return new Database(dbPath, { readonly: true });
  }
  return new Database(`file:${dbPath}?immutable=1`, { readonly: true });
}

/** 取数据库及 WAL 文件的最近修改时间，用于区间前剪枝 */
function latestMtimeMs(dbPath: string): number {
  let mtime = statSync(dbPath).mtimeMs;
  const walPath = `${dbPath}-wal`;
  if (existsSync(walPath)) {
    const walMtime = statSync(walPath).mtimeMs;
    if (walMtime > mtime) mtime = walMtime;
  }
  return mtime;
}

/** 扫描单个会话库，按时间过滤并惰性产出归一化事件 */
function* scanConversation(
  dbPath: string,
  fallbackSession: string,
  range: DateRange,
): Generator<UsageEvent> {
  let db: Database;
  try {
    db = openConversationDb(dbPath);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    console.warn(`warning: cannot open agy db ${dbPath}: ${msg}`);
    return;
  }

  try {
    let session = fallbackSession;
    try {
      const meta = db.query("SELECT cascade_id FROM trajectory_meta LIMIT 1").get() as MetaRow | null;
      if (meta?.cascade_id) session = meta.cascade_id;
    } catch {
      // 缺表或半成品空库静默回退文件名
    }

    let steps: StepRow[];
    let gens: GenRow[];
    try {
      steps = db
        .query("SELECT idx, status, metadata FROM steps WHERE step_type = ? ORDER BY idx")
        .all(STEP_TYPE_PLANNER) as StepRow[];
      gens = db.query("SELECT idx, data FROM gen_metadata ORDER BY idx").all() as GenRow[];
    } catch {
      return;
    }

    const genByStepIdx = new Map<number, GenInfo>();
    const genList: GenInfo[] = [];
    for (const g of gens) {
      if (!g.data) continue;
      const parsed = parseGenMetadata(g.data);
      genList.push(parsed);
      if (parsed.lastStepIndex !== undefined) {
        genByStepIdx.set(parsed.lastStepIndex + 1, parsed);
      }
    }

    for (let i = 0; i < steps.length; i++) {
      const step = steps[i]!;
      if (!step.metadata) continue;
      const metaFields = decodeWire(step.metadata);

      const ts = parseTimeMs(getBytes(metaFields, 1));
      if (ts === undefined || ts < range.start || ts >= range.end) continue;

      const gen = genByStepIdx.get(step.idx) ?? genList[i];
      const parsedUsage = parseUsage(getBytes(metaFields, 9)) ?? gen?.usage;
      if (!parsedUsage) continue;

      const usage: Usage = {
        input: parsedUsage.input,
        output: parsedUsage.output,
        reasoning: parsedUsage.reasoning,
        cache_read: parsedUsage.cacheRead,
        cache_write: 0,
      };
      const total =
        usage.input + usage.output + usage.reasoning + usage.cache_read + usage.cache_write;
      if (total === 0) continue;

      const modelRaw = gen?.model ?? "unknown";
      const identity = identifyModel(modelRaw);
      let cost: Cost = {};
      if (identity.pricing) cost = calcCost(identity.pricing, usage);

      const streamStart = parseTimeMs(getBytes(metaFields, 6));
      const streamEnd = parseTimeMs(getBytes(metaFields, 7));

      let ttftMs = gen?.ttftMs;
      if (ttftMs === undefined && streamStart !== undefined && streamStart >= ts) {
        ttftMs = streamStart - ts;
      }

      let durationMs = gen?.durationMs;
      if (
        durationMs === undefined &&
        streamStart !== undefined &&
        streamEnd !== undefined &&
        streamEnd > streamStart
      ) {
        durationMs = streamEnd - streamStart;
      }

      const event: UsageEvent = {
        source: "agy",
        session,
        model: identity.name,
        provider: "google",
        ts,
        usage,
        cost,
      };
      if (identity.pricing) event.pricing = identity.pricing;
      if (step.status !== STEP_STATUS_DONE) event.aborted = true;
      if (ttftMs !== undefined) event.ttftMs = ttftMs;
      if (durationMs !== undefined && durationMs > 0) event.durationMs = durationMs;
      yield event;
    }
  } finally {
    db.close();
  }
}

/** 遍历 ~/.gemini/antigravity/conversations/*.db，产出区间内的模型调用事件 */
export async function* scanAgy(range: DateRange): AsyncGenerator<UsageEvent> {
  const root = `${process.env.HOME}/.gemini/antigravity/conversations`;
  let files: string[];
  try {
    files = readdirSync(root);
  } catch {
    throw new Error(`agy conversations dir not found: ${root}`);
  }

  for (const name of files) {
    if (!name.endsWith(".db")) continue;
    const dbPath = join(root, name);
    try {
      if (latestMtimeMs(dbPath) < range.start) continue;
    } catch {
      continue;
    }
    const fallbackSession = name.slice(0, -3);
    yield* scanConversation(dbPath, fallbackSession, range);
  }
}
