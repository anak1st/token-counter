// 价格表（内置常量，单位：元 / 百万 token，人民币）
// 规则：不区分峰谷，统一按高峰价计算（使用时段以工作时间为主）；cache_write 按 input_cache_miss 价并入输入。
// 查价按模型名平铺匹配（先精确、再走 aliases）；厂商只用于价目表分组。
// 源里带的 cost 一律不采信、不换汇，费用一律用本表结算。

export interface ModelPricing {
  /** 未命中缓存的输入价 */
  input_cache_miss: number;
  /** 命中缓存的输入价 */
  input_cache_hit: number;
  /** 输出价（reasoning 按此价计） */
  output: number;
  /** 历史模型名，查价前归一到本模型 */
  aliases?: readonly string[];
}

/** 价目表：厂商分组 → 模型名 → 价目。厂商只用于组织价目表，不参与查价 */
export interface PricingTable {
  readonly [vendor: string]: Readonly<Record<string, ModelPricing>>;
}

/** 按模型名平铺查价：遍历全部厂商的模型名与 aliases；查不到返回 undefined */
export function resolveModel(model: string): ModelPricing | undefined {
  for (const models of Object.values(PRICING)) {
    const direct = models[model];
    if (direct) return direct;
  }
  for (const models of Object.values(PRICING)) {
    for (const m of Object.values(models)) {
      if (m.aliases?.includes(model)) return m;
    }
  }
  return undefined;
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
    // DeepSeek 2026-09-10 起统一为 deepseek-flash（无版本号），旧名走 aliases
    "deepseek-flash": {
      input_cache_miss: 2.0,
      input_cache_hit: 0.04,
      output: 8.0,
      aliases: [
        "deepseek-v4-flash",
        "deepseek-v4-flash-vision-exp",
        "deepseek-v4.1-flash",
        "deepseek-v4.1-flash-expires-on-0910",
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
      aliases: ["glm-5.3-flash"],
    },
  },
};
