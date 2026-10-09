// 价格表（内置常量，单位：元 / 百万 token，人民币）
// 规则：不区分峰谷，统一按高峰价计算（使用时段以工作时间为主）；cache_write 按 input_cache_miss 价并入输入。
// 源自带的 cost 一律不采信、不换汇——费用一律在入口用本表结算（依据见 docs/mimo.md）。

export interface ModelPricing {
  /** 未命中缓存的输入价 */
  input_cache_miss: number;
  /** 命中缓存的输入价 */
  input_cache_hit: number;
  /** 输出价（reasoning 按此价计） */
  output: number;
  /** 历史模型名，计费前归一到本模型 */
  aliases?: readonly string[];
}

export interface ProviderPricing {
  currency: "CNY";
  models: Readonly<Record<string, ModelPricing>>;
}

export interface PricingTable {
  providers: Readonly<Record<string, ProviderPricing>>;
}

/** 按 provider + 模型名查价，先精确匹配再走 aliases 归一 */
export function resolveModel(provider: string, model: string): ModelPricing | undefined {
  const p = PRICING.providers[provider];
  if (!p) return undefined;
  const direct = p.models[model];
  if (direct) return direct;
  for (const m of Object.values(p.models)) {
    if (m.aliases?.includes(model)) return m;
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
  providers: {
    deepseek: {
      currency: "CNY",
      models: {
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
    },
    xiaomi: {
      currency: "CNY",
      // 源数据 cache_write 全为 0，按 miss 价并入的口径未实测验证（见 docs/mimo.md）
      models: {
        "mimo-v2.6-flash": {
          input_cache_miss: 1.0,
          input_cache_hit: 0.02,
          output: 2.0,
        },
      },
    },
    zhipu: {
      currency: "CNY",
      models: {
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
    },
  },
};
