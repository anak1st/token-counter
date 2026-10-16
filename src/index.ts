// token-counter 入口：解析参数 → 跑唯一选中的 adapter → 聚合 → 渲染

import { USAGE, UsageError, parseCli } from "./cli";
import { scanDsh } from "./adapters/dsh";
import { scanMimo } from "./adapters/mimo";
import { scanZcode } from "./adapters/zcode";
import { scanOpencode } from "./adapters/opencode";
import { aggregate } from "./aggregate";
import { render } from "./render";
import type { DateRange, Source, UsageEvent } from "./types";

/** 源 → 扫描器注册表；新增数据源在此登记（与 cli 的源名单保持同步） */
const SCANNERS: Record<Source, (range: DateRange) => AsyncGenerator<UsageEvent>> = {
  mimo: scanMimo,
  dsh: scanDsh,
  zcode: scanZcode,
  opencode: scanOpencode,
};

async function main(): Promise<void> {
  let opts;
  try {
    opts = parseCli(process.argv.slice(2));
  } catch (e) {
    if (e instanceof UsageError) {
      if (e.message) console.error(`error: ${e.message}\n`);
      console.log(USAGE);
      process.exit(e.message ? 1 : 0);
    }
    throw e;
  }

  // 事件先收集完再聚合：dsh 的 aborted 标记在 turn/end 才回填，必须等到全部产出
  const events: UsageEvent[] = [];
  for await (const ev of SCANNERS[opts.source](opts.range)) events.push(ev);

  if (events.length === 0) {
    console.log("no usage data in the selected range");
    return;
  }

  const groups = aggregate(events);

  // 未知模型（查不到人民币价）不出表格，只在末尾列出 provider/model
  const known = groups.filter((g) => g.pricing !== undefined);
  const unknown = groups
    .filter((g) => g.pricing === undefined)
    .map((g) => `${g.provider}/${g.model}`);

  if (known.length > 0) console.log(render(known));
  if (unknown.length > 0) {
    if (known.length > 0) console.log("");
    console.log(`unknown models (no CNY pricing): ${unknown.join(", ")}`);
  }
}

main().catch((e) => {
  console.error(`error: ${e instanceof Error ? e.message : e}`);
  process.exit(1);
});
