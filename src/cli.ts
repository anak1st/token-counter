// CLI 参数定义：Commander 负责子命令、帮助与基础校验。
// 子命令：daily（按天，必须给日期区间）、hourly / summary（预留，只接受 --agent）。
// 日期区间与 reserved 命令的语义校验在 resolveDailyRange / handlers 中完成。

import { Command, Option } from "commander";
import type { ColorMode } from "./color";
import type { DateRange, Source } from "./types";
import { lastNDays, nextDayStart, parseDate } from "./time";

/** --agent 的合法取值；与 SCANNERS 注册表保持一致 */
const AGENTS: readonly Source[] = ["mimo", "dsh", "zcode", "opencode"];

export interface BaseOptions {
  agent: Source;
  color: ColorMode;
}

export interface DailyOptions extends BaseOptions {
  last?: string;
  start?: string;
  end?: string;
}

/** 子命令的实际执行逻辑；解析只负责把选项交出去 */
export interface CliHandlers {
  daily(options: DailyOptions): Promise<void>;
  hourly(options: BaseOptions): Promise<void>;
  summary(options: BaseOptions): Promise<void>;
}

/** --agent：必填，值域由 Commander 校验 */
function agentOption(): Option {
  return new Option("--agent <name>", "agent app to read")
    .choices([...AGENTS])
    .makeOptionMandatory();
}

/** --color：auto 跟随终端，always / never 显式覆盖 */
function colorOption(): Option {
  return new Option("--color <when>", "color output")
    .choices(["auto", "always", "never"])
    .default("auto");
}

/** 构建 Commander 命令树；index 负责注入三个 handler */
export function createProgram(handlers: CliHandlers): Command {
  const program = new Command();
  program
    .name("token-counter")
    .description("token usage and cost for AI coding agents")
    .showHelpAfterError()
    .showSuggestionAfterError();

  program.addHelpText(
    "after",
    `
Examples:
  $ token-counter daily --agent dsh --last 7
  $ token-counter daily --agent opencode --start 2026-10-01 --end 2026-10-08
  $ token-counter daily --agent dsh --last 7 --color never`,
  );

  program
    .command("daily")
    .description("daily breakdown for one agent")
    .addOption(agentOption())
    .addOption(colorOption())
    .option("--last <n>", "last N days including today")
    .option("--start <date>", "start date (YYYY-MM-DD, inclusive)")
    .option("--end <date>", "end date (YYYY-MM-DD, inclusive)")
    .action((options: DailyOptions) => handlers.daily(options));

  program
    .command("hourly")
    .description("hour-of-day breakdown (reserved)")
    .addOption(agentOption())
    .addOption(colorOption())
    .action((options: BaseOptions) => handlers.hourly(options));

  program
    .command("summary")
    .description("all-time summary (reserved)")
    .addOption(agentOption())
    .addOption(colorOption())
    .action((options: BaseOptions) => handlers.summary(options));

  return program;
}

/** daily 的日期区间：--last 与 --start/--end 二选一；--end 含当天 */
export function resolveDailyRange(options: DailyOptions): DateRange {
  const { last, start, end } = options;
  if (last !== undefined && (start !== undefined || end !== undefined)) {
    throw new Error("use either --last or --start/--end, not both");
  }

  if (last !== undefined) {
    const n = Number(last);
    if (!Number.isInteger(n) || n < 1) {
      throw new Error("--last expects a positive integer");
    }
    return lastNDays(n);
  }

  if (start === undefined || end === undefined) {
    if (start === undefined && end === undefined) {
      throw new Error("date range required: use --last N or --start/--end");
    }
    throw new Error("--start and --end must be used together");
  }

  let startMs: number;
  let endMs: number;
  try {
    startMs = parseDate(start);
    endMs = parseDate(end);
  } catch (e) {
    throw new Error(e instanceof Error ? e.message : String(e));
  }
  if (startMs > endMs) {
    throw new Error("--start must be earlier than or equal to --end");
  }
  return { start: startMs, end: nextDayStart(endMs) };
}
