//! 模型目录、全小写别名归一与官方美元价结算。
//!
//! 价格表来自 `models.json`：
//! - 规范模型名、厂商名与别名必须全部小写。
//! - 查价时先对输入做去空格与小写归一，优先精确匹配规范键，再查显式 `aliases`（不做模糊推断）。
//! - 全部价格单位为「美元 / 百万 token」；`cache_write` 按 `input_cache_miss` 计价，`reasoning` 按 `output` 计价。

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::model::Usage;

/// 编译期嵌入的默认 `config/models.json`，保证零配置即可运行。
const DEFAULT_MODELS_JSON: &str = include_str!("../config/models.json");

/// 当前支持的 `models.json` 结构版本号。
const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// 单个规范模型的官方美元单价（单位：美元 / 百万 token）。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPricing {
    /// 厂商标签（仅用于归类与 UI 展示，不参与查价）。
    pub vendor: String,
    /// 未命中缓存输入价（`cache_write` 并入此档）。
    pub input_cache_miss: f64,
    /// 命中缓存输入价（`cache_read`）。
    pub input_cache_hit: f64,
    /// 输出价（`reasoning` 并入此档）。
    pub output: f64,
    /// 可选的人类可读定价备注。
    pub note: Option<String>,
}

/// 原始模型名查表归一后的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelIdentity {
    /// 统一后的小写模型名（未知模型保留小写原名）。
    pub name: String,
    /// 命中价目表时附带的官方价，未命中为 `None`。
    pub pricing: Option<ModelPricing>,
}

/// 当前生效价目表的来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogSource {
    /// 编译期内嵌的 `config/models.json`。
    EmbeddedDefault,
    /// 磁盘上的外部 JSON 文件（显式路径或用户配置目录）。
    File(PathBuf),
}

/// `models.json` 顶层反序列化结构。
#[derive(Debug, Deserialize)]
struct ModelsFile {
    version: u32,
    models: BTreeMap<String, RawModelEntry>,
}

/// `models.json` 中单个模型条目的反序列化结构。
#[derive(Debug, Deserialize)]
struct RawModelEntry {
    vendor: String,
    input_cache_miss: f64,
    input_cache_hit: f64,
    output: f64,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

/// 已通过校验的只读模型目录，提供 O(1) 查价。
#[derive(Debug, Clone)]
pub struct PricingCatalog {
    models: BTreeMap<String, ModelPricing>,
    aliases: HashMap<String, String>,
}

impl PricingCatalog {
    /// 加载并校验内嵌的默认 `config/models.json`。
    pub fn load_default() -> Result<Self> {
        Self::load_from_str(DEFAULT_MODELS_JSON)
            .context("failed to parse embedded config/models.json")
    }

    /// 从磁盘文件加载并校验 `models.json`。
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read models config at {}", path.display()))?;
        Self::load_from_str(&content)
            .with_context(|| format!("failed to parse models config at {}", path.display()))
    }

    /// 按优先级加载价目表：
    /// 1. 显式传入的文件路径
    /// 2. `~/.config/token-counter/models.json`（若存在）
    /// 3. 编译期内嵌的默认 `config/models.json`
    pub fn resolve(explicit_path: Option<&Path>) -> Result<(Self, CatalogSource)> {
        if let Some(path) = explicit_path {
            let catalog = Self::load_from_file(path)?;
            return Ok((catalog, CatalogSource::File(path.to_path_buf())));
        }

        if let Some(user_path) = default_user_models_path() {
            if user_path.is_file() {
                let catalog = Self::load_from_file(&user_path)?;
                return Ok((catalog, CatalogSource::File(user_path)));
            }
        }

        let catalog = Self::load_default()?;
        Ok((catalog, CatalogSource::EmbeddedDefault))
    }

    /// 解析并严格校验 `models.json` 内容（全小写、合法单价、无冲突别名）。
    pub fn load_from_str(json: &str) -> Result<Self> {
        let parsed: ModelsFile = serde_json::from_str(json)?;
        if parsed.version != SUPPORTED_SCHEMA_VERSION {
            bail!(
                "unsupported models.json version {} (expected {})",
                parsed.version,
                SUPPORTED_SCHEMA_VERSION
            );
        }

        let mut models = BTreeMap::new();
        let mut raw_aliases: Vec<(String, String)> = Vec::new();

        for (key, entry) in parsed.models {
            validate_lowercase_identifier(&key, "model key")?;

            let vendor = entry.vendor.trim().to_string();
            if vendor.is_empty() {
                bail!("model '{}' must specify a non-empty vendor", key);
            }
            if vendor != vendor.to_ascii_lowercase() {
                bail!("model '{}' vendor '{}' must be lowercase", key, vendor);
            }

            validate_rate(entry.input_cache_miss, &key, "input_cache_miss")?;
            validate_rate(entry.input_cache_hit, &key, "input_cache_hit")?;
            validate_rate(entry.output, &key, "output")?;

            for alias in entry.aliases {
                validate_lowercase_identifier(&alias, &format!("alias of '{}'", key))?;
                raw_aliases.push((alias, key.clone()));
            }

            let pricing = ModelPricing {
                vendor,
                input_cache_miss: entry.input_cache_miss,
                input_cache_hit: entry.input_cache_hit,
                output: entry.output,
                note: entry.note,
            };
            models.insert(key, pricing);
        }

        let mut aliases = HashMap::new();
        for (alias, canonical) in raw_aliases {
            if models.contains_key(&alias) {
                bail!(
                    "alias '{}' for model '{}' collides with an existing canonical model key",
                    alias,
                    canonical
                );
            }
            if let Some(existing) = aliases.insert(alias.clone(), canonical.clone()) {
                bail!(
                    "duplicate alias '{}' declared by both '{}' and '{}'",
                    alias,
                    existing,
                    canonical
                );
            }
        }

        Ok(Self { models, aliases })
    }

    /// 返回当前已注册的规范模型数量。
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// 将原始模型名归一为小写并查找官方美元价；未命中则保留小写原名且无价。
    pub fn identify(&self, raw_model: &str) -> ModelIdentity {
        let normalized = raw_model.trim().to_ascii_lowercase();

        if let Some(pricing) = self.models.get(&normalized) {
            return ModelIdentity {
                name: normalized,
                pricing: Some(pricing.clone()),
            };
        }

        if let Some(canonical) = self.aliases.get(&normalized) {
            if let Some(pricing) = self.models.get(canonical) {
                return ModelIdentity {
                    name: canonical.clone(),
                    pricing: Some(pricing.clone()),
                };
            }
        }

        ModelIdentity {
            name: normalized,
            pricing: None,
        }
    }
}

/// 按官方美元价结算单次请求费用。
pub fn calc_cost_usd(pricing: &ModelPricing, usage: &Usage) -> f64 {
    let miss_tokens = usage.input.saturating_add(usage.cache_write) as f64;
    let hit_tokens = usage.cache_read as f64;
    let out_tokens = usage.output_with_reasoning() as f64;

    (miss_tokens * pricing.input_cache_miss
        + hit_tokens * pricing.input_cache_hit
        + out_tokens * pricing.output)
        / 1_000_000.0
}

/// 默认用户级覆盖路径：`~/.config/token-counter/models.json`。
fn default_user_models_path() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(".config");
    path.push("token-counter");
    path.push("models.json");
    Some(path)
}

/// 校验规范键或别名非空、无首尾空白且严格全小写。
fn validate_lowercase_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{} must not be empty", label);
    }
    if value.trim() != value {
        bail!(
            "{} '{}' must not have leading or trailing whitespace",
            label,
            value
        );
    }
    if value != value.to_ascii_lowercase() {
        bail!("{} '{}' must be all lowercase", label, value);
    }
    Ok(())
}

/// 校验单价为有限非负数。
fn validate_rate(rate: f64, model: &str, field: &str) -> Result<()> {
    if !rate.is_finite() || rate < 0.0 {
        bail!(
            "model '{}' {} rate must be finite and >= 0 (got {})",
            model,
            field,
            rate
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_models_json_loads_and_validates() {
        let catalog = PricingCatalog::load_default().expect("embedded models.json must be valid");
        assert!(catalog.model_count() >= 11);
    }

    #[test]
    fn identify_handles_exact_case_and_aliases() {
        let catalog = PricingCatalog::load_default().unwrap();

        let exact = catalog.identify("deepseek-v4.1-flash");
        assert_eq!(exact.name, "deepseek-v4.1-flash");
        assert!(exact.pricing.is_some());

        let upper = catalog.identify("GLM-5.3");
        assert_eq!(upper.name, "glm-5.3");
        assert!(upper.pricing.is_some());

        let alias = catalog.identify("Gemini 4 Neon (High)");
        assert_eq!(alias.name, "gemini-4-neon");
        assert!(alias.pricing.is_some());

        let unknown = catalog.identify("Some-Unknown-Model");
        assert_eq!(unknown.name, "some-unknown-model");
        assert!(unknown.pricing.is_none());
    }

    #[test]
    fn calc_cost_usd_matches_official_rates() {
        let catalog = PricingCatalog::load_default().unwrap();
        let usage = Usage {
            input: 500_000,
            cache_write: 500_000,
            cache_read: 1_000_000,
            output: 250_000,
            reasoning: 250_000,
        };

        let ds = catalog.identify("deepseek-v4.1-flash");
        let ds_usd = calc_cost_usd(ds.pricing.as_ref().unwrap(), &usage);
        // 1M * 0.30 + 1M * 0.006 + 0.5M * 1.20 = 0.906
        assert!((ds_usd - 0.906).abs() < 1e-9);

        let gm = catalog.identify("gemini-4-neon");
        let gm_usd = calc_cost_usd(gm.pricing.as_ref().unwrap(), &usage);
        // 1M * 3.00 + 1M * 0.15 + 0.5M * 15.00 = 10.65
        assert!((gm_usd - 10.65).abs() < 1e-9);
    }

    #[test]
    fn validation_rejects_uppercase_keys_and_duplicate_aliases() {
        let bad_case = r#"{
            "version": 1,
            "models": {
                "GLM-5.3": {
                    "vendor": "zhipu",
                    "input_cache_miss": 1.0,
                    "input_cache_hit": 0.1,
                    "output": 2.0
                }
            }
        }"#;
        assert!(PricingCatalog::load_from_str(bad_case).is_err());

        let dup_alias = r#"{
            "version": 1,
            "models": {
                "model-a": {
                    "vendor": "v",
                    "input_cache_miss": 1.0,
                    "input_cache_hit": 0.1,
                    "output": 2.0,
                    "aliases": ["shared-alias"]
                },
                "model-b": {
                    "vendor": "v",
                    "input_cache_miss": 1.0,
                    "input_cache_hit": 0.1,
                    "output": 2.0,
                    "aliases": ["shared-alias"]
                }
            }
        }"#;
        assert!(PricingCatalog::load_from_str(dup_alias).is_err());
    }
}
