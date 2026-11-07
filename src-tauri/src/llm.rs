//! Chat Completions requests and locally validated action plans.

use std::{env, net::IpAddr, time::Duration};

use reqwest::{Client, Url, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::DeskActError;

const ACTION_PLAN_VERSION: u32 = 1;
const MAX_ACTION_PLAN_BYTES: usize = 32 * 1024;
const MAX_ACTION_PLAN_STEPS: usize = 20;
const MAX_ACTION_TEXT_LENGTH: usize = 500;
const MAX_OPERATION_LENGTH: usize = 80;
const MAX_COMPLETION_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// A complete, locally validated proposal from the primary model; not authorization to execute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionPlan {
    /// Version of DeskAct's action-plan JSON contract.
    pub version: u32,
    /// Concise human-readable statement of the proposed outcome.
    pub summary: String,
    /// Bounded ordered proposal steps and their expected outcomes.
    pub steps: Vec<ActionPlanStep>,
}

/// One proposed operation and its locally interpreted parameters and postcondition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionPlanStep {
    /// Operation identifier; execution support is determined later by Rust policy.
    pub operation: String,
    /// Structured operation parameters, never treated as permission by themselves.
    pub parameters: std::collections::BTreeMap<String, Value>,
    /// Observable outcome expected if the operation succeeds.
    pub expected_outcome: String,
}

/// Result of the no-actions synthetic endpoint check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LlmConnectionReport {
    /// Whether a credential was sent and accepted by the endpoint.
    pub authentication_accepted: bool,
    /// Whether the endpoint returned non-empty completion content.
    pub response_content_received: bool,
}

struct LlmConfig {
    chat_completions_url: Url,
    model: String,
    api_key: Option<HeaderValue>,
}

impl LlmConfig {
    fn from_env() -> Result<Self, DeskActError> {
        let base_url = env::var("DESKACT_LLM_BASE_URL").map_err(|_| {
            DeskActError::new(
                "Set DESKACT_LLM_BASE_URL to the configured Chat Completions endpoint base URL"
                    .into(),
            )
        })?;
        let model = env::var("DESKACT_LLM_MODEL").map_err(|_| {
            DeskActError::new("Set DESKACT_LLM_MODEL to an explicit model identifier".into())
        })?;
        if model.trim().is_empty()
            || model.chars().count() > 200
            || model.chars().any(char::is_control)
        {
            return Err(DeskActError::new(
                "DESKACT_LLM_MODEL must be a non-empty identifier of at most 200 characters".into(),
            ));
        }

        let mut base_url = Url::parse(&base_url).map_err(|_| {
            DeskActError::new("DESKACT_LLM_BASE_URL must be a valid absolute URL".into())
        })?;
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(DeskActError::new(
                "DESKACT_LLM_BASE_URL must not contain credentials, a query, or a fragment".into(),
            ));
        }
        if base_url.scheme() != "https"
            && !(base_url.scheme() == "http" && is_loopback_host(&base_url))
        {
            return Err(DeskActError::new(
                "Use HTTPS for remote LLM endpoints; HTTP is permitted only for loopback endpoints"
                    .into(),
            ));
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        let chat_completions_url = base_url.join("chat/completions").map_err(|_| {
            DeskActError::new("Could not construct the Chat Completions endpoint URL".into())
        })?;

        let api_key = match env::var("DESKACT_LLM_API_KEY") {
            Ok(value) if !value.is_empty() => Some(
                HeaderValue::from_str(&format!("Bearer {value}")).map_err(|_| {
                    DeskActError::new(
                        "DESKACT_LLM_API_KEY contains invalid header characters".into(),
                    )
                })?,
            ),
            Ok(_) | Err(env::VarError::NotPresent) => None,
            Err(env::VarError::NotUnicode(_)) => {
                return Err(DeskActError::new(
                    "DESKACT_LLM_API_KEY must be valid UTF-8".into(),
                ));
            }
        };

        Ok(Self {
            chat_completions_url,
            model,
            api_key,
        })
    }

    fn client(&self) -> Result<Client, DeskActError> {
        Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| DeskActError::new("Could not initialize the HTTP client".into()))
    }

    async fn complete(&self, messages: Vec<ChatMessage>) -> Result<String, DeskActError> {
        let client = self.client()?;
        let mut request = client
            .post(self.chat_completions_url.clone())
            .json(&ChatRequest {
                model: &self.model,
                messages,
            });
        if let Some(api_key) = &self.api_key {
            request = request.header(reqwest::header::AUTHORIZATION, api_key.clone());
        }
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                DeskActError::new("Chat Completions request timed out".into())
            } else {
                DeskActError::new("Could not reach the configured Chat Completions endpoint".into())
            }
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(DeskActError::new(format!(
                "Endpoint reachable, authentication rejected (HTTP {})",
                status.as_u16()
            )));
        }
        if !status.is_success() {
            return Err(DeskActError::new(format!(
                "Endpoint reachable, but Chat Completions returned HTTP {}",
                status.as_u16()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_COMPLETION_BYTES as u64)
        {
            return Err(DeskActError::new(format!(
                "Chat Completions response exceeds the {MAX_COMPLETION_BYTES}-byte limit"
            )));
        }
        let mut response = response;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| DeskActError::new("Could not read the Chat Completions response".into()))?
        {
            if body.len() + chunk.len() > MAX_COMPLETION_BYTES {
                return Err(DeskActError::new(format!(
                    "Chat Completions response exceeds the {MAX_COMPLETION_BYTES}-byte limit"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        let response: ChatResponse = serde_json::from_slice(&body).map_err(|_| {
            DeskActError::new(
                "Endpoint reachable, but response did not match the Chat Completions response shape"
                    .into(),
            )
        })?;
        let choice = response.choices.into_iter().next().ok_or_else(|| {
            DeskActError::new(
                "Endpoint reachable, but response contained no completion choice".into(),
            )
        })?;
        match choice.finish_reason.as_deref() {
            Some("length") => {
                return Err(DeskActError::new(
                    "Endpoint reachable, but completion was cut off by the token limit".into(),
                ));
            }
            Some("content_filter") => {
                return Err(DeskActError::new(
                    "Endpoint reachable, but completion was rejected by the provider content filter"
                        .into(),
                ));
            }
            _ => {}
        }
        let content = choice
            .message
            .content
            .filter(|content| !content.trim().is_empty())
            .ok_or_else(|| {
                DeskActError::new(
                    "Endpoint reachable, but response contained no completion content".into(),
                )
            })?;
        Ok(content)
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChatMessageResponse {
    content: Option<String>,
}

/// Send a synthetic, non-sensitive request without executing any desktop action.
pub async fn test_llm_connection() -> Result<LlmConnectionReport, DeskActError> {
    let config = LlmConfig::from_env()?;
    let content = config
        .complete(vec![
            ChatMessage {
                role: "system",
                content: "This is a synthetic connectivity check. Return a short plain-text reply; do not propose or execute desktop actions.".into(),
            },
            ChatMessage {
                role: "user",
                content: "Reply with the words: DeskAct synthetic connection check.".into(),
            },
        ])
        .await?;
    Ok(LlmConnectionReport {
        authentication_accepted: config.api_key.is_some(),
        response_content_received: !content.trim().is_empty(),
    })
}

/// Request a plan from the primary model and allow one bounded correction attempt.
pub async fn request_action_plan(request: &str) -> Result<ActionPlan, DeskActError> {
    if request.trim().is_empty()
        || request.chars().count() > 4_000
        || request.chars().any(char::is_control)
    {
        return Err(DeskActError::new(
            "Task request must be non-empty, at most 4,000 characters, and contain no control characters".into(),
        ));
    }
    let config = LlmConfig::from_env()?;
    let system = r#"Return only one complete JSON object matching DeskAct action-plan version 1. Schema: {"version":1,"summary":string,"steps":[{"operation":lowercase_identifier,"parameters":object,"expected_outcome":string}]}. Use 1 to 20 steps; summary and expected_outcome must be non-empty and at most 500 characters. This is a proposal, not authorization. Never include shell commands or executable paths."#;
    let first_response = config
        .complete(vec![
            ChatMessage {
                role: "system",
                content: system.into(),
            },
            ChatMessage {
                role: "user",
                content: format!("Propose a plan for this request: {request}"),
            },
        ])
        .await?;
    let validation_feedback = match validate_action_plan_json(&first_response) {
        Ok(plan) => return reject_credential_echo(plan, &config),
        Err(_) => "The previous response failed local schema or semantic validation.",
    };

    let corrected_response = config
        .complete(vec![
            ChatMessage {
                role: "system",
                content: system.into(),
            },
            ChatMessage {
                role: "user",
                content: format!("Original task request: {request}"),
            },
            ChatMessage {
                role: "assistant",
                content: first_response,
            },
            ChatMessage {
                role: "user",
                content: format!(
                    "{validation_feedback} Return one corrected complete JSON action plan matching the system contract."
                ),
            },
        ])
        .await?;
    let plan = validate_action_plan_json(&corrected_response).map_err(|_| {
        DeskActError::new(
            "Primary model returned an invalid action plan after one bounded correction attempt; the corrected response failed local validation and was suppressed".into(),
        )
    })?;
    reject_credential_echo(plan, &config)
}

fn reject_credential_echo(
    plan: ActionPlan,
    config: &LlmConfig,
) -> Result<ActionPlan, DeskActError> {
    let Some(credential) = config
        .api_key
        .as_ref()
        .and_then(|header| header.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
    else {
        return Ok(plan);
    };
    if plan.summary.contains(credential)
        || plan.steps.iter().any(|step| {
            step.operation.contains(credential)
                || step.expected_outcome.contains(credential)
                || step.parameters.iter().any(|(key, value)| {
                    key.contains(credential) || value_contains_credential(value, credential)
                })
        })
    {
        return Err(DeskActError::new(
            "Validated action plan contains the configured credential and was suppressed".into(),
        ));
    }
    Ok(plan)
}

fn value_contains_credential(value: &Value, credential: &str) -> bool {
    match value {
        Value::String(value) => value.contains(credential),
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_credential(value, credential)),
        Value::Object(values) => values.iter().any(|(key, value)| {
            key.contains(credential) || value_contains_credential(value, credential)
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// Parse and semantically validate a complete version-1 action plan.
pub fn validate_action_plan_json(response: &str) -> Result<ActionPlan, DeskActError> {
    if response.len() > MAX_ACTION_PLAN_BYTES {
        return Err(DeskActError::new(format!(
            "Action plan exceeds the {MAX_ACTION_PLAN_BYTES}-byte size limit"
        )));
    }
    let plan: ActionPlan = serde_json::from_str(response).map_err(|error| {
        DeskActError::new(format!(
            "Primary model returned invalid action-plan JSON: {error}"
        ))
    })?;
    if plan.version != ACTION_PLAN_VERSION {
        return Err(DeskActError::new(format!(
            "Unsupported action-plan version {}; expected {ACTION_PLAN_VERSION}",
            plan.version
        )));
    }
    validate_text(&plan.summary, MAX_ACTION_TEXT_LENGTH, "summary")?;
    if plan.steps.is_empty() || plan.steps.len() > MAX_ACTION_PLAN_STEPS {
        return Err(DeskActError::new(format!(
            "Action plan must contain between 1 and {MAX_ACTION_PLAN_STEPS} steps"
        )));
    }
    for (index, step) in plan.steps.iter().enumerate() {
        validate_operation(&step.operation, index + 1)?;
        validate_text(
            &step.expected_outcome,
            MAX_ACTION_TEXT_LENGTH,
            &format!("step {} expected outcome", index + 1),
        )?;
    }
    Ok(plan)
}

fn is_loopback_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_text(value: &str, max_length: usize, field: &str) -> Result<(), DeskActError> {
    if value.trim().is_empty()
        || value.chars().count() > max_length
        || value.chars().any(char::is_control)
    {
        return Err(DeskActError::new(format!(
            "Action plan {field} must be non-empty, at most {max_length} characters, and contain no control characters"
        )));
    }
    Ok(())
}

fn validate_operation(operation: &str, step: usize) -> Result<(), DeskActError> {
    let mut chars = operation.chars();
    let valid_first = chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase());
    let valid_rest = chars.all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '_' | '.' | '-')
    });
    if !valid_first || !valid_rest || operation.len() > MAX_OPERATION_LENGTH {
        return Err(DeskActError::new(format!(
            "Action plan step {step} operation must be a lowercase identifier of at most {MAX_OPERATION_LENGTH} characters"
        )));
    }
    Ok(())
}
