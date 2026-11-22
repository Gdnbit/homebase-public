//! References for comparison, never billing entitlement or permission.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

const PRICE_SOURCE: &str = "https://openrouter.ai/api/v1/models";
fn price_source(url: &str) -> bool {
    matches!(
        url,
        "https://openrouter.ai/api/v1/models"
            | "https://openrouter.ai/api/v1/models/user"
            | "https://openrouter.ai/api/v1/models/user?output_modalities=all"
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceState {
    Reported,
    Partial,
    Invalid,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateKind {
    Prompt,
    Completion,
    Request,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateUnit {
    UsdPerToken,
    UsdPerRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceRate {
    pub kind: RateKind,
    pub amount: String,
    pub unit: RateUnit,
    pub per_million_tokens: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceInfo {
    pub route_id: String,
    pub state: PriceState,
    pub source_url: String,
    pub observed_at: String,
    pub reference_only: bool,
    pub rates: Vec<PriceRate>,
    pub unmodeled_dimensions: usize,
    pub missing_dimensions: usize,
    pub invalid_dimensions: usize,
    pub conditional: bool,
}

/// Shift a nonnegative decimal exactly; never round a small positive rate to zero.
fn decimal(value: &str, shift: i32) -> Option<String> {
    if value.is_empty() || value.len() > 128 {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let mut exponent = lower.split('e');
    let mantissa = exponent.next()?;
    let power = match exponent.next() {
        Some(v) => v.parse::<i32>().ok()?,
        None => 0,
    };
    if exponent.next().is_some() || !(-24..=24).contains(&power) {
        return None;
    }
    let mut parts = mantissa.split('.');
    let integer = parts.next()?;
    let fraction = parts.next();
    if parts.next().is_some()
        || integer.is_empty()
        || !integer.bytes().all(|c| c.is_ascii_digit())
        || fraction.is_some_and(|v| v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let digits = format!("{}{}", integer, fraction.unwrap_or(""));
    let point = integer.len() as i32 + power + shift;
    let text = if point <= 0 {
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else if point as usize >= digits.len() {
        format!("{}{}", digits, "0".repeat(point as usize - digits.len()))
    } else {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    };
    let mut parts = text.split('.');
    let integer = parts.next()?.trim_start_matches('0');
    let fraction = parts.next().unwrap_or("").trim_end_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    Some(if fraction.is_empty() {
        integer.into()
    } else {
        format!("{integer}.{fraction}")
    })
}

impl PriceInfo {
    pub fn with_catalog_source(mut self, url: &str) -> Result<Self, String> {
        if !price_source(url) {
            return Err("模型报价来源未确认，已保留上次目录；未反射未知URL或猜测公开来源".into());
        }
        self.source_url = url.into();
        Ok(self)
    }
    pub fn from_openrouter(model_id: &str, raw: &Value, observed_at: &str) -> Self {
        let mut info = Self {
            route_id: format!("openrouter::{model_id}"),
            state: PriceState::Invalid,
            source_url: PRICE_SOURCE.into(),
            observed_at: observed_at.into(),
            reference_only: true,
            rates: vec![],
            unmodeled_dimensions: 0,
            missing_dimensions: 0,
            invalid_dimensions: 0,
            conditional: false,
        };
        let Some(object) = raw
            .as_object()
            .filter(|_| serde_json::to_vec(raw).is_ok_and(|v| v.len() <= 32768))
        else {
            info.invalid_dimensions = 1;
            return info;
        };
        info.unmodeled_dimensions = object
            .keys()
            .filter(|v| !["prompt", "completion", "request"].contains(&v.as_str()))
            .count();
        info.conditional = object.contains_key("discount") || object.contains_key("overrides");
        for (key, kind) in [
            ("prompt", RateKind::Prompt),
            ("completion", RateKind::Completion),
            ("request", RateKind::Request),
        ] {
            let Some(value) = object.get(key) else {
                info.missing_dimensions += 1;
                continue;
            };
            let Some(amount) = value
                .as_str()
                .filter(|v| v.len() <= 64)
                .and_then(|v| decimal(v, 0))
            else {
                info.invalid_dimensions += 1;
                continue;
            };
            let token = kind != RateKind::Request;
            let per_million_tokens = if token { decimal(&amount, 6) } else { None };
            info.rates.push(PriceRate {
                kind,
                amount,
                unit: if token {
                    RateUnit::UsdPerToken
                } else {
                    RateUnit::UsdPerRequest
                },
                per_million_tokens,
            });
        }
        info.state = if info.rates.is_empty() {
            PriceState::Invalid
        } else if info.unmodeled_dimensions + info.missing_dimensions + info.invalid_dimensions > 0
        {
            PriceState::Partial
        } else {
            PriceState::Reported
        };
        info
    }
    pub fn valid_for(&self, route_id: &str) -> bool {
        if self.unmodeled_dimensions > 4096
            || self.invalid_dimensions > 3
            || self.missing_dimensions > 3
        {
            return false;
        }
        let expected = if self.rates.is_empty() {
            PriceState::Invalid
        } else if self.unmodeled_dimensions + self.missing_dimensions + self.invalid_dimensions > 0
        {
            PriceState::Partial
        } else {
            PriceState::Reported
        };
        if self.route_id != route_id
            || !route_id.starts_with("openrouter::")
            || route_id.len() > 512
            || !price_source(&self.source_url)
            || !self.reference_only
            || self.observed_at.len() > 40
            || chrono::DateTime::parse_from_rfc3339(&self.observed_at).is_err()
            || self.state != expected
            || self.rates.len() > 3
            || self.unmodeled_dimensions > 4096
            || self.invalid_dimensions > 3
            || self.missing_dimensions > 3
        {
            return false;
        }
        let mut kinds = BTreeSet::new();
        self.rates.iter().all(|rate| {
            if !kinds.insert(&rate.kind) || decimal(&rate.amount, 0).as_ref() != Some(&rate.amount)
            {
                return false;
            }
            if rate.kind == RateKind::Request {
                rate.unit == RateUnit::UsdPerRequest && rate.per_million_tokens.is_none()
            } else {
                rate.unit == RateUnit::UsdPerToken
                    && decimal(&rate.amount, 6) == rate.per_million_tokens
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    RegistryRecorded,
    RecordsChecked,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceInfo {
    pub state: EvidenceState,
    pub source_commit: String,
    pub package_version: String,
    pub manifest_sha256: String,
    pub verification_sha256: String,
    pub registered_at: String,
    pub checked_at: Option<String>,
    pub code_integrity_checked: bool,
}
impl EvidenceInfo {
    pub fn valid(&self) -> bool {
        let hex = |text: &str, n| {
            text.len() == n
                && text
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        let date =
            |text: &str| text.len() <= 40 && chrono::DateTime::parse_from_rfc3339(text).is_ok();
        hex(&self.source_commit, 40)
            && hex(&self.manifest_sha256, 64)
            && hex(&self.verification_sha256, 64)
            && !self.package_version.is_empty()
            && self.package_version.len() <= 80
            && !self.package_version.chars().any(char::is_control)
            && date(&self.registered_at)
            && !self.code_integrity_checked
            && match self.state {
                EvidenceState::RegistryRecorded => self.checked_at.is_none(),
                EvidenceState::RecordsChecked => self.checked_at.as_deref().is_some_and(date),
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const TIME: &str = "2026-11-16T02:30:00+08:00";
    #[test]
    fn catalog_source_distinguishes_public_and_account_filtered_prices_without_url_reflection() {
        let p = PriceInfo::from_openrouter(
            "fixture",
            &json!({"prompt":"0","completion":"0","request":"0"}),
            TIME,
        );
        let user = p
            .clone()
            .with_catalog_source("https://openrouter.ai/api/v1/models/user?output_modalities=all")
            .unwrap();
        assert!(user.valid_for("openrouter::fixture"));
        assert_ne!(user.source_url, p.source_url);
        for url in [
            "https://example.invalid?token=synthetic-private",
            "https://openrouter.ai/api/v1/models/user?token=synthetic-private",
            "https://openrouter.ai.evil.invalid/api/v1/models",
        ] {
            let error = p.clone().with_catalog_source(url).unwrap_err();
            assert!(!error.contains("synthetic-private") && !error.contains("example.invalid"));
        }
        assert!(p
            .with_catalog_source("https://openrouter.ai/api/v1/models/user")
            .unwrap()
            .valid_for("openrouter::fixture"));
    }
    #[test]
    fn exact_price_conversion_preserves_tiny_positive_rates_and_units() {
        let p = PriceInfo::from_openrouter(
            "fixture",
            &json!({"prompt":"0.00000003","completion":"1e-24","request":"0.0100"}),
            TIME,
        );
        assert!(p.valid_for("openrouter::fixture"));
        assert_eq!(p.state, PriceState::Reported);
        assert_eq!(p.rates[0].per_million_tokens.as_deref(), Some("0.03"));
        assert_eq!(
            p.rates[1].per_million_tokens.as_deref(),
            Some("0.000000000000000001")
        );
        assert_eq!(p.rates[2].amount, "0.01");
        assert!(p.rates[2].per_million_tokens.is_none());
        assert!(!p.valid_for("openai::fixture"));
        assert!(!p.valid_for("openrouter::other"));
    }
    #[test]
    fn zero_reference_never_becomes_account_entitlement_and_other_dimensions_are_explicit() {
        let zero = PriceInfo::from_openrouter(
            "fixture:free",
            &json!({"prompt":"0","completion":"0","request":"0"}),
            TIME,
        );
        assert!(zero.reference_only && zero.valid_for("openrouter::fixture:free"));
        let p = PriceInfo::from_openrouter(
            "fixture",
            &json!({"prompt":"0","completion":"0","request":"0","image":"2","overrides":[{"min_prompt_tokens":1000,"prompt":"0.1"}],"discount":0}),
            TIME,
        );
        assert_eq!(p.state, PriceState::Partial);
        assert_eq!(p.unmodeled_dimensions, 3);
        assert!(p.conditional && p.reference_only);
    }
    #[test]
    fn invalid_missing_negative_and_unmodeled_prices_never_turn_into_zero() {
        for value in [
            json!(null),
            json!([]),
            json!({}),
            json!({"prompt":"-1","completion":"1e-400","request":0}),
        ] {
            let p = PriceInfo::from_openrouter("fixture", &value, TIME);
            assert_eq!(p.state, PriceState::Invalid);
            assert!(p.rates.is_empty());
            assert!(p.valid_for("openrouter::fixture"));
        }
        for amount in [
            "NaN",
            "Infinity",
            " 0",
            "0.0 USD",
            "1e999999999999999",
            "1.2.3",
            "1e-2147483648",
        ] {
            assert!(decimal(amount, 0).is_none());
        }
        let p = PriceInfo::from_openrouter("fixture", &json!({"prompt":"0.1"}), TIME);
        assert_eq!(p.state, PriceState::Partial);
        assert_eq!(p.missing_dimensions, 2);
    }
    #[test]
    fn persisted_price_facts_reject_tampered_units_sources_and_route_binding() {
        let p = PriceInfo::from_openrouter(
            "fixture",
            &json!({"prompt":"0.1","completion":"0.2","request":"0"}),
            TIME,
        );
        let mut changed = p.clone();
        changed.rates[0].per_million_tokens = Some("0".into());
        assert!(!changed.valid_for(&p.route_id));
        changed = p.clone();
        changed.reference_only = false;
        assert!(!changed.valid_for(&p.route_id));
        changed = p.clone();
        changed.source_url = "https://example.invalid?token=synthetic".into();
        assert!(!changed.valid_for(&p.route_id));
        changed = p.clone();
        changed.observed_at = "unknown".into();
        assert!(!changed.valid_for(&p.route_id));
        changed = p.clone();
        changed.unmodeled_dimensions = usize::MAX;
        assert!(!changed.valid_for(&p.route_id));
        let round: PriceInfo = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(round, p);
    }
    #[test]
    fn record_evidence_is_distinct_from_a_current_code_integrity_check() {
        let mut e = EvidenceInfo {
            state: EvidenceState::RegistryRecorded,
            source_commit: "a".repeat(40),
            package_version: "1.2.3".into(),
            manifest_sha256: "b".repeat(64),
            verification_sha256: "c".repeat(64),
            registered_at: TIME.into(),
            checked_at: None,
            code_integrity_checked: false,
        };
        assert!(e.valid());
        e.code_integrity_checked = true;
        assert!(!e.valid());
        e.code_integrity_checked = false;
        e.state = EvidenceState::RecordsChecked;
        assert!(!e.valid());
        e.checked_at = Some(TIME.into());
        assert!(e.valid());
    }
}
