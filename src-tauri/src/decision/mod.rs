//! Evaluation-only System One clients. No product route or UI selectability.

pub mod eval;

use audio_graph_decision_contract::{
    Answer, DecisionEngine, DecisionError, DecisionRequest, DecisionResponse, Egress, EngineLimits,
    Question, WireResponse, async_trait,
};
use reqwest::{Client, Url, header};
use serde::Serialize;
use std::time::Duration;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudflareModel {
    Jev,
    ClefFlash,
}

#[derive(Debug, Clone)]
pub enum EndpointKind {
    TypeSafe,
    Cloudflare {
        account_id: String,
        model: CloudflareModel,
    },
    Ollama {
        url: String,
        model: String,
    },
    Vllm {
        url: String,
        model: String,
    },
}

impl EndpointKind {
    pub fn credential_key(&self) -> Option<&'static str> {
        match self {
            Self::TypeSafe => Some("typesafe_api_key"),
            Self::Cloudflare { .. } => Some("cloudflare_workers_ai_token"),
            Self::Ollama { .. } | Self::Vllm { .. } => None,
        }
    }

    pub fn limits(&self) -> EngineLimits {
        let (questions, choices, tokens) = match self {
            Self::TypeSafe => (usize::MAX, 255, Some(32_768)),
            Self::Cloudflare {
                model: CloudflareModel::Jev,
                ..
            } => (usize::MAX, 255, Some(32_000)),
            Self::Cloudflare {
                model: CloudflareModel::ClefFlash,
                ..
            } => (64, 255, Some(24_576)),
            Self::Ollama { .. } | Self::Vllm { .. } => (64, 26, None),
        };
        EngineLimits {
            max_questions: questions,
            max_choice_options: choices,
            max_score_levels: 10,
            max_state_tokens: tokens,
        }
    }

    fn model(&self) -> &str {
        match self {
            Self::TypeSafe => "jev-latest",
            Self::Cloudflare {
                model: CloudflareModel::Jev,
                ..
            } => "typesafe/jev",
            Self::Cloudflare {
                model: CloudflareModel::ClefFlash,
                ..
            } => "clef-flash",
            Self::Ollama { model, .. } | Self::Vllm { model, .. } => model,
        }
    }

    fn url(&self) -> Result<Url, DecisionError> {
        let url = match self {
            Self::TypeSafe => "https://api.typesafe.ai/v1/systemone".to_owned(),
            Self::Cloudflare { account_id, model } => {
                if account_id.is_empty() || !account_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
                    return Err(DecisionError::Internal);
                }
                match model {
                    CloudflareModel::Jev => {
                        format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/run")
                    }
                    CloudflareModel::ClefFlash => format!(
                        "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/run/@cf/cloudflare/clef-flash"
                    ),
                }
            }
            Self::Ollama { url, model } | Self::Vllm { url, model } => {
                // A local engine must never silently become a cloud relay.
                if model.contains(":cloud") {
                    return Err(DecisionError::UnsupportedQuestion);
                }
                url.clone()
            }
        };
        let url = Url::parse(&url).map_err(|_| DecisionError::Internal)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(DecisionError::Internal);
        }
        if self.credential_key().is_none() {
            let loopback = match url.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                // Use IP literals, not DNS names that could resolve off-device.
                _ => false,
            };
            if !loopback || !matches!(url.scheme(), "http" | "https") {
                return Err(DecisionError::Internal);
            }
        }
        Ok(url)
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialReadiness {
    Saved,
    Missing,
}

/// Presence only. Never returns a saved plaintext credential to a renderer.
pub fn credential_readiness(kind: &EndpointKind) -> Result<CredentialReadiness, DecisionError> {
    let Some(key) = kind.credential_key() else {
        return Ok(CredentialReadiness::Saved);
    };
    let store =
        crate::credentials::try_load_credentials().map_err(|_| DecisionError::Credential)?;
    Ok(
        if store
            .is_present(key)
            .map_err(|_| DecisionError::Credential)?
        {
            CredentialReadiness::Saved
        } else {
            CredentialReadiness::Missing
        },
    )
}

pub struct SystemOneHttpEngine {
    kind: EndpointKind,
    url: Url,
    client: Client,
    credential: Option<Zeroizing<String>>,
}

impl SystemOneHttpEngine {
    /// Desktop path: credentials come only from the existing credential backend.
    pub fn from_saved(kind: EndpointKind) -> Result<Self, DecisionError> {
        let credential = if let Some(key) = kind.credential_key() {
            crate::credentials::try_load_credentials()
                .map_err(|_| DecisionError::Credential)?
                .get_owned(key)
                .map_err(|_| DecisionError::Credential)?
        } else {
            None
        };
        Self::new(kind, credential)
    }

    /// Evaluation harness only: environment override, then saved credentials.
    pub fn for_eval(kind: EndpointKind) -> Result<Self, DecisionError> {
        let variable = match kind {
            EndpointKind::TypeSafe => Some("TYPESAFE_API_KEY"),
            EndpointKind::Cloudflare { .. } => Some("CLOUDFLARE_API_TOKEN"),
            _ => None,
        };
        if let Some(value) = variable
            .and_then(|name| std::env::var(name).ok())
            .filter(|s| !s.trim().is_empty())
        {
            return Self::new(kind, Some(value));
        }
        Self::from_saved(kind)
    }

    pub fn new(kind: EndpointKind, credential: Option<String>) -> Result<Self, DecisionError> {
        let credential = credential.map(Zeroizing::new);
        let url = kind.url()?;
        if kind.credential_key().is_some()
            && credential.as_ref().is_none_or(|s| s.trim().is_empty())
        {
            return Err(DecisionError::Credential);
        }
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none());
        if kind.credential_key().is_none() {
            builder = builder.no_proxy();
        }
        let client = builder.build().map_err(|_| DecisionError::Internal)?;
        Ok(Self {
            kind,
            url,
            client,
            credential,
        })
    }

    fn diagnostic(&self, message: &str) -> DecisionError {
        DecisionError::Transport(crate::error::redacted_provider_diagnostic(
            message,
            self.credential.iter().map(|s| s.as_str()),
        ))
    }

    pub async fn evaluate_wire(
        &self,
        req: &DecisionRequest,
    ) -> Result<WireResponse, DecisionError> {
        self.limits().validate(req)?;
        if !matches!(self.kind, EndpointKind::Vllm { .. }) {
            for (id, question) in &req.questions {
                if matches!(question, Question::Choice { criteria, .. } if criteria.len() < 2) {
                    return Err(DecisionError::CardinalityLimit {
                        question: id.clone(),
                        limit: self.limits().max_choice_options,
                    });
                }
            }
        }
        #[derive(Serialize)]
        struct Body<'a> {
            model: &'a str,
            #[serde(flatten)]
            request: &'a DecisionRequest,
        }
        #[derive(Serialize)]
        struct CloudflareBody<'a> {
            model: &'a str,
            input: &'a DecisionRequest,
        }
        let request = self.client.post(self.url.clone());
        let mut request = if matches!(
            self.kind,
            EndpointKind::Cloudflare {
                model: CloudflareModel::Jev,
                ..
            }
        ) {
            request.json(&CloudflareBody {
                model: self.kind.model(),
                input: req,
            })
        } else {
            request.json(&Body {
                model: self.kind.model(),
                request: req,
            })
        };
        if let Some(key) = &self.credential {
            let mut value = header::HeaderValue::from_str(&format!("Bearer {}", key.as_str()))
                .map_err(|_| DecisionError::Credential)?;
            value.set_sensitive(true);
            request = request.header(header::AUTHORIZATION, value);
        }
        let response = request
            .send()
            .await
            .map_err(|_| self.diagnostic("System One request failed (details omitted)"))?;
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(DecisionError::Credential);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if body.contains("max_tokens_exceeded") || body.contains("context_length_exceeded") {
                return Err(DecisionError::StateTooLong);
            }
            // Never echo provider bodies: they may contain state or credentials.
            return Err(self.diagnostic(&format!(
                "System One HTTP {} (body omitted)",
                status.as_u16()
            )));
        }
        let body = response
            .text()
            .await
            .map_err(|_| self.diagnostic("System One response read failed"))?;
        let wire = if matches!(self.kind, EndpointKind::Cloudflare { .. }) {
            #[derive(serde::Deserialize)]
            struct Envelope {
                result: Box<serde_json::value::RawValue>,
                success: bool,
            }
            let envelope: Envelope = serde_json::from_str(&body)
                .map_err(|_| self.diagnostic("Invalid Cloudflare response"))?;
            if !envelope.success {
                return Err(self.diagnostic("Cloudflare evaluation failed"));
            }
            WireResponse::from_json(envelope.result.get())
        } else {
            WireResponse::from_json(&body)
        }
        .map_err(|_| self.diagnostic("Invalid System One response"))?;
        validate_answers(req, wire.typed())
            .map_err(|_| self.diagnostic("System One answer does not match request schema"))?;
        Ok(wire)
    }
}

fn validate_answers(req: &DecisionRequest, response: &DecisionResponse) -> Result<(), ()> {
    if req.questions.len() != response.answers.len() {
        return Err(());
    }
    let probability = |p: &f64| p.is_finite() && (0.0..=1.0).contains(p);
    for (id, question) in &req.questions {
        let answer = response.answers.get(id).ok_or(())?;
        let valid = match (question, answer) {
            (
                Question::Choice { criteria, .. },
                Answer::Choice {
                    choice,
                    probabilities,
                    confidence,
                },
            ) => {
                criteria.contains_key(choice)
                    && criteria.len() == probabilities.len()
                    && criteria
                        .keys()
                        .all(|key| probabilities.get(key).is_some_and(probability))
                    && probability(confidence)
            }
            (
                Question::Noul { .. },
                Answer::Noul {
                    noul,
                    probabilities,
                    confidence,
                },
            ) => {
                probability(noul)
                    && probabilities
                        .as_ref()
                        .is_none_or(|p| p.values().all(probability))
                    && confidence.as_ref().is_none_or(probability)
            }
            (
                Question::Score { criteria, .. },
                Answer::Score {
                    score,
                    probabilities,
                    confidence,
                },
            ) => {
                score.is_finite()
                    && *score >= 0.0
                    && *score <= (criteria.len() - 1) as f64
                    && probabilities.len() == criteria.len()
                    && probabilities.iter().all(probability)
                    && probability(confidence)
            }
            _ => false,
        };
        if !valid {
            return Err(());
        }
    }
    Ok(())
}

#[async_trait]
impl DecisionEngine for SystemOneHttpEngine {
    fn id(&self) -> &str {
        match self.kind {
            EndpointKind::TypeSafe => "typesafe_jev",
            EndpointKind::Cloudflare { .. } => "cloudflare_workers_ai",
            EndpointKind::Ollama { .. } => "local_ollama",
            EndpointKind::Vllm { .. } => "local_vllm",
        }
    }
    fn egress(&self) -> Egress {
        if self.kind.credential_key().is_some() {
            Egress::Cloud
        } else {
            Egress::Local
        }
    }
    fn limits(&self) -> EngineLimits {
        self.kind.limits()
    }
    async fn system_one(&self, req: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        let mut response = self.evaluate_wire(req).await?.into_typed();
        response.engine_id = self.id().to_owned();
        if response.model.is_empty() {
            response.model = self.kind.model().to_owned();
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests;
