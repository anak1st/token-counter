// token-counter 入口

import { USAGE, UsageError, parseCli } from "./cli";
import { scanDsh } from "./adapters/dsh";
import { scanMimo } from "./adapters/mimo";
import { aggregate } from "./aggregate";
import { render } from "./render";
import type { DateRange, Source, UsageEvent } from "./types";

const SCANNERS: Record<Source, (range: DateRange) => AsyncGenerator<UsageEvent>> = {
  mimo: scanMimo,
  dsh: scanDsh,
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

  const events: UsageEvent[] = [];
  for await (const ev of SCANNERS[opts.source](opts.range)) events.push(ev);

  if (events.length === 0) {
    console.log("no usage data in the selected range");
    return;
  }
  console.log(render(aggregate(events), opts.source));
}

main().catch((e) => {
  console.error(`error: ${e instanceof Error ? e.message : e}`);
  process.exit(1);
});
