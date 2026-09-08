//! The configured AI provider, resolved once into the capabilities callers need.

use crate::types::{AiProvider, AppConfig};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";
const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiApiProtocol {
    AnthropicMessages,
    OpenAiChatCompletions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiSettingsError {
    MissingApiKey,
    MissingBaseUrl,
}

impl fmt::Display for AiSettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingApiKey => formatter
                .write_str("No API key configured. Go to Settings > AI to set up your API key."),
            Self::MissingBaseUrl => {
                formatter.write_str("No base URL configured for the OpenAI Compatible provider.")
            }
        }
    }
}

/// Secret-bearing network settings resolved once from the provider configuration.
/// Deliberately does not implement `Debug` so credentials cannot be logged accidentally.
#[derive(Clone)]
pub struct AiRequestSettings {
    protocol: AiApiProtocol,
    endpoint: String,
    api_key: Option<String>,
    model: String,
}

impl AiRequestSettings {
    pub fn protocol(&self) -> AiApiProtocol {
        self.protocol
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeProtocol {
    Ollama,
    OpenAiCompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiTargetId {
    pub endpoint: String,
    pub model: String,
    pub generation: u64,
}

/// Everything needed to probe a configured backend. The credential is excluded from the
/// serializable identity and this type deliberately does not implement `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct AiBackendTarget {
    id: AiTargetId,
    protocol: ProbeProtocol,
    bearer_token: Option<String>,
}

impl AiBackendTarget {
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        protocol: ProbeProtocol,
        bearer_token: Option<String>,
        generation: u64,
    ) -> Self {
        Self {
            id: AiTargetId {
                endpoint: endpoint.into(),
                model: model.into(),
                generation,
            },
            protocol,
            bearer_token: bearer_token.filter(|token| !token.trim().is_empty()),
        }
    }

    pub fn id(&self) -> &AiTargetId {
        &self.id
    }

    pub fn protocol(&self) -> ProbeProtocol {
        self.protocol
    }

    pub(crate) fn bearer_token(&self) -> Option<&str> {
        self.bearer_token.as_deref()
    }

    pub fn same_settings(&self, other: &Self) -> bool {
        self.id.endpoint == other.id.endpoint
            && self.id.model == other.id.model
            && self.protocol == other.protocol
            && self.bearer_token == other.bearer_token
    }
}

pub(crate) fn normalize_openai_base(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    base.strip_suffix("/v1")
        .unwrap_or(base)
        .trim_end_matches('/')
        .to_string()
}

/// A provider together with the settings that belong to it.
///
/// Callers ask this profile for capabilities instead of interpreting provider IDs or
/// selecting provider-specific fields themselves.
pub struct ConfiguredAiProvider<'a> {
    provider: AiProvider,
    config: &'a AppConfig,
}

impl<'a> ConfiguredAiProvider<'a> {
    pub fn from_config(config: &'a AppConfig) -> Self {
        Self {
            provider: config.ai_provider.clone().unwrap_or(AiProvider::Anthropic),
            config,
        }
    }

    pub fn api_key(&self) -> Option<&str> {
        match &self.provider {
            AiProvider::Anthropic | AiProvider::Unknown(_) => self.config.ai_api_key.as_deref(),
            AiProvider::OpenAi => self.config.openai_api_key.as_deref(),
            AiProvider::Ollama => self.config.ollama_api_key.as_deref(),
            AiProvider::OpenAiCompatible => self.config.openai_compatible_api_key.as_deref(),
        }
    }

    pub fn base_url(&self) -> Option<&str> {
        let configured = match &self.provider {
            AiProvider::Ollama => self.config.ollama_base_url.as_deref(),
            AiProvider::OpenAiCompatible => self.config.openai_compatible_base_url.as_deref(),
            AiProvider::Anthropic | AiProvider::OpenAi | AiProvider::Unknown(_) => None,
        };
        configured.map(str::trim).filter(|url| !url.is_empty())
    }

    pub fn can_probe(&self) -> bool {
        match &self.provider {
            AiProvider::Ollama => true,
            AiProvider::OpenAiCompatible => self.base_url().is_some(),
            AiProvider::Anthropic | AiProvider::OpenAi | AiProvider::Unknown(_) => false,
        }
    }

    pub fn requires_api_key(&self) -> bool {
        match &self.provider {
            AiProvider::Anthropic | AiProvider::OpenAi | AiProvider::Unknown(_) => true,
            AiProvider::Ollama | AiProvider::OpenAiCompatible => false,
        }
    }

    pub fn request_settings(&self) -> Result<AiRequestSettings, AiSettingsError> {
        let api_key = self
            .api_key()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string);
        if self.requires_api_key() && api_key.is_none() {
            return Err(AiSettingsError::MissingApiKey);
        }

        let (protocol, endpoint) = match &self.provider {
            AiProvider::Anthropic | AiProvider::Unknown(_) => (
                AiApiProtocol::AnthropicMessages,
                ANTHROPIC_API_URL.to_string(),
            ),
            AiProvider::OpenAi => (
                AiApiProtocol::OpenAiChatCompletions,
                OPENAI_API_URL.to_string(),
            ),
            AiProvider::Ollama => (
                AiApiProtocol::OpenAiChatCompletions,
                format!(
                    "{}/v1/chat/completions",
                    self.base_url()
                        .unwrap_or(DEFAULT_OLLAMA_URL)
                        .trim_end_matches('/')
                ),
            ),
            AiProvider::OpenAiCompatible => (
                AiApiProtocol::OpenAiChatCompletions,
                format!(
                    "{}/v1/chat/completions",
                    normalize_openai_base(self.base_url().ok_or(AiSettingsError::MissingBaseUrl)?)
                ),
            ),
        };

        Ok(AiRequestSettings {
            protocol,
            endpoint,
            api_key,
            model: self.config.ai_model.clone(),
        })
    }

    pub fn health_target(&self, generation: u64) -> Option<AiBackendTarget> {
        if !self.can_probe() {
            return None;
        }
        let (endpoint, protocol) = match &self.provider {
            AiProvider::Ollama => (
                self.base_url().unwrap_or(DEFAULT_OLLAMA_URL).to_string(),
                ProbeProtocol::Ollama,
            ),
            AiProvider::OpenAiCompatible => (
                normalize_openai_base(self.base_url()?),
                ProbeProtocol::OpenAiCompatible,
            ),
            AiProvider::Anthropic | AiProvider::OpenAi | AiProvider::Unknown(_) => return None,
        };
        Some(AiBackendTarget::new(
            endpoint,
            self.config.ai_model.clone(),
            protocol,
            self.api_key().map(str::to_string),
            generation,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{AiApiProtocol, ConfiguredAiProvider, ProbeProtocol};
    use crate::types::{AiProvider, AppConfig};

    #[test]
    fn an_openai_compatible_local_server_resolves_as_a_probeable_backend() {
        let config = AppConfig {
            ai_provider: Some(AiProvider::OpenAiCompatible),
            openai_compatible_base_url: Some("http://desktop:11434/v1".to_string()),
            openai_compatible_api_key: Some("local-secret".to_string()),
            ..AppConfig::default()
        };

        let provider = ConfiguredAiProvider::from_config(&config);

        assert_eq!(provider.base_url(), Some("http://desktop:11434/v1"));
        assert_eq!(provider.api_key(), Some("local-secret"));
        assert!(provider.can_probe());
        assert!(!provider.requires_api_key());
    }

    #[test]
    fn an_openai_compatible_profile_produces_its_own_health_target() {
        let config = AppConfig {
            ai_provider: Some(AiProvider::OpenAiCompatible),
            ai_model: "gemma3:4b".to_string(),
            openai_compatible_base_url: Some("http://desktop:11434/v1/".to_string()),
            openai_compatible_api_key: Some("local-secret".to_string()),
            ..AppConfig::default()
        };

        let target = ConfiguredAiProvider::from_config(&config)
            .health_target(7)
            .expect("a configured local backend should be tracked");

        assert_eq!(target.id().endpoint, "http://desktop:11434");
        assert_eq!(target.id().model, "gemma3:4b");
        assert_eq!(target.id().generation, 7);
        assert_eq!(target.protocol(), ProbeProtocol::OpenAiCompatible);
        assert_eq!(target.bearer_token(), Some("local-secret"));
    }

    #[test]
    fn one_resolved_request_profile_owns_provider_routing_and_validation() {
        let compatible = AppConfig {
            ai_provider: Some(AiProvider::OpenAiCompatible),
            ai_model: "gemma3:4b".to_string(),
            openai_compatible_base_url: Some(" http://desktop:11434/v1/ ".to_string()),
            ..AppConfig::default()
        };
        let request = ConfiguredAiProvider::from_config(&compatible)
            .request_settings()
            .expect("local API key is optional");

        assert_eq!(request.protocol(), AiApiProtocol::OpenAiChatCompletions);
        assert_eq!(
            request.endpoint(),
            "http://desktop:11434/v1/chat/completions"
        );
        assert_eq!(request.api_key(), None);
        assert_eq!(request.model(), "gemma3:4b");

        let anthropic_without_key = AppConfig {
            ai_provider: Some(AiProvider::Anthropic),
            ai_api_key: None,
            ..AppConfig::default()
        };
        assert!(ConfiguredAiProvider::from_config(&anthropic_without_key)
            .request_settings()
            .is_err());
    }
}
