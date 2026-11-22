//! Chat Completions requests and locally validated action plans.

use std::{env, net::IpAddr, time::Duration};

use reqwest::{Client, Url, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::DeskActError;

const ACTION_PLAN_VERSION: u32 = 1;
const MAX_ACTION_PLAN_BYTES: usize = 32 * 1024;
const MAX_ACTION_PLAN_STEPS: usize = 20;
const MAX_ACTION_TEXT_LENGTH: usize = 500;
const MAX_OPERATION_LENGTH: usize = 80;
const MAX_COMPLETION_BYTES: usize = 64 * 1024;
const MAX_TOOL_CALLS_PER_TURN: usize = 5;
const MAX_TOOL_ARGUMENT_BYTES: usize = 8 * 1024;
const MAX_TOOL_REF_LENGTH: usize = 100;
const MAX_TOOL_TEXT_LENGTH: usize = 2_000;
const MAX_TOOL_SUMMARY_LENGTH: usize = 500;
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
    /// Whether the endpoint answered a synthetic request by proposing a well-formed tool call.
    /// Agent-loop tasks require this; open-app JSON plans do not.
    pub tool_calling_supported: bool,
}

/// One tool from the fixed agent-loop tool set, with locally validated arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTool {
    /// Fresh bounded snapshot of the target window, optionally rooted at a ref.
    Observe { root: Option<String> },
    /// Click the control addressed by an accessibility ref.
    Click { element_ref: String },
    /// Replace a control's value with text.
    SetValue { element_ref: String, text: String },
    /// Press a key or shortcut such as `ctrl+s`.
    Press { keys: String },
    /// Read a control's current value.
    ReadValue { element_ref: String },
    /// Model-reported completion; not a verified outcome until Rust confirms it.
    Finish { summary: String },
    /// Pause the loop and ask the user a question.
    AskUser { question: String },
}

/// A model-proposed tool call; a proposal only, never authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallProposal {
    /// Provider call id, to be echoed in the tool-result message.
    pub id: String,
    /// The validated tool and arguments.
    pub tool: AgentTool,
}

/// One model turn: proposed tool calls and any accompanying text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurn {
    /// Validated proposals in model order; empty when the model answered with text only.
    pub tool_calls: Vec<ToolCallProposal>,
    /// Assistant text accompanying the turn, if any.
    pub content: Option<String>,
    /// Raw assistant message to append to the conversation so tool results can reference it.
    pub assistant_message: Value,
}

#[derive(Deserialize)]
struct RawToolCall {
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    function: Option<RawFunction>,
}

#[derive(Deserialize)]
struct RawFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserveArgs {
    root: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefArgs {
    #[serde(rename = "ref")]
    element_ref: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetValueArgs {
    #[serde(rename = "ref")]
    element_ref: String,
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PressArgs {
    keys: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FinishArgs {
    summary: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskUserArgs {
    question: String,
}

/// Chat Completions `tools` definitions for the fixed agent-loop tool set.
pub fn agent_tool_definitions() -> Value {
    let function = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({"type": "function", "function": {
            "name": name,
            "description": description,
            "parameters": {"type": "object", "properties": properties,
                "required": required, "additionalProperties": false}
        }})
    };
    let element =
        json!({"type": "string", "description": "Accessibility ref from the latest observation"});
    json!([
        function(
            "observe",
            "Take a fresh snapshot of the target window, optionally rooted at a ref.",
            json!({"root": element}),
            &[]
        ),
        function(
            "click",
            "Click the control with the given ref.",
            json!({"ref": element}),
            &["ref"]
        ),
        function(
            "set_value",
            "Replace the value of the control with the given ref.",
            json!({"ref": element, "text": {"type": "string"}}),
            &["ref", "text"]
        ),
        function(
            "press",
            "Press a key or shortcut in the target window.",
            json!({"keys": {"type": "string"}}),
            &["keys"]
        ),
        function(
            "read_value",
            "Read the current value of the control with the given ref.",
            json!({"ref": element}),
            &["ref"]
        ),
        function(
            "finish",
            "Report the task as complete with a short summary.",
            json!({"summary": {"type": "string"}}),
            &["summary"]
        ),
        function(
            "ask_user",
            "Pause and ask the user a question.",
            json!({"question": {"type": "string"}}),
            &["question"]
        ),
    ])
}

/// Parse an assistant chat message (JSON) into validated tool-call proposals; any malformed
/// call fails the whole turn closed.
pub fn parse_tool_calls(message: &Value) -> Result<Vec<ToolCallProposal>, DeskActError> {
    let raw: Vec<RawToolCall> = match message.get("tool_calls") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|_| tool_error("tool_calls did not match the Chat Completions shape"))?,
    };
    parse_raw_tool_calls(raw)
}

fn assistant_message_for(content: &Option<String>, calls: &[ToolCallProposal]) -> Value {
    let calls: Vec<Value> = calls
        .iter()
        .map(|call| {
            let (name, arguments) = match &call.tool {
                AgentTool::Observe { root } => ("observe", json!({"root": root})),
                AgentTool::Click { element_ref } => ("click", json!({"ref": element_ref})),
                AgentTool::SetValue { element_ref, text } => {
                    ("set_value", json!({"ref": element_ref, "text": text}))
                }
                AgentTool::Press { keys } => ("press", json!({"keys": keys})),
                AgentTool::ReadValue { element_ref } => ("read_value", json!({"ref": element_ref})),
                AgentTool::Finish { summary } => ("finish", json!({"summary": summary})),
                AgentTool::AskUser { question } => ("ask_user", json!({"question": question})),
            };
            json!({"id": call.id, "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()}})
        })
        .collect();
    json!({"role": "assistant", "content": content, "tool_calls": calls})
}

fn tool_error(detail: &str) -> DeskActError {
    DeskActError::new(format!(
        "Primary model returned a malformed tool call ({detail}); it was suppressed"
    ))
}

fn parse_raw_tool_calls(raw: Vec<RawToolCall>) -> Result<Vec<ToolCallProposal>, DeskActError> {
    if raw.len() > MAX_TOOL_CALLS_PER_TURN {
        return Err(tool_error(&format!(
            "more than {MAX_TOOL_CALLS_PER_TURN} calls in one turn"
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    raw.into_iter()
        .map(|call| {
            if call.kind.as_deref() != Some("function") {
                return Err(tool_error("call type must be function"));
            }
            let id = call
                .id
                .filter(|id| validate_tool_text(id, MAX_TOOL_REF_LENGTH))
                .ok_or_else(|| tool_error("missing or invalid call id"))?;
            if !seen.insert(id.clone()) {
                return Err(tool_error("duplicate call id"));
            }
            let function = call
                .function
                .ok_or_else(|| tool_error("missing function"))?;
            let name = function
                .name
                .ok_or_else(|| tool_error("missing tool name"))?;
            let arguments = function
                .arguments
                .ok_or_else(|| tool_error("missing arguments"))?;
            if arguments.len() > MAX_TOOL_ARGUMENT_BYTES {
                return Err(tool_error("arguments exceed the size limit"));
            }
            let tool = parse_tool(&name, &arguments)?;
            Ok(ToolCallProposal { id, tool })
        })
        .collect()
}

fn validate_tool_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}

fn parse_tool(name: &str, arguments: &str) -> Result<AgentTool, DeskActError> {
    fn args<T: serde::de::DeserializeOwned>(arguments: &str) -> Result<T, DeskActError> {
        serde_json::from_str(arguments)
            .map_err(|_| tool_error("arguments did not match the tool schema"))
    }
    let checked = |value: String, max: usize| {
        if validate_tool_text(&value, max) {
            Ok(value)
        } else {
            Err(tool_error(
                "argument is empty, too long, or has control characters",
            ))
        }
    };
    Ok(match name {
        "observe" => AgentTool::Observe {
            root: args::<ObserveArgs>(arguments)?
                .root
                .map(|root| checked(root, MAX_TOOL_REF_LENGTH))
                .transpose()?,
        },
        "click" => AgentTool::Click {
            element_ref: checked(args::<RefArgs>(arguments)?.element_ref, MAX_TOOL_REF_LENGTH)?,
        },
        "read_value" => AgentTool::ReadValue {
            element_ref: checked(args::<RefArgs>(arguments)?.element_ref, MAX_TOOL_REF_LENGTH)?,
        },
        "set_value" => {
            let parsed: SetValueArgs = args(arguments)?;
            if parsed.text.chars().count() > MAX_TOOL_TEXT_LENGTH {
                return Err(tool_error("text is too long"));
            }
            AgentTool::SetValue {
                element_ref: checked(parsed.element_ref, MAX_TOOL_REF_LENGTH)?,
                text: parsed.text,
            }
        }
        "press" => AgentTool::Press {
            keys: checked(args::<PressArgs>(arguments)?.keys, MAX_TOOL_REF_LENGTH)?,
        },
        "finish" => AgentTool::Finish {
            summary: checked(
                args::<FinishArgs>(arguments)?.summary,
                MAX_TOOL_SUMMARY_LENGTH,
            )?,
        },
        "ask_user" => AgentTool::AskUser {
            question: checked(
                args::<AskUserArgs>(arguments)?.question,
                MAX_TOOL_SUMMARY_LENGTH,
            )?,
        },
        _ => return Err(tool_error("unknown tool name")),
    })
}

/// Send conversation messages (Chat Completions message objects) with the agent-loop tool set
/// and return the model's validated proposals. Requires an endpoint that supports tool calling.
pub async fn request_agent_turn(messages: Vec<Value>) -> Result<AgentTurn, DeskActError> {
    let config = LlmConfig::from_env()?;
    config.request_tools(messages).await
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
        let choice = self
            .send_chat(&ChatRequest {
                model: &self.model,
                messages,
                tools: None,
            })
            .await?;
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

    async fn request_tools(&self, messages: Vec<Value>) -> Result<AgentTurn, DeskActError> {
        let tools = agent_tool_definitions();
        let choice = self
            .send_chat(&ChatRequest {
                model: &self.model,
                messages,
                tools: Some(&tools),
            })
            .await
            .map_err(|error| {
                DeskActError::new(format!(
                    "{error}. Agent-loop tasks require an endpoint that supports Chat Completions tool calling; opening apps still works without it"
                ))
            })?;
        let ChatMessageResponse {
            content,
            tool_calls,
        } = choice.message;
        let tool_calls = parse_raw_tool_calls(tool_calls.unwrap_or_default())?;
        let content = content.filter(|content| !content.trim().is_empty());
        if tool_calls.is_empty() && content.is_none() {
            return Err(DeskActError::new(
                "Endpoint reachable, but response contained neither tool calls nor content".into(),
            ));
        }
        if let Some(credential) = self.credential() {
            let echoed = content
                .as_deref()
                .is_some_and(|text| text.contains(credential))
                || tool_calls
                    .iter()
                    .any(|call| format!("{:?}", call.tool).contains(credential));
            if echoed {
                return Err(DeskActError::new(
                    "Tool-call response contains the configured credential and was suppressed"
                        .into(),
                ));
            }
        }
        let assistant_message = assistant_message_for(&content, &tool_calls);
        Ok(AgentTurn {
            tool_calls,
            content,
            assistant_message,
        })
    }

    fn credential(&self) -> Option<&str> {
        self.api_key
            .as_ref()
            .and_then(|header| header.to_str().ok())
            .and_then(|header| header.strip_prefix("Bearer "))
    }

    async fn send_chat<M: Serialize>(
        &self,
        body: &ChatRequest<'_, M>,
    ) -> Result<ChatChoice, DeskActError> {
        let client = self.client()?;
        let mut request = client.post(self.chat_completions_url.clone()).json(body);
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
        Ok(choice)
    }
}

#[derive(Serialize)]
struct ChatRequest<'a, M> {
    model: &'a str,
    messages: Vec<M>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a Value>,
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
    tool_calls: Option<Vec<RawToolCall>>,
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
    // Probe tool calling with a synthetic request. Any failure means "not supported" here: the
    // endpoint was already shown reachable, and no returned call is ever executed.
    let tool_calling_supported = config
        .request_tools(vec![
            json!({"role": "system", "content": "This is a synthetic capability check. Call the finish tool with the summary 'DeskAct tool-calling check'. Do not propose or execute desktop actions."}),
            json!({"role": "user", "content": "Run the synthetic tool-calling check."}),
        ])
        .await
        .is_ok_and(|turn| !turn.tool_calls.is_empty());
    Ok(LlmConnectionReport {
        authentication_accepted: config.api_key.is_some(),
        response_content_received: !content.trim().is_empty(),
        tool_calling_supported,
    })
}

/// Request a plan from the primary model and allow one bounded correction attempt.
pub async fn request_action_plan(request: &str) -> Result<ActionPlan, DeskActError> {
    request_action_plan_with_context(request, None).await
}

/// Request a plan with privacy-minimal local context such as eligible catalog candidates.
pub async fn request_action_plan_with_context(
    request: &str,
    context: Option<&str>,
) -> Result<ActionPlan, DeskActError> {
    if request.trim().is_empty()
        || request.chars().count() > 4_000
        || request.chars().any(char::is_control)
    {
        return Err(DeskActError::new(
            "Task request must be non-empty, at most 4,000 characters, and contain no control characters".into(),
        ));
    }
    let config = LlmConfig::from_env()?;
    let system = if context.is_some() {
        r#"Return only one complete JSON object matching DeskAct action-plan version 1. Schema: {"version":1,"summary":string,"steps":[{"operation":lowercase_identifier,"parameters":object,"expected_outcome":string}]}. Use 1 to 20 steps; summary and expected_outcome must be non-empty and at most 500 characters. For this executor, the only supported operation is open_app with exactly one parameter app_id selected from the supplied catalog candidates. For each step, expected_outcome must be exactly `<selected candidate display_name> is open`. This is a proposal, not authorization. Never include shell commands or executable paths."#
    } else {
        r#"Return only one complete JSON object matching DeskAct action-plan version 1. Schema: {"version":1,"summary":string,"steps":[{"operation":lowercase_identifier,"parameters":object,"expected_outcome":string}]}. Use 1 to 20 steps; summary and expected_outcome must be non-empty and at most 500 characters. This is a proposal, not authorization. Never include shell commands or executable paths."#
    };
    let user_request = match context {
        Some(context) => format!(
            "Eligible catalog candidates (IDs and display names only): {context}\nPropose a plan for this request: {request}"
        ),
        None => format!("Propose a plan for this request: {request}"),
    };
    let first_response = config
        .complete(vec![
            ChatMessage {
                role: "system",
                content: system.into(),
            },
            ChatMessage {
                role: "user",
                content: user_request.clone(),
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
                content: user_request.clone(),
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
