use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<GenerateOptions>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct GenerateOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateResponse {
    pub response: String,
    pub done: bool,
    pub total_duration: Option<u64>,
    pub prompt_eval_count: Option<u64>,
    pub prompt_eval_duration: Option<u64>,
    pub eval_count: Option<u64>,
    pub eval_duration: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct GenerateMetrics {
    pub client_duration: Duration,
    pub prompt_chars: usize,
    pub ollama_total_duration: Option<Duration>,
    pub prompt_eval_count: Option<u64>,
    pub prompt_eval_duration: Option<Duration>,
    pub eval_count: Option<u64>,
    pub eval_duration: Option<Duration>,
}

#[derive(Debug)]
pub struct TimedGenerateResponse {
    pub response: String,
    pub metrics: GenerateMetrics,
}

impl OllamaClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn generate(&self, model: &str, prompt: &str) -> Result<GenerateResponse> {
        Ok(self.generate_timed(model, prompt).await?.into_response())
    }

    pub async fn generate_timed(&self, model: &str, prompt: &str) -> Result<TimedGenerateResponse> {
        self.generate_timed_with_options(model, prompt, GenerateOptions::default())
            .await
    }

    pub async fn generate_timed_with_options(
        &self,
        model: &str,
        prompt: &str,
        options: GenerateOptions,
    ) -> Result<TimedGenerateResponse> {
        let url = format!("{}/api/generate", self.base_url);
        let request = GenerateRequest {
            model,
            prompt,
            stream: false,
            options: options.has_values().then_some(options),
        };
        let started_at = Instant::now();

        let response = self
            .http
            .post(url)
            .json(&request)
            .send()
            .await
            .context("failed to contact Ollama API")?
            .error_for_status()
            .context("Ollama API returned an error status")?
            .json::<GenerateResponse>()
            .await
            .context("failed to parse Ollama response")?;

        Ok(TimedGenerateResponse {
            metrics: GenerateMetrics {
                client_duration: started_at.elapsed(),
                prompt_chars: prompt.chars().count(),
                ollama_total_duration: response.total_duration.map(nanos_to_duration),
                prompt_eval_count: response.prompt_eval_count,
                prompt_eval_duration: response.prompt_eval_duration.map(nanos_to_duration),
                eval_count: response.eval_count,
                eval_duration: response.eval_duration.map(nanos_to_duration),
            },
            response: response.response,
        })
    }
}

impl GenerateOptions {
    fn has_values(self) -> bool {
        self.num_predict.is_some()
    }
}

impl TimedGenerateResponse {
    fn into_response(self) -> GenerateResponse {
        GenerateResponse {
            response: self.response,
            done: true,
            total_duration: self.metrics.ollama_total_duration.map(duration_to_nanos),
            prompt_eval_count: self.metrics.prompt_eval_count,
            prompt_eval_duration: self.metrics.prompt_eval_duration.map(duration_to_nanos),
            eval_count: self.metrics.eval_count,
            eval_duration: self.metrics.eval_duration.map(duration_to_nanos),
        }
    }
}

fn nanos_to_duration(value: u64) -> Duration {
    Duration::from_nanos(value)
}

fn duration_to_nanos(value: Duration) -> u64 {
    value.as_nanos().min(u128::from(u64::MAX)) as u64
}
