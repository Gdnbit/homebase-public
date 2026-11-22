use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Clone)]
pub struct OpenRouterModelCatalog {
    pub fetched_at: String,
    pub source_url: String,
    pub groups: Vec<OpenRouterModelGroup>,
}

#[derive(Debug, Serialize, Clone)]
pub struct OpenRouterModelGroup {
    pub provider: String,
    pub label: String,
    pub models: Vec<OpenRouterModelOption>,
}

#[derive(Debug, Serialize, Clone)]
pub struct OpenRouterModelOption {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub supported_parameters: Vec<String>,
    pub prompt_price_per_m: Option<f64>,
    pub completion_price_per_m: Option<f64>,
    pub image_price: Option<f64>,
    pub request_price: Option<f64>,
    pub price_label: String,
    pub capability_score: i32,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterRawModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterRawModel {
    id: String,
    name: String,
    context_length: Option<u64>,
    architecture: Option<OpenRouterArchitecture>,
    pricing: Option<OpenRouterPricing>,
    supported_parameters: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterArchitecture {
    input_modalities: Option<Vec<String>>,
    output_modalities: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterPricing {
    prompt: Option<String>,
    completion: Option<String>,
    image: Option<String>,
    request: Option<String>,
}

#[tauri::command]
pub async fn openrouter_models_catalog() -> Result<OpenRouterModelCatalog, String> {
    let source_url = "https://openrouter.ai/api/v1/models?output_modalities=all";
    let response = reqwest::Client::new()
        .get(source_url)
        .header("User-Agent", "HomeBase/0.1 OpenRouter catalog")
        .send()
        .await
        .map_err(|e| format!("OpenRouter 模型列表请求失败: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "OpenRouter 模型列表返回 HTTP {}: {}",
            status.as_u16(),
            body.chars().take(240).collect::<String>()
        ));
    }

    let raw = response
        .json::<OpenRouterModelsResponse>()
        .await
        .map_err(|e| format!("OpenRouter 模型列表 JSON 解析失败: {e}"))?;

    let mut groups: BTreeMap<String, Vec<OpenRouterModelOption>> = BTreeMap::new();
    for model in raw.data {
        let provider = model.id.split('/').next().unwrap_or("unknown").to_string();
        let option = raw_model_to_option(model);
        groups.entry(provider).or_default().push(option);
    }

    let mut grouped: Vec<OpenRouterModelGroup> = groups
        .into_iter()
        .map(|(provider, mut models)| {
            models.sort_by(|a, b| {
                b.capability_score
                    .cmp(&a.capability_score)
                    .then_with(|| {
                        b.context_length
                            .unwrap_or(0)
                            .cmp(&a.context_length.unwrap_or(0))
                    })
                    .then_with(|| a.name.cmp(&b.name))
            });
            OpenRouterModelGroup {
                label: provider_label(&provider).to_string(),
                provider,
                models,
            }
        })
        .collect();

    grouped.sort_by(|a, b| {
        provider_rank(&a.provider)
            .cmp(&provider_rank(&b.provider))
            .then_with(|| a.label.cmp(&b.label))
    });

    Ok(OpenRouterModelCatalog {
        fetched_at: chrono::Utc::now().to_rfc3339(),
        source_url: source_url.to_string(),
        groups: grouped,
    })
}

fn raw_model_to_option(model: OpenRouterRawModel) -> OpenRouterModelOption {
    let input_modalities = model
        .architecture
        .as_ref()
        .and_then(|a| a.input_modalities.clone())
        .unwrap_or_default();
    let output_modalities = model
        .architecture
        .as_ref()
        .and_then(|a| a.output_modalities.clone())
        .unwrap_or_default();
    let supported_parameters = model.supported_parameters.unwrap_or_default();
    let prompt_price_per_m = model
        .pricing
        .as_ref()
        .and_then(|p| parse_price_per_m(p.prompt.as_deref()));
    let completion_price_per_m = model
        .pricing
        .as_ref()
        .and_then(|p| parse_price_per_m(p.completion.as_deref()));
    let image_price = model
        .pricing
        .as_ref()
        .and_then(|p| parse_price(p.image.as_deref()));
    let request_price = model
        .pricing
        .as_ref()
        .and_then(|p| parse_price(p.request.as_deref()));
    let price_label = price_label(
        prompt_price_per_m,
        completion_price_per_m,
        image_price,
        request_price,
    );
    let capability_score = capability_score(
        &model.id,
        &model.name,
        model.context_length,
        &input_modalities,
        &output_modalities,
        &supported_parameters,
    );

    OpenRouterModelOption {
        id: model.id,
        name: model.name,
        context_length: model.context_length,
        input_modalities,
        output_modalities,
        supported_parameters,
        prompt_price_per_m,
        completion_price_per_m,
        image_price,
        request_price,
        price_label,
        capability_score,
    }
}

fn parse_price(value: Option<&str>) -> Option<f64> {
    value.and_then(|s| s.parse::<f64>().ok())
}

fn parse_price_per_m(value: Option<&str>) -> Option<f64> {
    parse_price(value).map(|v| v * 1_000_000.0)
}

fn price_label(
    prompt: Option<f64>,
    completion: Option<f64>,
    image: Option<f64>,
    request: Option<f64>,
) -> String {
    if let Some(image) = image.filter(|v| *v > 0.0) {
        return format!("image ${image:.4}");
    }
    if let Some(request) = request.filter(|v| *v > 0.0) {
        return format!("request ${request:.4}");
    }
    match (prompt, completion) {
        (Some(p), Some(c)) if p == 0.0 && c == 0.0 => "free".to_string(),
        (Some(p), Some(c)) => format!("${p:.3}/${c:.3} per 1M"),
        (Some(p), None) => format!("input ${p:.3} per 1M"),
        (None, Some(c)) => format!("output ${c:.3} per 1M"),
        _ => "price n/a".to_string(),
    }
}

fn provider_label(provider: &str) -> &str {
    match provider {
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "google" => "Google",
        "meta-llama" => "Meta",
        "mistralai" => "Mistral",
        "qwen" => "Qwen",
        "deepseek" => "DeepSeek",
        "x-ai" => "xAI",
        "cohere" => "Cohere",
        "perplexity" => "Perplexity",
        "microsoft" => "Microsoft",
        "nvidia" => "NVIDIA",
        "minimax" => "MiniMax",
        "recraft" => "Recraft",
        "sourceful" => "Sourceful",
        "stepfun" => "StepFun",
        "z-ai" => "Z.ai",
        _ => provider,
    }
}

fn provider_rank(provider: &str) -> i32 {
    match provider {
        "openai" => 0,
        "anthropic" => 1,
        "google" => 2,
        "x-ai" => 3,
        "mistralai" => 4,
        "qwen" => 5,
        "deepseek" => 6,
        "meta-llama" => 7,
        "microsoft" => 8,
        "nvidia" => 9,
        _ => 100,
    }
}

fn capability_score(
    id: &str,
    name: &str,
    context_length: Option<u64>,
    input_modalities: &[String],
    output_modalities: &[String],
    supported_parameters: &[String],
) -> i32 {
    let haystack = format!("{} {}", id.to_lowercase(), name.to_lowercase());
    let mut score = 0;

    for (needle, points) in [
        ("opus", 100),
        ("gpt-5", 98),
        ("gpt-4.5", 94),
        ("gemini-3", 94),
        ("pro", 85),
        ("sonnet", 82),
        ("grok", 78),
        ("reasoning", 72),
        ("plus", 68),
        ("max", 65),
        ("medium", 60),
        ("flash", 45),
        ("mini", 35),
        ("nano", 25),
        ("free", -10),
    ] {
        if haystack.contains(needle) {
            score += points;
        }
    }

    if input_modalities.iter().any(|m| m == "image") {
        score += 8;
    }
    if input_modalities.iter().any(|m| m == "audio") {
        score += 8;
    }
    if input_modalities.iter().any(|m| m == "video") {
        score += 10;
    }
    if output_modalities.iter().any(|m| m == "image") {
        score += 7;
    }
    if output_modalities.iter().any(|m| m == "video") {
        score += 9;
    }
    if supported_parameters.iter().any(|p| p == "tools") {
        score += 5;
    }
    if supported_parameters.iter().any(|p| p == "response_format") {
        score += 5;
    }
    if let Some(ctx) = context_length {
        if ctx >= 1_000_000 {
            score += 12;
        } else if ctx >= 200_000 {
            score += 8;
        } else if ctx >= 100_000 {
            score += 4;
        }
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_per_m_converts_token_price() {
        assert_eq!(parse_price_per_m(Some("0.000003")), Some(3.0));
    }

    #[test]
    fn price_label_formats_free_and_paid() {
        assert_eq!(price_label(Some(0.0), Some(0.0), None, None), "free");
        assert_eq!(
            price_label(Some(3.0), Some(15.0), None, None),
            "$3.000/$15.000 per 1M"
        );
    }
}
