// 价格表（内置常量，单位：元 / 百万 token，人民币）
// 规则：不区分峰谷，统一按高峰价计算（使用时段以工作时间为主）；cache_write 按 input_cache_miss 价并入输入。
// 模型名归一：价目表的键就是统一模型名（canonical），aliases 是同一模型的历史名 / 渠道名 / 免费档；
// 免费档等别名逐个写在 aliases 里，不做后缀推断。查价先精确匹配键、再走 aliases；分组与展示都用统一名。
// 厂商只用于价目表分组；源里带的 cost 一律不采信、不换汇，费用一律用本表结算。

export interface ModelPricing {
  /** 未命中缓存的输入价 */
  input_cache_miss: number;
  /** 命中缓存的输入价 */
  input_cache_hit: number;
  /** 输出价（reasoning 按此价计） */
  output: number;
  /** 同一模型的历史名 / 渠道名 / 免费档，归一到本价目表的键（统一名）；逐个显式列出 */
  aliases?: readonly string[];
}

/** 价目表：厂商分组 → 统一模型名 → 价目。厂商只用于组织价目表，不参与查价 */
export interface PricingTable {
  readonly [vendor: string]: Readonly<Record<string, ModelPricing>>;
}

/** 模型归一结果：name 是统一模型名，命中价目表时才带 pricing */
export interface ModelIdentity {
  name: string;
  pricing?: ModelPricing;
}

/** 单次查表：先匹配价目表的键，再匹配各模型的 aliases；未命中返回 undefined */
function findModel(model: string): ModelIdentity | undefined {
  for (const models of Object.values(PRICING)) {
    const direct = models[model];
    if (direct) return { name: model, pricing: direct };
  }
  for (const models of Object.values(PRICING)) {
    for (const [name, pricing] of Object.entries(models)) {
      if (pricing.aliases?.includes(model)) return { name, pricing };
    }
  }
  return undefined;
}

/** 模型名归一 + 查价：精确键 → aliases；都查不到则保留原名、无价 */
export function identifyModel(model: string): ModelIdentity {
  const direct = findModel(model);
  if (direct) return direct;
  return { name: model };
}

/** 入口结算：人民币费用（元）。input 视为未命中缓存；reasoning 按 output 价；cache_write 按未命中价 */
export function calcCost(p: ModelPricing, u: {
  input: number;
  output: number;
  reasoning: number;
  cache_read: number;
  cache_write: number;
}): number {
  return (
    (u.input + u.cache_write) * p.input_cache_miss +
    u.cache_read * p.input_cache_hit +
    (u.output + u.reasoning) * p.output
  ) / 1_000_000;
}

export const PRICING: PricingTable = {
  deepseek: {
    // 统一名体现最新版本 deepseek-v4.1-flash；无版本名、v4 名与免费档都归一到此
    "deepseek-v4.1-flash": {
      input_cache_miss: 2.0,
      input_cache_hit: 0.04,
      output: 8.0,
      aliases: [
        "deepseek-flash",
        "deepseek-v4-flash",
        "deepseek-v4-flash-vision-exp",
        "deepseek-v4.1-flash-expires-on-0910",
        // 免费档是同一模型的不同计费渠道，显式列在这里，不做后缀推断
        "deepseek-flash-free",
        "deepseek-v4-flash-free",
        "deepseek-v4.1-flash-free",
      ],
    },
  },
  xiaomi: {
    // 源数据 cache_write 全为 0，按 miss 价并入的口径未实测验证
    "mimo-v2.6-flash": {
      input_cache_miss: 1.0,
      input_cache_hit: 0.02,
      output: 2.0,
    },
  },
  zhipu: {
    "GLM-5.3": {
      input_cache_miss: 8.0,
      input_cache_hit: 2.0,
      output: 28.0,
      aliases: ["glm-5.3"],
    },
    "GLM-5.3-Flash": {
      input_cache_miss: 0.8,
      input_cache_hit: 0.23,
      output: 2.8,
      aliases: [
        "glm-5.3-flash",
        // 免费档是同一模型的不同计费渠道，显式列在这里，不做后缀推断
        "glm-5.3-flash-free",
      ],
    },
  },
};
