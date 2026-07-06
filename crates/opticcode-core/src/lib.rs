use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use opticcode_llm::GenerateOptions;
use opticcode_llm::OllamaClient;
pub use opticcode_llm::{parse_keep_alive, GenerateMetrics};
use opticcode_tools::build_project_context;

pub struct OpticCode {
    llm: OllamaClient,
    model: String,
}

pub struct AskOptions {
    pub workspace: PathBuf,
    pub prompt: String,
    pub profile: Option<String>,
    pub brief: bool,
    pub max_tokens: Option<u32>,
}

pub struct PlanOptions {
    pub workspace: PathBuf,
    pub goal: String,
    pub profile: Option<String>,
    pub brief: bool,
    pub max_tokens: Option<u32>,
}

pub struct AssistantOutput {
    pub text: String,
    pub metrics: GenerateMetrics,
}

#[derive(Debug, Clone)]
pub struct ProfileContext {
    pub id: String,
    pub source: PathBuf,
    pub content: String,
}

pub const DEFAULT_PROFILE: &str = "minecraft-java-1.8";

impl OpticCode {
    pub fn new(ollama_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            llm: OllamaClient::new(ollama_url),
            model: model.into(),
        }
    }

    pub fn with_keep_alive(mut self, keep_alive: Option<String>) -> Self {
        self.llm = self.llm.with_keep_alive(keep_alive);
        self
    }

    pub async fn ask_with_project_context(&self, options: AskOptions) -> Result<String> {
        Ok(self.ask_with_metrics(options).await?.text)
    }

    pub async fn ask_with_metrics(&self, options: AskOptions) -> Result<AssistantOutput> {
        let context = build_project_context(&options.workspace)?;
        let profile = load_profile_for_workspace(&options.workspace, options.profile.as_deref())?;
        let prompt = build_prompt(
            &options.prompt,
            &context.to_prompt_context(),
            profile.as_ref(),
            options.brief,
        );
        let response = self
            .llm
            .generate_timed_with_options(
                &self.model,
                &prompt,
                GenerateOptions {
                    num_predict: options.max_tokens.or_else(|| options.brief.then_some(240)),
                },
            )
            .await?;
        Ok(AssistantOutput {
            text: response.response,
            metrics: response.metrics,
        })
    }

    pub async fn plan_with_project_context(&self, options: PlanOptions) -> Result<String> {
        Ok(self.plan_with_metrics(options).await?.text)
    }

    pub async fn plan_with_metrics(&self, options: PlanOptions) -> Result<AssistantOutput> {
        let context = build_project_context(&options.workspace)?;
        let profile = load_profile_for_workspace(&options.workspace, options.profile.as_deref())?;
        let prompt = build_plan_prompt(
            &options.goal,
            &context.to_prompt_context(),
            profile.as_ref(),
            options.brief,
        );
        let response = self
            .llm
            .generate_timed_with_options(
                &self.model,
                &prompt,
                GenerateOptions {
                    num_predict: options.max_tokens.or_else(|| options.brief.then_some(320)),
                },
            )
            .await?;
        Ok(AssistantOutput {
            text: response.response,
            metrics: response.metrics,
        })
    }
}

pub fn load_profile_for_workspace(
    workspace: &Path,
    profile_id: Option<&str>,
) -> Result<Option<ProfileContext>> {
    let Some(profile_id) = profile_id else {
        return Ok(None);
    };

    let profile_id = profile_id.trim();
    if profile_id.is_empty() || profile_id.eq_ignore_ascii_case("none") {
        return Ok(None);
    }

    let relative = Path::new("skills")
        .join("profiles")
        .join(profile_id)
        .join("profile.md");
    let mut candidates = Vec::new();
    candidates.push(workspace.join(&relative));
    candidates.push(std::env::current_dir()?.join(&relative));

    for candidate in candidates {
        if candidate.exists() {
            let content = fs::read_to_string(&candidate)
                .with_context(|| format!("failed to read profile {}", candidate.display()))?;
            return Ok(Some(ProfileContext {
                id: profile_id.to_string(),
                source: candidate,
                content,
            }));
        }
    }

    anyhow::bail!("profile `{profile_id}` not found under skills/profiles/{profile_id}/profile.md")
}

impl ProfileContext {
    pub fn to_display_string(&self) -> String {
        format!(
            "Profile: {}\nSource: {}\n\n{}",
            self.id,
            self.source.display(),
            self.content
        )
    }
}

fn build_prompt(
    user_prompt: &str,
    project_context: &str,
    profile: Option<&ProfileContext>,
    brief: bool,
) -> String {
    let style = if brief {
        "\nMode court : reponds en 6 lignes maximum, sans introduction longue.\n"
    } else {
        ""
    };
    let profile_context = format_profile_context(profile);

    format!(
        r#"Tu es OpticCode, un assistant code local specialise Java Minecraft legacy.

Contraintes obligatoires :
- cible Java 8 stricte ;
- cible Bukkit/Spigot/PandaSpigot 1.8.8 / 1.8.9 ;
- pas de records, pas de var, pas d'API moderne inutile ;
- pour Bukkit 1.8.8, ne pas utiliser api-version dans plugin.yml ;
- pour gunpowder en 1.8.8, preferer Material.SULPHUR a Material.GUNPOWDER ;
- proposer un plan et signaler les risques avant toute modification ;
- si une information legacy est incertaine, le dire clairement.
{style}

Profil actif :
{profile_context}

Contexte projet detecte :
{project_context}

Demande utilisateur :
{user_prompt}
"#
    )
}

fn build_plan_prompt(
    goal: &str,
    project_context: &str,
    profile: Option<&ProfileContext>,
    brief: bool,
) -> String {
    let format_instruction = if brief {
        "Format attendu :\n- 6 puces maximum ;\n- chaque puce doit etre courte ;\n- inclure seulement les risques et verifications essentiels."
    } else {
        "Format attendu :\n1. Resume de l'objectif\n2. Fichiers a inspecter ou creer\n3. Plan d'implementation\n4. Points legacy Bukkit/Java 8 a surveiller\n5. Verifications a lancer\n6. Questions bloquantes, seulement si necessaire"
    };
    let profile_context = format_profile_context(profile);

    format!(
        r#"Tu es OpticCode en mode plan.

Tu dois produire uniquement un plan d'action. Tu ne dois pas ecrire un patch complet et tu ne dois pas pretendre avoir modifie des fichiers.

Contraintes obligatoires :
- cible Java 8 stricte ;
- cible Bukkit/Spigot/PandaSpigot 1.8.8 / 1.8.9 ;
- pas de records, pas de var, pas d'API moderne inutile ;
- pour Bukkit 1.8.8, ne pas utiliser api-version dans plugin.yml ;
- si la demande concerne gunpowder en 1.8.8, preferer Material.SULPHUR a Material.GUNPOWDER ;
- signaler les risques, les fichiers probables, les tests a lancer et les informations manquantes ;
- si une information legacy est incertaine, le dire clairement.

Interdictions en mode plan :
- ne pas inclure de bloc de code ;
- ne pas ecrire de classe complete ;
- ne pas ecrire de fichier plugin.yml complet ;
- ne pas inventer de package exact si le projet ne le montre pas ;
- ne pas citer une regle legacy qui ne concerne pas l'objectif.

{format_instruction}

Profil actif :
{profile_context}

Contexte projet detecte :
{project_context}

Objectif utilisateur :
{goal}
"#
    )
}

fn format_profile_context(profile: Option<&ProfileContext>) -> String {
    profile.map_or_else(
        || "none".to_string(),
        |profile| {
            format!(
                "id: {}\nsource: {}\n{}",
                profile.id,
                profile.source.display(),
                profile.content
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{build_plan_prompt, build_prompt, ProfileContext};
    use std::path::PathBuf;

    fn test_profile() -> ProfileContext {
        ProfileContext {
            id: "minecraft-java-1.8".to_string(),
            source: PathBuf::from("skills/profiles/minecraft-java-1.8/profile.md"),
            content: "Regle profil: Java 8 et Material.SULPHUR.".to_string(),
        }
    }

    #[test]
    fn prompt_contains_legacy_guardrails() {
        let profile = test_profile();
        let prompt = build_prompt("test", "context", Some(&profile), false);

        assert!(prompt.contains("Java 8"));
        assert!(prompt.contains("PandaSpigot"));
        assert!(prompt.contains("Material.SULPHUR"));
        assert!(prompt.contains("api-version"));
        assert!(prompt.contains("Regle profil"));
    }

    #[test]
    fn plan_prompt_forbids_file_modification_claims() {
        let profile = test_profile();
        let prompt = build_plan_prompt("ajouter /coins", "context", Some(&profile), false);

        assert!(prompt.contains("mode plan"));
        assert!(prompt.contains("Tu ne dois pas ecrire un patch complet"));
        assert!(prompt.contains("ne pas inclure de bloc de code"));
        assert!(prompt.contains("Verifications a lancer"));
        assert!(prompt.contains("Bukkit"));
    }

    #[test]
    fn brief_plan_prompt_limits_shape() {
        let prompt = build_plan_prompt("verifier plugin", "context", None, true);

        assert!(prompt.contains("6 puces maximum"));
        assert!(!prompt.contains("1. Resume de l'objectif"));
    }
}
