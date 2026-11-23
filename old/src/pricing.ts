// 价格表（内置常量，单位：美元 / 百万 token，人民币 / 百万 token）
// 规则：以官方美元价为基准，同时在厂商公布官方人民币价时并列记录；不进行汇率换算，不采信源里带的 cost。
// 不区分峰谷，统一按高峰/标准价计算；cache_write 按 input_cache_miss 价并入输入。
// 模型名归一：价目表的键就是统一模型名（canonical），aliases 是同一模型的历史名 / 渠道名 / 免费档；
// 免费档等别名逐个写在 aliases 里，不做后缀推断。查价先精确匹配键、再走 aliases；分组与展示都用统一名。

/** 单个币种的三档单价（per 1M tokens） */
export interface PriceTier {
  /** 未命中缓存的输入价 */
  input_cache_miss: number;
  /** 命中缓存的输入价 */
  input_cache_hit: number;
  /** 输出价（reasoning 按此价计） */
  output: number;
}

/** 模型的双币种定价；厂商仅公布美元价时 cny 缺省 */
export interface ModelPricing {
  /** 官方美元价 */
  usd?: PriceTier;
  /** 官方人民币价（仅在厂商提供国内人民币定价时填写，不换汇） */
  cny?: PriceTier;
  /** 同一模型的历史名 / 渠道名 / 免费档，归一到本价目表的键（统一名）；逐个显式列出 */
  aliases?: readonly string[];
}

/** 入口结算出的双币种费用；未提供对应币种官方价时字段缺省 */
export interface Cost {
  usd?: number;
  cny?: number;
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

/** 按单个币种单价表结算费用 */
function calcTierCost(
  tier: PriceTier,
  u: {
    input: number;
    output: number;
    reasoning: number;
    cache_read: number;
    cache_write: number;
  },
): number {
  return (
    (u.input + u.cache_write) * tier.input_cache_miss +
    u.cache_read * tier.input_cache_hit +
    (u.output + u.reasoning) * tier.output
  ) / 1_000_000;
}

/** 入口结算：分别计算官方美元与官方人民币费用；不换汇 */
export function calcCost(
  p: ModelPricing,
  u: {
    input: number;
    output: number;
    reasoning: number;
    cache_read: number;
    cache_write: number;
  },
): Cost {
  const cost: Cost = {};
  if (p.usd) cost.usd = calcTierCost(p.usd, u);
  if (p.cny) cost.cny = calcTierCost(p.cny, u);
  return cost;
}

export const PRICING: PricingTable = {
  deepseek: {
    // 统一名体现最新版本 deepseek-v4.1-flash；无版本名、v4 名与免费档都归一到此
    // 官方高峰时段价：api-docs.deepseek.com（USD 与 CNY 均取高峰档）
    "deepseek-v4.1-flash": {
      usd: {
        input_cache_miss: 0.30,
        input_cache_hit: 0.006,
        output: 1.20,
      },
      cny: {
        input_cache_miss: 2.0,
        input_cache_hit: 0.04,
        output: 8.0,
      },
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
    // 官方实时 API 价：海外美元 vs 大陆人民币
    "mimo-v2.6-flash": {
      usd: {
        input_cache_miss: 0.14,
        input_cache_hit: 0.0028,
        output: 0.28,
      },
      cny: {
        input_cache_miss: 1.0,
        input_cache_hit: 0.02,
        output: 2.0,
      },
    },
  },
  zhipu: {
    // 官方价：国际站 Z.ai（USD）vs 国内 BigModel.cn（CNY）
    "GLM-5.3": {
      usd: {
        input_cache_miss: 1.40,
        input_cache_hit: 0.26,
        output: 4.40,
      },
      cny: {
        input_cache_miss: 8.0,
        input_cache_hit: 2.0,
        output: 28.0,
      },
      aliases: ["glm-5.3"],
    },
    "GLM-5.3-Flash": {
      usd: {
        input_cache_miss: 0.15,
        input_cache_hit: 0.03,
        output: 0.50,
      },
      cny: {
        input_cache_miss: 0.8,
        input_cache_hit: 0.23,
        output: 2.8,
      },
      aliases: [
        "glm-5.3-flash",
        // 免费档是同一模型的不同计费渠道，显式列在这里，不做后缀推断
        "glm-5.3-flash-free",
      ],
    },
  },
  google: {
    // Google AI 官方美元价（ai.google.dev/gemini-api/docs/pricing）；无官方人民币价，不换汇
    // 有限期促销/首发价的模型按项目惯例统一用常规价结算，首发价仅在注释与 docs/agy.md 中备查
    "gemini-4-hydrogen": {
      usd: {
        input_cache_miss: 1.50,
        input_cache_hit: 0.075,
        output: 7.50,
      },
      aliases: [
        "gemini-4-hydrogen-low",
        "gemini-4-hydrogen-medium",
        "gemini-4-hydrogen-high",
        "Gemini 4 Hydrogen",
        "Gemini 4 Hydrogen (Low)",
        "Gemini 4 Hydrogen (Medium)",
        "Gemini 4 Hydrogen (High)",
      ],
    },
    "gemini-4-neon": {
      usd: {
        input_cache_miss: 3.00,
        input_cache_hit: 0.15,
        output: 15.00,
      },
      aliases: [
        "gemini-4-neon-low",
        "gemini-4-neon-medium",
        "gemini-4-neon-high",
        "Gemini 4 Neon",
        "Gemini 4 Neon (Low)",
        "Gemini 4 Neon (Medium)",
        "Gemini 4 Neon (High)",
      ],
    },
    // 常规价（2026-12-01 起）；2026-11-30 前首发优惠为 $2.00 / $0.10 / $10.00
    "gemini-4-argon": {
      usd: {
        input_cache_miss: 4.00,
        input_cache_hit: 0.20,
        output: 20.00,
      },
      aliases: [
        "gemini-4-argon-low",
        "gemini-4-argon-medium",
        "gemini-4-argon-high",
        "Gemini 4 Argon",
        "Gemini 4 Argon (Low)",
        "Gemini 4 Argon (Medium)",
        "Gemini 4 Argon (High)",
      ],
    },
    // 常规价（2026-12-01 起）；2026-11-30 前首发优惠为 $2.00 / $0.10 / $10.00
    "gemini-4-carbon": {
      usd: {
        input_cache_miss: 4.00,
        input_cache_hit: 0.20,
        output: 20.00,
      },
      aliases: [
        "gemini-4-carbon-low",
        "gemini-4-carbon-medium",
        "gemini-4-carbon-high",
        "Gemini 4 Carbon",
        "Gemini 4 Carbon (Low)",
        "Gemini 4 Carbon (Medium)",
        "Gemini 4 Carbon (High)",
      ],
    },
    // 常规价（2027-01-01 起）；2026-12-31 前首发优惠为 $0.75 / $0.075 / $3.75；3.7 Flash 同价
    "gemini-3.8-flash": {
      usd: {
        input_cache_miss: 1.50,
        input_cache_hit: 0.15,
        output: 7.50,
      },
      aliases: [
        "gemini-3.7-flash",
        "gemini-3.8-flash-low",
        "gemini-3.8-flash-medium",
        "gemini-3.8-flash-high",
        "Gemini 3.8 Flash",
        "Gemini 3.8 Flash (Low)",
        "Gemini 3.8 Flash (Medium)",
        "Gemini 3.8 Flash (High)",
      ],
    },
    "gemini-3.5-flash-lite": {
      usd: {
        input_cache_miss: 0.30,
        input_cache_hit: 0.03,
        output: 2.50,
      },
      aliases: [
        "Gemini 3.5 Flash-Lite",
      ],
    },
    // <=200k 上下文常规价
    "gemini-3.1-pro-preview": {
      usd: {
        input_cache_miss: 2.00,
        input_cache_hit: 0.20,
        output: 12.00,
      },
      aliases: [
        "gemini-3.1-pro",
        "Gemini 3.1 Pro",
        "Gemini 3.1 Pro (High)",
      ],
    },
  },
};
