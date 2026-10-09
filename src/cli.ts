// CLI 参数解析：token-counter <mimo|dsh|zcode> (--from/--to | --last N)

import { parseArgs } from "node:util";
import type { DateRange, Source } from "./types";
import { lastNDays, parseDate } from "./time";

export const USAGE = `Usage: token-counter <mimo|dsh|zcode> [date range]

Sources:
  mimo               MiMoCode local SQLite
  dsh                DeepSeek Harness session logs
  zcode              ZCode local SQLite (GLM)

Date range (required, one of):
  --from YYYY-MM-DD  start date (inclusive)
  --to   YYYY-MM-DD  end date (exclusive)
  --last N           last N days including today

Examples:
  token-counter dsh --last 7
  token-counter mimo --from 2026-10-01 --to 2026-10-08
  token-counter zcode --from 2026-08-27 --to 2026-09-05`;

export class UsageError extends Error {}

export interface CliOptions {
  source: Source;
  range: DateRange;
}

function asSource(s: string): Source {
  if (s === "mimo" || s === "dsh" || s === "zcode") return s;
  throw new UsageError(`unknown source "${s}" (expected mimo, dsh or zcode)`);
}

export function parseCli(argv: string[]): CliOptions {
  let values: { from?: string; to?: string; last?: string; help?: boolean };
  let positionals: string[];
  try {
    const r = parseArgs({
      args: argv,
      allowPositionals: true,
      strict: true,
      options: {
        from: { type: "string" },
        to: { type: "string" },
        last: { type: "string" },
        help: { type: "boolean", short: "h" },
      },
    });
    values = r.values;
    positionals = r.positionals;
  } catch (e) {
    throw new UsageError(e instanceof Error ? e.message : String(e));
  }

  if (values.help) throw new UsageError("");
  if (positionals.length !== 1) {
    throw new UsageError("expected exactly one source argument (mimo, dsh or zcode)");
  }
  const src = asSource(positionals[0]!);

  const { from, to, last } = values;
  if (last !== undefined && (from !== undefined || to !== undefined)) {
    throw new UsageError("--last cannot be combined with --from/--to");
  }

  let range: DateRange;
  if (last !== undefined) {
    const n = Number(last);
    if (!Number.isInteger(n) || n < 1) throw new UsageError("--last expects a positive integer");
    range = lastNDays(n);
  } else if (from !== undefined || to !== undefined) {
    let start = 0;
    let end = Number.MAX_SAFE_INTEGER;
    try {
      if (from !== undefined) start = parseDate(from);
      if (to !== undefined) end = parseDate(to);
    } catch (e) {
      throw new UsageError(e instanceof Error ? e.message : String(e));
    }
    if (start >= end) throw new UsageError("--from must be earlier than --to");
    range = { start, end };
  } else {
    throw new UsageError("date range required: use --from/--to or --last N");
  }

  return { source: src, range };
}
