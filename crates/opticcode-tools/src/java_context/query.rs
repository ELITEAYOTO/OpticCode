use std::collections::BTreeSet;

use super::schema::JavaContextQuery;

pub(super) struct ParsedQuery {
    pub report: JavaContextQuery,
    pub task_lower: String,
    pub identifiers_lower: BTreeSet<String>,
    pub terms_lower: BTreeSet<String>,
    pub omitted_identifiers: usize,
    pub omitted_terms: usize,
}

impl ParsedQuery {
    pub fn requests_build_manifest(&self) -> bool {
        self.task_lower.contains("pom.xml")
            || self.task_lower.contains("build.gradle")
            || self.matches_any(&[
                "build",
                "dependance",
                "dependency",
                "gradle",
                "maven",
                "pom",
                "version",
            ])
    }

    pub fn requests_bukkit_descriptor(&self) -> bool {
        self.task_lower.contains("plugin.yml")
            || self.task_lower.contains("plugin.yaml")
            || self.matches_any(&[
                "bukkit",
                "command",
                "commande",
                "config",
                "configuration",
                "descriptor",
                "entrypoint",
                "main",
                "permission",
            ])
    }

    fn matches_any(&self, terms: &[&str]) -> bool {
        terms.iter().any(|term| self.terms_lower.contains(*term))
    }
}

pub(super) fn parse_query(task: &str, max_identifiers: usize, max_terms: usize) -> ParsedQuery {
    let raw_tokens = scan_tokens(task);
    let mut identifiers = Vec::new();
    let mut identifier_seen = BTreeSet::new();
    let mut terms = Vec::new();
    let mut term_seen = BTreeSet::new();
    let mut ignored_terms = 0usize;
    let mut omitted_identifiers = 0usize;
    let mut omitted_terms = 0usize;
    let mut truncated = false;

    for token in raw_tokens {
        let identifier_key = token.to_lowercase();
        if identifier_seen.insert(identifier_key) {
            if identifiers.len() < max_identifiers {
                identifiers.push(token.clone());
            } else {
                omitted_identifiers = omitted_identifiers.saturating_add(1);
                truncated = true;
            }
        }

        for part in identifier_terms(&token) {
            let normalized = part.to_lowercase();
            if normalized.len() < 2 || is_stop_word(&normalized) {
                ignored_terms = ignored_terms.saturating_add(1);
                continue;
            }
            if term_seen.insert(normalized.clone()) {
                if terms.len() < max_terms {
                    terms.push(normalized);
                } else {
                    omitted_terms = omitted_terms.saturating_add(1);
                    truncated = true;
                }
            }
        }
    }

    let identifiers_lower = identifiers
        .iter()
        .map(|identifier| identifier.to_lowercase())
        .collect();
    let terms_lower = terms.iter().cloned().collect();
    ParsedQuery {
        report: JavaContextQuery {
            raw: task.to_string(),
            normalized: task
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase(),
            raw_chars: task.chars().count(),
            identifiers,
            terms,
            ignored_terms,
            truncated,
        },
        task_lower: task.to_lowercase(),
        identifiers_lower,
        terms_lower,
        omitted_identifiers,
        omitted_terms,
    }
}

fn scan_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '$' | '.' | '#') {
            current.push(character);
        } else {
            push_token(&mut tokens, &mut current);
        }
    }
    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    let token = current
        .trim_matches(|character| matches!(character, '_' | '$' | '.' | '#'))
        .to_string();
    current.clear();
    if !token.is_empty() {
        tokens.push(token);
    }
}

fn identifier_terms(identifier: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for segment in identifier.split(['.', '#', '$', '_']) {
        if segment.is_empty() {
            continue;
        }
        terms.push(segment.to_string());
        terms.extend(split_camel_case(segment));
    }
    terms
}

pub(super) fn searchable_terms(value: &str) -> BTreeSet<String> {
    identifier_terms(value)
        .into_iter()
        .map(|part| part.to_lowercase())
        .filter(|part| part.len() >= 2 && !is_stop_word(part))
        .collect()
}

fn split_camel_case(value: &str) -> Vec<String> {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut start = 0usize;
    for index in 1..characters.len() {
        let previous = characters[index - 1];
        let current = characters[index];
        let next = characters.get(index + 1).copied();
        let boundary = (current.is_uppercase() && previous.is_lowercase())
            || (current.is_uppercase()
                && previous.is_uppercase()
                && next.is_some_and(char::is_lowercase))
            || (current.is_ascii_digit() && !previous.is_ascii_digit())
            || (!current.is_ascii_digit() && previous.is_ascii_digit());
        if boundary {
            parts.push(characters[start..index].iter().collect());
            start = index;
        }
    }
    parts.push(characters[start..].iter().collect());
    parts
}

fn is_stop_word(value: &str) -> bool {
    matches!(
        value,
        "a" | "au"
            | "aux"
            | "avec"
            | "ce"
            | "ces"
            | "cette"
            | "corriger"
            | "dans"
            | "de"
            | "des"
            | "du"
            | "en"
            | "et"
            | "faire"
            | "la"
            | "le"
            | "les"
            | "modifier"
            | "pour"
            | "que"
            | "qui"
            | "sur"
            | "un"
            | "une"
            | "verify"
            | "fix"
            | "the"
            | "and"
            | "for"
            | "with"
            | "from"
            | "into"
    )
}

#[cfg(test)]
mod tests {
    use super::parse_query;

    #[test]
    fn extracts_qualified_camel_and_legacy_identifiers() {
        let parsed = parse_query(
            "Corriger SpawnerManager#createSpawner et Material.NETHER_STALK dans Kspawners",
            32,
            64,
        );

        assert!(parsed
            .report
            .identifiers
            .contains(&"SpawnerManager#createSpawner".to_string()));
        assert!(parsed
            .report
            .identifiers
            .contains(&"Material.NETHER_STALK".to_string()));
        assert!(parsed.report.terms.contains(&"spawner".to_string()));
        assert!(parsed.report.terms.contains(&"manager".to_string()));
        assert!(parsed.report.terms.contains(&"create".to_string()));
        assert!(parsed.report.terms.contains(&"nether".to_string()));
        assert!(parsed.report.terms.contains(&"stalk".to_string()));
        assert!(!parsed.report.terms.contains(&"corriger".to_string()));
        assert!(!parsed.report.truncated);
    }

    #[test]
    fn reports_query_term_truncation_deterministically() {
        let parsed = parse_query("Alpha Beta Gamma Delta", 2, 2);
        assert_eq!(parsed.report.identifiers, ["Alpha", "Beta"]);
        assert_eq!(parsed.report.terms, ["alpha", "beta"]);
        assert_eq!(parsed.report.normalized, "alpha beta gamma delta");
        assert_eq!(parsed.omitted_identifiers, 2);
        assert_eq!(parsed.omitted_terms, 2);
        assert!(parsed.report.truncated);
    }
}
