//! Model integration module for Anda Engine
//!
//! This module provides implementations for various AI model providers, including:
//! - OpenAI (completion and embedding models)
//! - DeepSeek (completion models)
//! - Cohere (embedding models)
//!
//! Each provider implementation includes:
//! - Client configuration and management
//! - API request/response handling
//! - Conversion to Anda's internal data structures
//!
//! The module is designed to be extensible, allowing easy addition of new model providers
//! while maintaining a consistent interface through the `CompletionFeaturesDyn` and
//! `EmbeddingFeaturesDyn` traits.

use anda_core::{AgentOutput, BoxError, BoxPinFut, CompletionRequest, Embedding, ToolCall, Usage};
use std::sync::Arc;

pub mod cohere;
pub mod deepseek;
pub mod gemini;
pub mod kimi;
pub mod openai;
pub mod xai;

/// Trait for dynamic completion features that can be used across threads
pub trait CompletionFeaturesDyn: Send + Sync + 'static {
    /// Performs a completion request and returns a future with the agent's output
    fn completion(&self, req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>>;
}

/// Trait for dynamic embedding features that can be used across threads
pub trait EmbeddingFeaturesDyn: Send + Sync + 'static {
    /// Returns the number of dimensions for the embedding model
    fn ndims(&self) -> usize;

    /// Embeds multiple texts and returns a future with the resulting embeddings
    fn embed(&self, texts: Vec<String>) -> BoxPinFut<Result<(Vec<Embedding>, Usage), BoxError>>;

    /// Embeds a single query text and returns a future with the resulting embedding
    fn embed_query(&self, text: String) -> BoxPinFut<Result<(Embedding, Usage), BoxError>>;
}

/// A placeholder implementation for unimplemented features
#[derive(Clone, Debug)]
pub struct NotImplemented;

impl CompletionFeaturesDyn for NotImplemented {
    fn completion(&self, _req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
        Box::pin(futures::future::ready(Err("not implemented".into())))
    }
}

impl EmbeddingFeaturesDyn for NotImplemented {
    fn ndims(&self) -> usize {
        0
    }

    fn embed(&self, _texts: Vec<String>) -> BoxPinFut<Result<(Vec<Embedding>, Usage), BoxError>> {
        Box::pin(futures::future::ready(Err("not implemented".into())))
    }

    fn embed_query(&self, _text: String) -> BoxPinFut<Result<(Embedding, Usage), BoxError>> {
        Box::pin(futures::future::ready(Err("not implemented".into())))
    }
}

/// A mock implementation for testing purposes
#[derive(Clone, Debug)]
pub struct MockImplemented;

impl CompletionFeaturesDyn for MockImplemented {
    fn completion(&self, req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
        Box::pin(futures::future::ready(Ok(AgentOutput {
            content: req.prompt.clone(),
            tool_calls: req
                .tools
                .iter()
                .map(|tool| ToolCall {
                    id: tool.name.clone(),
                    name: tool.name.clone(),
                    args: req.prompt.clone(),
                    result: None,
                })
                .collect(),
            ..Default::default()
        })))
    }
}

impl EmbeddingFeaturesDyn for MockImplemented {
    fn ndims(&self) -> usize {
        384 // EMBED_MULTILINGUAL_LIGHT_V3
    }

    fn embed(&self, texts: Vec<String>) -> BoxPinFut<Result<(Vec<Embedding>, Usage), BoxError>> {
        Box::pin(futures::future::ready(Ok((
            texts
                .into_iter()
                .map(|text| Embedding {
                    text,
                    vec: vec![0.0; 384],
                })
                .collect(),
            Usage::default(),
        ))))
    }

    fn embed_query(&self, _text: String) -> BoxPinFut<Result<(Embedding, Usage), BoxError>> {
        Box::pin(futures::future::ready(Ok((
            Embedding {
                text: "test".to_string(),
                vec: vec![0.0; 384],
            },
            Usage::default(),
        ))))
    }
}

/// Main model struct that combines embedding and completion capabilities
#[derive(Clone)]
pub struct Model {
    /// Embedding feature implementation
    pub embedder: Arc<dyn EmbeddingFeaturesDyn>,
    /// Completion feature implementation
    pub completer: Arc<dyn CompletionFeaturesDyn>,
}

impl Model {
    /// Creates a new Model with specified embedder and completer
    pub fn new(
        completer: Arc<dyn CompletionFeaturesDyn>,
        embedder: Arc<dyn EmbeddingFeaturesDyn>,
    ) -> Self {
        Self {
            embedder,
            completer,
        }
    }

    /// Creates a Model with only completion features
    pub fn with_completer(completer: Arc<dyn CompletionFeaturesDyn>) -> Self {
        Self {
            completer,
            embedder: Arc::new(NotImplemented),
        }
    }

    /// Creates a Model with unimplemented features (returns errors for all operations)
    pub fn not_implemented() -> Self {
        Self {
            completer: Arc::new(NotImplemented),
            embedder: Arc::new(NotImplemented),
        }
    }

    /// Creates a Model with mock implementations for testing
    pub fn mock_implemented() -> Self {
        Self {
            completer: Arc::new(MockImplemented),
            embedder: Arc::new(MockImplemented),
        }
    }

    pub async fn completion(&self, req: CompletionRequest) -> Result<AgentOutput, BoxError> {
        self.completer.completion(req).await
    }

    pub fn ndims(&self) -> usize {
        self.embedder.ndims()
    }

    pub async fn embed(
        &self,
        texts: impl IntoIterator<Item = String> + Send,
    ) -> Result<(Vec<Embedding>, Usage), BoxError> {
        self.embedder.embed(texts.into_iter().collect()).await
    }

    pub async fn embed_query(&self, text: &str) -> Result<(Embedding, Usage), BoxError> {
        self.embedder.embed_query(text.to_string()).await
    }
}
