use anda_core::{
    AgentOutput, BoxError, FunctionDefinition, Message, ToolCall, Usage as ModelUsage,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;

// https://ai.google.dev/api/generate-content

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    /// Optional. Developer set system instruction(s). Currently, text only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contents: Vec<Content>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<ToolConfig>,

    pub generation_config: GenerationConfig,
}

/// Response from the model supporting multiple candidate responses.
///
/// Safety ratings and content filtering are reported for both prompt in
/// GenerateContentResponse.prompt_feedback and for each candidate in
/// finishReason and in safetyRatings.
///
/// The API:
/// - Returns either all requested candidates or none of them
/// - Returns no candidates at all only if there was something wrong with the
///   prompt (check promptFeedback)
/// - Reports feedback on each candidate in finishReason and safetyRatings.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentResponse {
    pub candidates: Vec<Candidate>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_feedback: Option<PromptFeedback>,

    pub usage_metadata: UsageMetadata,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
}

impl GenerateContentResponse {
    pub fn try_into(self, full_history: Vec<Value>) -> Result<AgentOutput, BoxError> {
        let mut output = AgentOutput {
            full_history,
            usage: ModelUsage {
                input_tokens: self.usage_metadata.prompt_token_count as u64,
                output_tokens: self.usage_metadata.candidates_token_count as u64,
                requests: 1,
            },
            ..Default::default()
        };

        for candidate in self.candidates {
            // candidate.content.role = Some(Role::Model);
            output
                .full_history
                .push(json!(Message::from(&candidate.content)));
            match candidate.finish_reason {
                Some(FinishReason::Stop) => {
                    let (content, tool_calls) = candidate.content.to_output();
                    output.content = content;
                    output.tool_calls = tool_calls;
                    break;
                }
                _ => {
                    output.failed_reason = serde_json::to_string(&candidate).ok();
                }
            }
        }

        if let Some(feedback) = self.prompt_feedback {
            output.failed_reason = serde_json::to_string(&feedback).ok();
        }

        Ok(output)
    }

    pub fn maybe_failed(&self) -> bool {
        self.prompt_feedback.is_some()
            || !self.candidates.iter().any(|candidate| {
                matches!(candidate.finish_reason.as_ref(), Some(FinishReason::Stop))
            })
    }
}

/// A response candidate generated from the model.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// Generated content returned from the model.
    pub content: Content,
    /// The reason why the model stopped generating tokens. If empty, the model
    /// has not stopped generating tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    /// List of ratings for the safety of a response candidate. There is at most
    /// one rating per category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub satefy_ratings: Option<Vec<SatisfyRating>>,
    /// Citation information for model-generated candidate.
    ///
    /// This field may be populated with recitation information for any text
    /// included in the content. These are passages that are "recited" from
    /// copyrighted material in the foundational LLM's training data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citation_metadata: Option<CitationMetadata>,
    /// Token count for this candidate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_count: Option<u32>,
    /// Average log probability score of the candidate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_logprobs: Option<f32>,
    /// Index of the candidate in the list of response candidates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    // Optional. The producer of the content. Must be either 'user' or 'model'.
    // Useful to set for multi-turn conversations, otherwise can be left blank or unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,

    pub parts: Vec<ContentPart>,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContentPart {
    /// whether or not the part is a reasoning/thinking text or not
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought: Option<bool>,
    /// an opaque sig for the thought so it can be reused - is a base64 string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,

    #[serde(flatten)]
    pub data: PartKind,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PartKind {
    CodeExecutionResult {
        outcome: String,
        output: String,
    },
    ExecutableCode {
        language: String,
        code: String,
    },
    FileData {
        file_uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    FunctionCall {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    FunctionResponse {
        name: String,
        response: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        will_continue: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        scheduling: Option<String>,
    },
    InlineData {
        mime_type: String,
        data: String,
    },
    Text(String),
}

impl Default for PartKind {
    fn default() -> Self {
        Self::Text(String::default())
    }
}

impl Content {
    pub fn to_output(&self) -> (String, Vec<ToolCall>) {
        let mut texts: Vec<&str> = Vec::new();
        let mut tools: Vec<ToolCall> = Vec::new();
        for part in &self.parts {
            if let PartKind::Text(text) = &part.data
                && part.thought != Some(true)
            {
                texts.push(text);
            } else if let PartKind::FunctionCall { name, args, id } = &part.data {
                tools.push(ToolCall {
                    id: id.clone().unwrap_or_default(),
                    name: name.clone(),
                    args: serde_json::to_string(args).unwrap_or_default(),
                    result: None,
                });
            }
        }
        (texts.join("\n"), tools)
    }
}

impl TryFrom<Value> for ContentPart {
    type Error = serde_json::Error;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(s) => Ok(Self {
                data: PartKind::Text(s),
                ..Default::default()
            }),
            _ => serde_json::from_value(value),
        }
    }
}

fn into_parts(value: Value) -> Result<Vec<ContentPart>, serde_json::Error> {
    match value {
        Value::Array(list) => list
            .into_iter()
            .map(ContentPart::try_from)
            .collect::<Result<_, _>>(),
        v => Ok(vec![ContentPart::try_from(v)?]),
    }
}

impl TryFrom<Value> for Content {
    type Error = serde_json::Error;

    fn try_from(msg: Value) -> Result<Self, Self::Error> {
        if msg.get("parts").is_some() {
            return serde_json::from_value(msg);
        }

        let msg: Message = serde_json::from_value(msg)?;
        Self::try_from(msg)
    }
}

impl TryFrom<Message> for Content {
    type Error = serde_json::Error;

    fn try_from(msg: Message) -> Result<Self, Self::Error> {
        match msg.role.as_str() {
            "user" => Ok(Self {
                role: Some(Role::User),
                parts: into_parts(msg.content)?,
            }),
            "tool" => Ok(Self {
                role: Some(Role::User),
                parts: vec![ContentPart {
                    data: PartKind::FunctionResponse {
                        id: msg.tool_call_id,
                        name: msg
                            .name
                            .map(|n| n.trim_start_matches("$").to_string())
                            .unwrap(),
                        response: match msg.content {
                            Value::String(s) => {
                                serde_json::from_str(&s).unwrap_or_else(|_| s.into())
                            }
                            v => v,
                        },
                        scheduling: None,
                        will_continue: None,
                    },
                    ..Default::default()
                }],
            }),
            _ => Ok(Self {
                role: Some(Role::Model),
                parts: into_parts(msg.content)?,
            }),
        }
    }
}

impl From<&Content> for Message {
    fn from(content: &Content) -> Self {
        Self {
            role: content.role.unwrap_or_default().to_string(),
            content: content.parts.iter().map(|v| json!(v)).collect(),
            tool_call_id: None,
            name: None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    User,
    Model,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Model => write!(f, "assistant"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase")]
pub enum Tool {
    FunctionDeclaration {
        function_declarations: Vec<FunctionDeclaration>,
    },

    CodeExecution {
        code_execution: CodeExecution,
    },
}

impl From<Vec<FunctionDefinition>> for Tool {
    fn from(tools: Vec<FunctionDefinition>) -> Self {
        Self::FunctionDeclaration {
            function_declarations: tools
                .into_iter()
                .map(|v| FunctionDeclaration {
                    name: v.name,
                    description: v.description,
                    parameters_json_schema: Some(v.parameters),
                    response_json_schema: None,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeExecution {}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfig {
    pub function_calling_config: FunctionCallingConfig,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            function_calling_config: FunctionCallingConfig {
                mode: FunctionCallingMode::Auto,
                allowed_function_names: None,
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCallingConfig {
    pub mode: FunctionCallingMode,
    /// A set of function names that, when provided, limits the functions the
    /// model will call.
    ///
    /// This should only be set when the Mode is ANY. Function names should match
    /// [FunctionDeclaration.name]. With mode set to ANY, model will predict a
    /// function call from the set of function names provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_function_names: Option<Vec<String>>,
}

/// Defines the execution behavior for function calling by defining the execution
/// mode.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FunctionCallingMode {
    /// Unspecified function calling mode. This value should not be used.
    #[default]
    ModeUnspecified,
    /// Default model behavior, model decides to predict either a function call
    /// or a natural language response.
    Auto,
    /// Model is constrained to always predicting a function call only. If
    /// "allowedFunctionNames" are set, the predicted function call will be
    /// limited to any one of "allowedFunctionNames", else the predicted
    /// function call will be any one of the provided "functionDeclarations".
    Any,
    /// Model will not predict any function call. Model behavior is same as when
    /// not passing any function declarations.
    None,
}

/// Gemini API Configuration options for model generation and outputs. Not all parameters are
/// configurable for every model. From [Gemini API Reference](https://ai.google.dev/api/generate-content#generationconfig)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    /// The set of character sequences (up to 5) that will stop output generation. If specified, the API will stop
    /// at the first appearance of a stop_sequence. The stop sequence will not be included as part of the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,

    /// MIME type of the generated candidate text. Supported MIME types are:
    ///     - text/plain:  (default) Text output
    ///     - application/json: JSON response in the response candidates.
    ///     - text/x.enum: ENUM as a string response in the response candidates.
    /// Refer to the docs for a list of all supported text MIME types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_mime_type: Option<String>,

    /// Output schema of the generated candidate text. Schemas must be a subset of the OpenAPI schema and can be
    /// objects, primitives or arrays. If set, a compatible responseMimeType must also  be set. Compatible MIME
    /// types: application/json: Schema for JSON response. Refer to the JSON text generation guide for more details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_modalities: Option<Vec<String>>,

    /// Number of generated responses to return. Currently, this value can only be set to 1. If
    /// unset, this will default to 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<i32>,

    /// The maximum number of tokens to include in a response candidate. Note: The default value varies by model, see
    /// the Model.output_token_limit attribute of the Model returned from the getModel function.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<i32>,

    /// Controls the randomness of the output. Note: The default value varies by model, see the Model.temperature
    /// attribute of the Model returned from the getModel function. Values can range from [0.0, 2.0].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// The maximum cumulative probability of tokens to consider when sampling. The model uses combined Top-k and
    /// Top-p (nucleus) sampling. Tokens are sorted based on their assigned probabilities so that only the most
    /// likely tokens are considered. Top-k sampling directly limits the maximum number of tokens to consider, while
    /// Nucleus sampling limits the number of tokens based on the cumulative probability. Note: The default value
    /// varies by Model and is specified by theModel.top_p attribute returned from the getModel function. An empty
    /// topK attribute indicates that the model doesn't apply top-k sampling and doesn't allow setting topK on requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    /// The maximum number of tokens to consider when sampling. Gemini models use Top-p (nucleus) sampling or a
    /// combination of Top-k and nucleus sampling. Top-k sampling considers the set of topK most probable tokens.
    /// Models running with nucleus sampling don't allow topK setting. Note: The default value varies by Model and is
    /// specified by theModel.top_p attribute returned from the getModel function. An empty topK attribute indicates
    /// that the model doesn't apply top-k sampling and doesn't allow setting topK on requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,

    /// Presence penalty applied to the next token's logprobs if the token has already been seen in the response.
    /// This penalty is binary on/off and not dependent on the number of times the token is used (after the first).
    /// Use frequencyPenalty for a penalty that increases with each use. A positive penalty will discourage the use
    /// of tokens that have already been used in the response, increasing the vocabulary. A negative penalty will
    /// encourage the use of tokens that have already been used in the response, decreasing the vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,

    /// Frequency penalty applied to the next token's logprobs, multiplied by the number of times each token has been
    /// seen in the response so far. A positive penalty will discourage the use of tokens that have already been
    /// used, proportional to the number of times the token has been used: The more a token is used, the more
    /// difficult it is for the  model to use that token again increasing the vocabulary of responses. Caution: A
    /// negative penalty will encourage the model to reuse tokens proportional to the number of times the token has
    /// been used. Small negative values will reduce the vocabulary of a response. Larger negative values will cause
    /// the model to  repeating a common token until it hits the maxOutputTokens limit: "...the the the the the...".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,

    /// If true, export the logprobs results in response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_logprobs: Option<bool>,

    /// Only valid if responseLogprobs=True. This sets the number of top logprobs to return at each decoding step in
    /// [Candidate.logprobs_result].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<i32>,

    /// Configuration for thinking/reasoning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<ThinkingConfig>,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            temperature: Some(1.0),
            max_output_tokens: Some(65535),
            stop_sequences: None,
            response_mime_type: None,
            response_schema: None,
            response_modalities: None,
            candidate_count: None,
            top_p: None,
            top_k: None,
            presence_penalty: None,
            frequency_penalty: None,
            response_logprobs: None,
            logprobs: None,
            thinking_config: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDeclaration {
    pub name: String,
    pub description: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters_json_schema: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_json_schema: Option<Value>,
}

/// A set of the feedback metadata the prompt specified in [GenerateContentRequest.contents](GenerateContentRequest).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptFeedback {
    /// Optional. If set, the prompt was blocked and no candidates are returned. Rephrase the prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<BlockReason>,
    /// Ratings for safety of the prompt. There is at most one rating per category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_ratings: Option<Vec<SatisfyRating>>,
}

/// Reason why a prompt was blocked by the model
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BlockReason {
    /// Default value. This value is unused.
    BlockReasonUnspecified,
    /// Prompt was blocked due to safety reasons. Inspect safetyRatings to understand which safety category blocked it.
    Safety,
    /// Prompt was blocked due to unknown reasons.
    Other,
    /// Prompt was blocked due to the terms which are included from the terminology blocklist.
    Blocklist,
    /// Prompt was blocked due to prohibited content.
    ProhibitedContent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadata {
    pub prompt_token_count: u32,

    pub candidates_token_count: u32,

    pub total_token_count: u32,

    #[serde(default)]
    pub thoughts_token_count: u32,
}

/// Config for thinking features.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    /// Indicates whether to include thoughts in the response. If true, thoughts
    /// are returned only when available.
    pub include_thoughts: bool,
    /// The number of thoughts tokens that the model should generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CitationMetadata {
    #[serde(default)]
    pub citation_sources: Vec<CitationSource>,
}

/// CitationSource
///
/// A citation to a source for a portion of a specific response.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CitationSource {
    /// Start of segment of the response that is attributed to this source.
    /// Index indicates the start of the segment, measured in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_index: Option<u32>,
    /// End of the attributed segment, exclusive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_index: Option<u32>,
    /// URI that is attributed as a source for a portion of the text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// License for the GitHub project that is attributed as a source for
    /// segment. License info is required for code citations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

/// Safety rating for a piece of content.
///
/// The safety rating contains the category of harm and the harm probability
/// level in that category for a piece of content. Content is classified for
/// safety across a number of harm categories and the probability of the harm
/// classification is included here.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SatisfyRating {
    /// The category for this rating.
    pub category: HarmCategory,
    /// The probability of harm for this content.
    pub probability: HarmProbability,
    /// Was this content blocked because of this rating?
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HarmProbability {
    /// Default value. This value is unused.
    #[default]
    HarmProbabilityUnspecified,
    /// Content has a negligible chance of being unsafe.
    Negligible,
    /// Content has a low chance of being unsafe.
    Low,
    /// Content has a medium chance of being unsafe.
    Medium,
    /// Content has a high chance of being unsafe.
    High,
}

// HarmCategory
//
// The category of a rating.
//
// These categories cover various kinds of harms that developers may wish to
// adjust.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HarmCategory {
    #[default]
    HarmCategoryUnspecified,
    HarmCategoryDerogatory,
    HarmCategoryToxicity,
    HarmCategoryViolence,
    HarmCategorySexually,
    HarmCategoryMedical,
    HarmCategoryDangerous,
    HarmCategoryHarassment,
    HarmCategoryHateSpeech,
    HarmCategorySexuallyExplicit,
    HarmCategoryDangerousContent,
    HarmCategoryCivicIntegrity,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FinishReason {
    /// Default value. This value is unused.
    #[default]
    FinishReasonUnspecified,
    /// Natural stop point of the model or provided stop sequence.
    Stop,
    /// The maximum number of tokens as specified in the request was reached.
    MaxTokens,
    /// The response candidate content was flagged for safety reasons.
    Safety,
    /// The response candidate content was flagged for recitation reasons.
    Recitation,
    /// The response candidate content was flagged for using an unsupported
    /// language.
    Language,
    /// Unknown reason.
    Other,
    /// Token generation stopped because the content contains forbidden terms.
    Blocklist,
    /// Token generation stopped for potentially containing prohibited content.
    ProhibitedContent,
    /// Token generation stopped because the content potentially contains
    /// Sensitive Personally Identifiable Information (SPII).
    Spii,
    /// The function call generated by the model is invalid.
    MalformedFunctionCall,
    /// Token generation stopped because generated images contain safety
    /// violations.
    ImageSafety,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_content_part() {
        // Test Text variant
        let text_part = ContentPart {
            thought: None,
            thought_signature: None,
            data: PartKind::Text("Hello world".to_string()),
        };
        let json_value = serde_json::to_value(&text_part).unwrap();
        assert_eq!(
            json_value,
            json!({
                "text": "Hello world"
            })
        );
        let deserialized: ContentPart = serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, text_part);

        // Test Text with thought metadata
        let thought_text_part = ContentPart {
            thought: Some(true),
            thought_signature: Some("base64signature".to_string()),
            data: PartKind::Text("This is a thought".to_string()),
        };
        let json_value = serde_json::to_value(&thought_text_part).unwrap();
        assert_eq!(
            json_value,
            json!({
                "thought": true,
                "thoughtSignature": "base64signature",
                "text": "This is a thought"
            })
        );
        let deserialized: ContentPart = serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, thought_text_part);

        // Test FunctionCall variant
        let function_call_part = ContentPart {
            thought: None,
            thought_signature: None,
            data: PartKind::FunctionCall {
                name: "get_weather".to_string(),
                args: Some(json!({"location": "Shanghai"})),
                id: Some("call_123".to_string()),
            },
        };
        let json_value = serde_json::to_value(&function_call_part).unwrap();
        assert_eq!(
            json_value,
            json!({
                "functionCall": {
                    "name": "get_weather",
                    "args": {"location": "Shanghai"},
                    "id": "call_123"
                }
            })
        );
        let deserialized: ContentPart = serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, function_call_part);

        // Test FunctionResponse variant
        let function_response_part = ContentPart {
            thought: None,
            thought_signature: None,
            data: PartKind::FunctionResponse {
                name: "get_weather".to_string(),
                response: json!({"temperature": "25°C", "condition": "sunny"}),
                id: Some("call_123".to_string()),
                will_continue: Some(false),
                scheduling: None,
            },
        };
        let json_value = serde_json::to_value(&function_response_part).unwrap();
        assert_eq!(
            json_value,
            json!({
                "functionResponse": {
                    "name": "get_weather",
                    "response": {"temperature": "25°C", "condition": "sunny"},
                    "id": "call_123",
                    "willContinue": false
                }
            })
        );
        let deserialized: ContentPart = serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, function_response_part);

        // Test InlineData variant
        let inline_data_part = ContentPart {
            thought: None,
            thought_signature: None,
            data: PartKind::InlineData {
                mime_type: "image/jpeg".to_string(),
                data: "base64encodedimagedata".to_string(),
            },
        };
        let json_value = serde_json::to_value(&inline_data_part).unwrap();
        assert_eq!(
            json_value,
            json!({
                "inlineData": {
                    "mimeType": "image/jpeg",
                    "data": "base64encodedimagedata"
                }
            })
        );
        let deserialized: ContentPart = serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, inline_data_part);

        // Test FileData variant
        let file_data_part = ContentPart {
            thought: None,
            thought_signature: None,
            data: PartKind::FileData {
                file_uri: "gs://my-bucket/image.jpg".to_string(),
                mime_type: Some("image/jpeg".to_string()),
            },
        };
        let json_value = serde_json::to_value(&file_data_part).unwrap();
        assert_eq!(
            json_value,
            json!({
                "fileData": {
                    "fileUri": "gs://my-bucket/image.jpg",
                    "mimeType": "image/jpeg"
                }
            })
        );
        let deserialized: ContentPart = serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, file_data_part);

        // Test ExecutableCode variant
        let executable_code_part = ContentPart {
            thought: None,
            thought_signature: None,
            data: PartKind::ExecutableCode {
                language: "python".to_string(),
                code: "print('Hello, World!')".to_string(),
            },
        };
        let json_value = serde_json::to_value(&executable_code_part).unwrap();
        assert_eq!(
            json_value,
            json!({
                "executableCode": {
                    "language": "python",
                    "code": "print('Hello, World!')"
                }
            })
        );
        let deserialized: ContentPart = serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, executable_code_part);

        // Test CodeExecutionResult variant
        let code_result_part = ContentPart {
            thought: None,
            thought_signature: None,
            data: PartKind::CodeExecutionResult {
                outcome: "success".to_string(),
                output: "Hello, World!".to_string(),
            },
        };
        let json_value = serde_json::to_value(&code_result_part).unwrap();
        assert_eq!(
            json_value,
            json!({
                "codeExecutionResult": {
                    "outcome": "success",
                    "output": "Hello, World!"
                }
            })
        );
        let deserialized: ContentPart = serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, code_result_part);

        // Test default ContentPart
        let default_part = ContentPart::default();
        let json_value = serde_json::to_value(&default_part).unwrap();
        assert_eq!(json_value, json!({"text": ""}));
        let deserialized: ContentPart = serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, default_part);

        // Test TryFrom<Value> for ContentPart with string
        let string_value = json!("Simple text");
        let content_part: ContentPart = ContentPart::try_from(string_value.clone()).unwrap();
        assert_eq!(content_part.data, PartKind::Text("Simple text".to_string()));
        assert_eq!(content_part.thought, None);
        assert_eq!(content_part.thought_signature, None);

        let val = into_parts(string_value.clone()).unwrap();
        assert_eq!(val, vec![content_part.clone()]);

        // Test TryFrom<Value> for ContentPart with complex object
        let complex_value = json!({
            "thought": true,
            "thoughtSignature": "abc123",
            "functionCall": {
                "name": "test_function",
                "args": {"param": "value"}
            }
        });
        let content_part2: ContentPart = ContentPart::try_from(complex_value.clone()).unwrap();
        assert_eq!(content_part2.thought, Some(true));
        assert_eq!(content_part2.thought_signature, Some("abc123".to_string()));
        if let PartKind::FunctionCall { name, args, id: _ } = &content_part2.data {
            assert_eq!(name, "test_function");
            assert_eq!(args, &Some(json!({"param": "value"})));
        } else {
            panic!("Expected FunctionCall variant");
        }

        let val = into_parts(complex_value.clone()).unwrap();
        assert_eq!(val, vec![content_part2.clone()]);

        let val = into_parts(json!(vec![string_value, complex_value])).unwrap();
        assert_eq!(val, vec![content_part, content_part2]);
    }
}
