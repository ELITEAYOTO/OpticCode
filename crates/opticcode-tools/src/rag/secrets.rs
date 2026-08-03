use std::sync::OnceLock;

use regex::Regex;

use super::schema::RagRuleDescriptor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RagSecretDetection {
    pub rule_id: &'static str,
    pub category: &'static str,
    pub line: u32,
    pub column: u32,
}

pub(crate) fn detect_secret(content: &str) -> Option<RagSecretDetection> {
    let patterns = [
        ("secret.private_key", "private_key", private_key_regex()),
        ("secret.github_token", "access_token", github_regex()),
        ("secret.aws_access_key", "access_key", aws_regex()),
        ("secret.openai_token", "access_token", openai_regex()),
        (
            "secret.huggingface_token",
            "access_token",
            huggingface_regex(),
        ),
        ("secret.gitlab_token", "access_token", gitlab_regex()),
        (
            "secret.uri_credentials",
            "credential",
            uri_credentials_regex(),
        ),
    ];
    for (rule_id, category, regex) in patterns {
        if let Some(found) = regex.find(content) {
            let (line, column) = bounded_position(content, found.start());
            return Some(RagSecretDetection {
                rule_id,
                category,
                line,
                column,
            });
        }
    }
    detect_credential_assignment(content)
}

pub(crate) fn secret_rule_descriptors() -> Vec<RagRuleDescriptor> {
    [
        ("secret.private_key", "private_key"),
        ("secret.github_token", "access_token"),
        ("secret.aws_access_key", "access_key"),
        ("secret.openai_token", "access_token"),
        ("secret.huggingface_token", "access_token"),
        ("secret.gitlab_token", "access_token"),
        ("secret.uri_credentials", "credential"),
        ("secret.credential_assignment", "credential"),
    ]
    .into_iter()
    .map(|(rule_id, category)| RagRuleDescriptor {
        rule_id: rule_id.to_string(),
        category: category.to_string(),
        decision: "exclude".to_string(),
    })
    .collect()
}

fn detect_credential_assignment(content: &str) -> Option<RagSecretDetection> {
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("//")
            || trimmed.starts_with('*')
        {
            continue;
        }
        let Some((separator_index, separator)) = first_assignment_separator(trimmed) else {
            continue;
        };
        let key = normalize_assignment_key(&trimmed[..separator_index]);
        if !is_credential_key(&key) {
            continue;
        }
        let value = trimmed[separator_index + separator.len_utf8()..]
            .trim()
            .trim_end_matches(',')
            .trim()
            .trim_matches(['\'', '"']);
        if is_explicitly_non_secret_value(value) {
            continue;
        }
        let line = (line_index + 1).min(u32::MAX as usize) as u32;
        let column = (separator_index + separator.len_utf8() + 1).min(u32::MAX as usize) as u32;
        return Some(RagSecretDetection {
            rule_id: "secret.credential_assignment",
            category: "credential",
            line,
            column,
        });
    }
    None
}

fn first_assignment_separator(value: &str) -> Option<(usize, char)> {
    value
        .char_indices()
        .find(|(_, character)| matches!(character, '=' | ':'))
}

fn normalize_assignment_key(value: &str) -> String {
    let key = value
        .trim()
        .trim_matches(['\'', '"'])
        .to_ascii_lowercase()
        .replace(['-', '.'], "_");
    key.rsplit('/').next().unwrap_or(&key).to_string()
}

fn is_credential_key(key: &str) -> bool {
    matches!(
        key,
        "password"
            | "passwd"
            | "pwd"
            | "secret"
            | "client_secret"
            | "api_key"
            | "apikey"
            | "token"
            | "access_token"
            | "access_key"
            | "private_key"
            | "database_password"
            | "db_password"
    ) || key.ends_with("_password")
        || key.ends_with("_secret")
        || key.ends_with("_token")
        || key.ends_with("_api_key")
}

fn is_explicitly_non_secret_value(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.is_empty()
        || matches!(
            lower.as_str(),
            "null" | "none" | "false" | "true" | "redacted" | "masked"
        )
        || (value.starts_with("${") && value.ends_with('}'))
        || (value.starts_with("{{") && value.ends_with("}}"))
        || (value.starts_with('<') && value.ends_with('>'))
        || value.chars().all(|character| character == '*')
}

fn bounded_position(content: &str, byte_offset: usize) -> (u32, u32) {
    let prefix = &content[..byte_offset.min(content.len())];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map(|(_, tail)| tail.chars().count() + 1)
        .unwrap_or_else(|| prefix.chars().count() + 1);
    (
        line.min(u32::MAX as usize) as u32,
        column.min(u32::MAX as usize) as u32,
    )
}

fn private_key_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"(?i)-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----")
            .expect("private-key regex should compile")
    })
}

fn github_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"\bgh[pousr]_[A-Za-z0-9]{20,255}\b").expect("GitHub token regex should compile")
    })
}

fn aws_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"\bAKIA[0-9A-Z]{16}\b").expect("AWS access-key regex should compile")
    })
}

fn openai_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,255}\b")
            .expect("OpenAI token regex should compile")
    })
}

fn huggingface_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"\bhf_[A-Za-z0-9]{20,255}\b").expect("Hugging Face token regex should compile")
    })
}

fn gitlab_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"\bglpat-[A-Za-z0-9_-]{20,255}\b").expect("GitLab token regex should compile")
    })
}

fn uri_credentials_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"(?i)\b[a-z][a-z0-9+.-]*://[^/\s:@]+:[^/\s@]+@")
            .expect("URI credential regex should compile")
    })
}
