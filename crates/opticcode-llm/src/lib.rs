use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::net::IpAddr;
use std::time::{Duration, Instant};

pub const DEFAULT_OLLAMA_HTTP_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_OLLAMA_HTTP_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    keep_alive: Option<String>,
    timeout: Duration,
    http: reqwest::Client,
}

#[derive(Debug, Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<GenerateOptions>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct GenerateOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OllamaModelInfo {
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaModelInfo>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateResponse {
    pub response: String,
    pub done: bool,
    pub total_duration: Option<u64>,
    pub load_duration: Option<u64>,
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
    pub ollama_load_duration: Option<Duration>,
    pub keep_alive: Option<String>,
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
            keep_alive: default_keep_alive(),
            timeout: DEFAULT_OLLAMA_HTTP_TIMEOUT,
            http: reqwest::Client::new(),
        }
    }

    pub fn try_new(base_url: impl AsRef<str>) -> Result<Self> {
        Ok(Self::new(validate_local_ollama_url(base_url.as_ref())?))
    }

    pub fn with_keep_alive(mut self, keep_alive: Option<String>) -> Self {
        self.keep_alive = keep_alive;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() || timeout > MAX_OLLAMA_HTTP_TIMEOUT {
            bail!(
                "Ollama HTTP timeout must be between 1 ns and {} seconds",
                MAX_OLLAMA_HTTP_TIMEOUT.as_secs()
            );
        }
        self.timeout = timeout;
        Ok(self)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub async fn list_models(&self) -> Result<Vec<OllamaModelInfo>> {
        validate_local_ollama_url(&self.base_url)?;
        let response = self
            .http
            .get(format!("{}/api/tags", self.base_url))
            .timeout(self.timeout)
            .send()
            .await
            .context("failed to contact local Ollama API")?
            .error_for_status()
            .context("local Ollama tags API returned an error status")?
            .json::<OllamaTagsResponse>()
            .await
            .context("failed to parse local Ollama model inventory")?;
        Ok(response.models)
    }

    pub async fn model_available(&self, model: &str) -> Result<bool> {
        if model.trim().is_empty() {
            bail!("Ollama model name must not be empty");
        }
        let models = self.list_models().await?;
        Ok(models.iter().any(|candidate| {
            candidate.name.eq_ignore_ascii_case(model)
                || candidate.model.eq_ignore_ascii_case(model)
                || strip_latest(&candidate.name).eq_ignore_ascii_case(strip_latest(model))
                || strip_latest(&candidate.model).eq_ignore_ascii_case(strip_latest(model))
        }))
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
        validate_local_ollama_url(&self.base_url)?;
        let url = format!("{}/api/generate", self.base_url);
        let request = GenerateRequest {
            model,
            prompt,
            stream: false,
            keep_alive: self.keep_alive.as_deref(),
            options: options.has_values().then_some(options),
        };
        let started_at = Instant::now();

        let response = self
            .http
            .post(url)
            .json(&request)
            .timeout(self.timeout)
            .send()
            .await
            .context("failed to contact local Ollama API")?
            .error_for_status()
            .context("local Ollama API returned an error status")?
            .json::<GenerateResponse>()
            .await
            .context("failed to parse Ollama response")?;

        Ok(TimedGenerateResponse {
            metrics: GenerateMetrics {
                client_duration: started_at.elapsed(),
                prompt_chars: prompt.chars().count(),
                ollama_total_duration: response.total_duration.map(nanos_to_duration),
                ollama_load_duration: response.load_duration.map(nanos_to_duration),
                keep_alive: self.keep_alive.clone(),
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
        self.num_predict.is_some() || self.temperature.is_some() || self.seed.is_some()
    }
}

impl TimedGenerateResponse {
    fn into_response(self) -> GenerateResponse {
        GenerateResponse {
            response: self.response,
            done: true,
            total_duration: self.metrics.ollama_total_duration.map(duration_to_nanos),
            load_duration: self.metrics.ollama_load_duration.map(duration_to_nanos),
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

fn default_keep_alive() -> Option<String> {
    match env::var("OPTICCODE_OLLAMA_KEEP_ALIVE") {
        Ok(value) => parse_keep_alive(&value),
        Err(_) => Some("15m".to_string()),
    }
}

pub fn parse_keep_alive(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn validate_local_ollama_url(value: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(value).context("invalid Ollama URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("Ollama URL must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Ollama URL must not contain credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("Ollama URL must not contain a query or fragment");
    }
    if !matches!(parsed.path(), "" | "/") {
        bail!("Ollama URL must not contain an API path");
    }
    let host = parsed.host_str().context("Ollama URL has no host")?;
    let ip_host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    let local = host.eq_ignore_ascii_case("localhost")
        || ip_host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !local {
        bail!("refusing non-local Ollama URL; only localhost and loopback IPs are allowed");
    }
    let mut normalized = parsed;
    normalized.set_path("");
    Ok(normalized.as_str().trim_end_matches('/').to_string())
}

fn strip_latest(value: &str) -> &str {
    value.strip_suffix(":latest").unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Duration;

    use super::{parse_keep_alive, validate_local_ollama_url, GenerateOptions, OllamaClient};

    #[test]
    fn parses_keep_alive_values() {
        assert_eq!(parse_keep_alive("15m").as_deref(), Some("15m"));
        assert_eq!(parse_keep_alive("0").as_deref(), Some("0"));
        assert_eq!(parse_keep_alive(" none "), None);
        assert_eq!(parse_keep_alive(""), None);
    }

    #[test]
    fn accepts_only_local_ollama_urls() {
        assert_eq!(
            validate_local_ollama_url("http://localhost:11434/").unwrap(),
            "http://localhost:11434"
        );
        assert!(validate_local_ollama_url("http://127.0.0.1:11434").is_ok());
        assert!(validate_local_ollama_url("http://[::1]:11434").is_ok());
        assert!(validate_local_ollama_url("https://example.com").is_err());
        assert!(validate_local_ollama_url("http://localhost:11434/api/generate").is_err());
        assert!(validate_local_ollama_url("http://user:secret@localhost:11434").is_err());
    }

    #[test]
    fn rejects_unbounded_http_timeouts() {
        assert!(OllamaClient::new("http://localhost:11434")
            .with_timeout(Duration::ZERO)
            .is_err());
        assert!(OllamaClient::new("http://localhost:11434")
            .with_timeout(Duration::from_secs(3_601))
            .is_err());
    }

    #[tokio::test]
    async fn reports_an_absent_local_ollama_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = OllamaClient::try_new(format!("http://{address}"))
            .unwrap()
            .with_timeout(Duration::from_millis(200))
            .unwrap();

        let error = client.list_models().await.unwrap_err();

        assert!(format!("{error:#}").contains("failed to contact local Ollama API"));
    }

    #[tokio::test]
    async fn detects_a_missing_model_from_the_local_inventory() {
        let (url, request) = spawn_mock(
            r#"{"models":[{"name":"other-coder:7b","model":"other-coder:7b","size":42}]}"#,
            Duration::ZERO,
        );
        let client = OllamaClient::try_new(url).unwrap();

        assert!(!client.model_available("qwen2.5-coder:14b").await.unwrap());
        assert!(request.recv().unwrap().starts_with("GET /api/tags "));
    }

    #[tokio::test]
    async fn sends_reproducible_generation_options() {
        let (url, request) = spawn_mock(
            r#"{"response":"ok","done":true,"total_duration":10,"load_duration":2,"prompt_eval_count":12,"prompt_eval_duration":3,"eval_count":4,"eval_duration":5}"#,
            Duration::ZERO,
        );
        let client = OllamaClient::try_new(url).unwrap();

        let response = client
            .generate_timed_with_options(
                "qwen2.5-coder:14b",
                "local source",
                GenerateOptions {
                    num_predict: Some(64),
                    temperature: Some(0.0),
                    seed: Some(42),
                },
            )
            .await
            .unwrap();

        assert_eq!(response.response, "ok");
        let request = request.recv().unwrap();
        assert!(request.starts_with("POST /api/generate "));
        assert!(request.contains("\"num_predict\":64"));
        assert!(request.contains("\"temperature\":0.0"));
        assert!(request.contains("\"seed\":42"));
    }

    #[tokio::test]
    async fn enforces_the_explicit_http_timeout() {
        let (url, _request) = spawn_mock(
            r#"{"response":"late","done":true,"total_duration":10,"load_duration":2,"prompt_eval_count":1,"prompt_eval_duration":3,"eval_count":1,"eval_duration":5}"#,
            Duration::from_millis(200),
        );
        let client = OllamaClient::try_new(url)
            .unwrap()
            .with_timeout(Duration::from_millis(30))
            .unwrap();

        let error = client
            .generate_timed("qwen2.5-coder:14b", "timeout")
            .await
            .unwrap_err();

        assert!(format!("{error:#}").contains("failed to contact local Ollama API"));
    }

    fn spawn_mock(body: &'static str, delay: Duration) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let request = read_http_request(&mut stream);
            sender.send(request).unwrap();
            thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (format!("http://{address}"), receiver)
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 2048];
        loop {
            let read = stream.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length: ")
                        .or_else(|| line.strip_prefix("Content-Length: "))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= header_end + content_length {
                break;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}
