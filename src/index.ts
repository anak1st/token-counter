// token-counter 入口：Commander 解析子命令 → 跑 adapter → 聚合 → 渲染
// daily 已实现；hourly / summary 只预留命令位。

import { createProgram, resolveDailyRange } from "./cli";
import type { BaseOptions, DailyOptions } from "./cli";
import { createStyle } from "./color";
import type { Style } from "./color";
import { scanDsh } from "./adapters/dsh";
import { scanMimo } from "./adapters/mimo";
import { scanZcode } from "./adapters/zcode";
import { scanOpencode } from "./adapters/opencode";
import { aggregate, overall } from "./aggregate";
import { renderOutput } from "./render";
import type { DateRange, Source, UsageEvent } from "./types";

/** 源 → 扫描器注册表；新增数据源在此登记（与 cli 的 AGENTS 保持同步） */
const SCANNERS: Record<Source, (range: DateRange) => AsyncGenerator<UsageEvent>> = {
  mimo: scanMimo,
  dsh: scanDsh,
  zcode: scanZcode,
  opencode: scanOpencode,
};

/** 收齐一个源在区间内的事件；dsh 的 aborted 标记在 turn/end 才回填，不能边读边聚合 */
async function collectEvents(source: Source, range: DateRange): Promise<UsageEvent[]> {
  const events: UsageEvent[] = [];
  for await (const event of SCANNERS[source](range)) events.push(event);
  return events;
}

/** 统一错误出口：错误写 stderr，退出码置 1，不污染 stdout */
function fail(style: Style, error: unknown): void {
  const message = error instanceof Error ? error.message : String(error);
  console.error(style.error(`error: ${message}`));
  process.exitCode = 1;
}

/** daily：先总计、后分组；未知模型只在末尾提示 */
async function runDaily(options: DailyOptions): Promise<void> {
  const style = createStyle(options.color);
  try {
    const range = resolveDailyRange(options);
    const events = await collectEvents(options.agent, range);

    if (events.length === 0) {
      console.log("no usage data in the selected range");
      return;
    }

    const groups = aggregate(events);
    const known = groups.filter((group) => group.pricing !== undefined);
    const unknown = groups
      .filter((group) => group.pricing === undefined)
      .map((group) => `${group.provider}/${group.model}`);

    if (known.length > 0) {
      console.log(renderOutput(overall(known), known, style));
    }
    if (unknown.length > 0) {
      if (known.length > 0) console.log("");
      console.log(style.warning(`unknown models (no CNY pricing): ${unknown.join(", ")}`));
    }
  } catch (error) {
    fail(style, error);
  }
}

/** hourly / summary：命令位已接入，具体输出后续实现 */
async function runReserved(command: "hourly" | "summary", options: BaseOptions): Promise<void> {
  const style = createStyle(options.color);
  fail(style, new Error(`${command} is not implemented yet`));
}

const program = createProgram({
  daily: runDaily,
  hourly: (options) => runReserved("hourly", options),
  summary: (options) => runReserved("summary", options),
});

program.parseAsync(process.argv).catch((error) => {
  console.error(`error: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
