use serde::Serialize;

use crate::java_index::{JavaIndexFile, JavaIndexedReference, JavaResolutionStatus};
use crate::java_syntax::JavaReferenceKind;

pub const LEGACY_RULE_CATALOG_SCHEMA_VERSION: u32 = 2;
pub const LEGACY_RULE_SET: &str = "minecraft_java_1_8_v2";

const TARGET_VERSIONS: &[&str] = &["1.8.8", "1.8.9"];
const LEGACY_SOURCE_ID: &str = "spigot-api-1.8.8-sources";
const MODERN_SOURCE_ID: &str = "spigot-api-1.21.4-sources";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyRuleEvidenceLevel {
    VerifiedApiPair,
    VerifiedLegacyTarget,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LegacyRuleSource {
    pub id: &'static str,
    pub coordinate: &'static str,
    pub artifact: &'static str,
    pub sha256: &'static str,
    pub url: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LegacyJavaRule {
    pub id: &'static str,
    pub owner: &'static str,
    pub modern: &'static str,
    pub legacy: &'static str,
    pub target_versions: &'static [&'static str],
    pub evidence_level: LegacyRuleEvidenceLevel,
    pub modern_source_id: Option<&'static str>,
    pub legacy_source_id: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LegacyRuleCatalog {
    pub schema_version: u32,
    pub rule_set: &'static str,
    pub target_versions: &'static [&'static str],
    pub sources: &'static [LegacyRuleSource],
    pub rules: &'static [LegacyJavaRule],
}

impl LegacyRuleCatalog {
    pub fn to_display_string(&self) -> String {
        let verified_pairs = self
            .rules
            .iter()
            .filter(|rule| rule.evidence_level == LegacyRuleEvidenceLevel::VerifiedApiPair)
            .count();
        let mut output = format!(
            concat!(
                "Minecraft Java legacy rule catalog:\n",
                "- schema: {}\n",
                "- rule set: {}\n",
                "- target versions: {}\n",
                "- rules: {} ({} verified API pairs)\n",
                "- pinned sources: {}\n"
            ),
            self.schema_version,
            self.rule_set,
            self.target_versions.join(", "),
            self.rules.len(),
            verified_pairs,
            self.sources.len(),
        );
        for rule in self.rules {
            output.push_str(&format!(
                "- {}: {} -> {} [{:?}]\n",
                rule.id, rule.modern, rule.legacy, rule.evidence_level
            ));
        }
        output
    }
}

impl LegacyJavaRule {
    pub(crate) fn modern_member(self) -> &'static str {
        self.modern
            .split_once('.')
            .map(|(_, member)| member)
            .expect("legacy rule modern name must contain an owner")
    }

    pub(crate) fn legacy_member(self) -> &'static str {
        self.legacy
            .split_once('.')
            .map(|(_, member)| member)
            .expect("legacy rule replacement must contain an owner")
    }

    pub(crate) fn owner_simple_name(self) -> &'static str {
        self.owner.rsplit('.').next().unwrap_or(self.owner)
    }

    pub(crate) fn target_id(self) -> String {
        format!("{}#{}", self.owner, self.modern_member())
    }
}

const fn verified_rule(
    id: &'static str,
    owner: &'static str,
    modern: &'static str,
    legacy: &'static str,
    reason: &'static str,
) -> LegacyJavaRule {
    LegacyJavaRule {
        id,
        owner,
        modern,
        legacy,
        target_versions: TARGET_VERSIONS,
        evidence_level: LegacyRuleEvidenceLevel::VerifiedApiPair,
        modern_source_id: Some(MODERN_SOURCE_ID),
        legacy_source_id: LEGACY_SOURCE_ID,
        reason,
    }
}

const fn legacy_target_rule(
    id: &'static str,
    owner: &'static str,
    modern: &'static str,
    legacy: &'static str,
    reason: &'static str,
) -> LegacyJavaRule {
    LegacyJavaRule {
        id,
        owner,
        modern,
        legacy,
        target_versions: TARGET_VERSIONS,
        evidence_level: LegacyRuleEvidenceLevel::VerifiedLegacyTarget,
        modern_source_id: None,
        legacy_source_id: LEGACY_SOURCE_ID,
        reason,
    }
}

pub const LEGACY_RULE_SOURCES: &[LegacyRuleSource] = &[
    LegacyRuleSource {
        id: LEGACY_SOURCE_ID,
        coordinate: "org.spigotmc:spigot-api:1.8.8-R0.1-SNAPSHOT:sources",
        artifact: "spigot-api-1.8.8-R0.1-20160221.082514-43-sources.jar",
        sha256: "f280f22be399e3d08521dfccba7bad2522f7cb2f9e32a27200425d40c37da308",
        url: "https://hub.spigotmc.org/nexus/repository/snapshots/org/spigotmc/spigot-api/1.8.8-R0.1-SNAPSHOT/spigot-api-1.8.8-R0.1-20160221.082514-43-sources.jar",
    },
    LegacyRuleSource {
        id: MODERN_SOURCE_ID,
        coordinate: "org.spigotmc:spigot-api:1.21.4-R0.1-SNAPSHOT:sources",
        artifact: "spigot-api-1.21.4-R0.1-20250325.160956-56-sources.jar",
        sha256: "6f8d397dd321817d02d7557e76221c89db27762085480f34d884188491267d0c",
        url: "https://hub.spigotmc.org/nexus/repository/snapshots/org/spigotmc/spigot-api/1.21.4-R0.1-SNAPSHOT/spigot-api-1.21.4-R0.1-20250325.160956-56-sources.jar",
    },
];

pub const LEGACY_JAVA_RULES: &[LegacyJavaRule] = &[
    verified_rule(
        "MC18-MATERIAL-001",
        "org.bukkit.Material",
        "Material.GUNPOWDER",
        "Material.SULPHUR",
        "Bukkit 1.8.8 uses SULPHUR for gunpowder.",
    ),
    verified_rule(
        "MC18-MATERIAL-002",
        "org.bukkit.Material",
        "Material.NETHER_WART",
        "Material.NETHER_STALK",
        "Bukkit 1.8.8 uses NETHER_STALK for the nether wart item.",
    ),
    verified_rule(
        "MC18-MATERIAL-003",
        "org.bukkit.Material",
        "Material.SPAWNER",
        "Material.MOB_SPAWNER",
        "Bukkit 1.8.8 uses MOB_SPAWNER for spawner blocks.",
    ),
    legacy_target_rule(
        "MC18-MATERIAL-004",
        "org.bukkit.Material",
        "Material.MONSTER_SPAWNER",
        "Material.MOB_SPAWNER",
        "Bukkit 1.8.8 uses MOB_SPAWNER for spawner blocks.",
    ),
    legacy_target_rule(
        "MC18-MATERIAL-005",
        "org.bukkit.Material",
        "Material.SPAWN_EGG",
        "Material.MONSTER_EGG",
        "Bukkit 1.8.8 uses MONSTER_EGG for the generic spawn egg item.",
    ),
    verified_rule(
        "MC18-MATERIAL-006",
        "org.bukkit.Material",
        "Material.WOODEN_SHOVEL",
        "Material.WOOD_SPADE",
        "Bukkit 1.8.8 uses SPADE names for shovels.",
    ),
    verified_rule(
        "MC18-MATERIAL-007",
        "org.bukkit.Material",
        "Material.STONE_SHOVEL",
        "Material.STONE_SPADE",
        "Bukkit 1.8.8 uses SPADE names for shovels.",
    ),
    verified_rule(
        "MC18-MATERIAL-008",
        "org.bukkit.Material",
        "Material.IRON_SHOVEL",
        "Material.IRON_SPADE",
        "Bukkit 1.8.8 uses SPADE names for shovels.",
    ),
    verified_rule(
        "MC18-MATERIAL-009",
        "org.bukkit.Material",
        "Material.DIAMOND_SHOVEL",
        "Material.DIAMOND_SPADE",
        "Bukkit 1.8.8 uses SPADE names for shovels.",
    ),
    verified_rule(
        "MC18-MATERIAL-010",
        "org.bukkit.Material",
        "Material.GOLDEN_SHOVEL",
        "Material.GOLD_SPADE",
        "Bukkit 1.8.8 uses GOLD_SPADE for the golden shovel.",
    ),
    legacy_target_rule(
        "MC18-MATERIAL-011",
        "org.bukkit.Material",
        "Material.GOLD_SHOVEL",
        "Material.GOLD_SPADE",
        "Bukkit 1.8.8 uses GOLD_SPADE for the historical GOLD_SHOVEL name.",
    ),
    verified_rule(
        "MC18-MATERIAL-012",
        "org.bukkit.Material",
        "Material.CRAFTING_TABLE",
        "Material.WORKBENCH",
        "Bukkit 1.8.8 uses WORKBENCH for crafting tables.",
    ),
    verified_rule(
        "MC18-MATERIAL-013",
        "org.bukkit.Material",
        "Material.COBWEB",
        "Material.WEB",
        "Bukkit 1.8.8 uses WEB for cobweb blocks.",
    ),
    verified_rule(
        "MC18-MATERIAL-014",
        "org.bukkit.Material",
        "Material.CLOCK",
        "Material.WATCH",
        "Bukkit 1.8.8 uses WATCH for the clock item.",
    ),
    verified_rule(
        "MC18-MATERIAL-015",
        "org.bukkit.Material",
        "Material.FIREWORK_ROCKET",
        "Material.FIREWORK",
        "Bukkit 1.8.8 uses FIREWORK for firework rocket items.",
    ),
    verified_rule(
        "MC18-MATERIAL-016",
        "org.bukkit.Material",
        "Material.FIREWORK_STAR",
        "Material.FIREWORK_CHARGE",
        "Bukkit 1.8.8 uses FIREWORK_CHARGE for firework star items.",
    ),
    verified_rule(
        "MC18-MATERIAL-017",
        "org.bukkit.Material",
        "Material.NETHER_PORTAL",
        "Material.PORTAL",
        "Bukkit 1.8.8 uses PORTAL for nether portal blocks.",
    ),
    verified_rule(
        "MC18-MATERIAL-018",
        "org.bukkit.Material",
        "Material.END_PORTAL",
        "Material.ENDER_PORTAL",
        "Bukkit 1.8.8 uses ENDER_PORTAL for end portal blocks.",
    ),
    verified_rule(
        "MC18-MATERIAL-019",
        "org.bukkit.Material",
        "Material.END_PORTAL_FRAME",
        "Material.ENDER_PORTAL_FRAME",
        "Bukkit 1.8.8 uses ENDER_PORTAL_FRAME for end portal frames.",
    ),
    verified_rule(
        "MC18-ENTITY-001",
        "org.bukkit.entity.EntityType",
        "EntityType.ZOMBIFIED_PIGLIN",
        "EntityType.PIG_ZOMBIE",
        "Bukkit 1.8.8 predates zombified piglin naming.",
    ),
    verified_rule(
        "MC18-ENTITY-002",
        "org.bukkit.entity.EntityType",
        "EntityType.MOOSHROOM",
        "EntityType.MUSHROOM_COW",
        "Bukkit 1.8.8 uses MUSHROOM_COW for mooshrooms.",
    ),
    verified_rule(
        "MC18-ENTITY-003",
        "org.bukkit.entity.EntityType",
        "EntityType.SNOW_GOLEM",
        "EntityType.SNOWMAN",
        "Bukkit 1.8.8 uses SNOWMAN for snow golems.",
    ),
    verified_rule(
        "MC18-ENTITY-004",
        "org.bukkit.entity.EntityType",
        "EntityType.TNT",
        "EntityType.PRIMED_TNT",
        "Bukkit 1.8.8 uses PRIMED_TNT for primed TNT entities.",
    ),
    verified_rule(
        "MC18-ENTITY-005",
        "org.bukkit.entity.EntityType",
        "EntityType.FIREWORK_ROCKET",
        "EntityType.FIREWORK",
        "Bukkit 1.8.8 uses FIREWORK for launched firework entities.",
    ),
    verified_rule(
        "MC18-ENTITY-006",
        "org.bukkit.entity.EntityType",
        "EntityType.FISHING_BOBBER",
        "EntityType.FISHING_HOOK",
        "Bukkit 1.8.8 uses FISHING_HOOK for fishing bobber entities.",
    ),
    verified_rule(
        "MC18-ENTITY-007",
        "org.bukkit.entity.EntityType",
        "EntityType.LIGHTNING_BOLT",
        "EntityType.LIGHTNING",
        "Bukkit 1.8.8 uses LIGHTNING for lightning bolt entities.",
    ),
];

pub fn legacy_rule_catalog() -> LegacyRuleCatalog {
    LegacyRuleCatalog {
        schema_version: LEGACY_RULE_CATALOG_SCHEMA_VERSION,
        rule_set: LEGACY_RULE_SET,
        target_versions: TARGET_VERSIONS,
        sources: LEGACY_RULE_SOURCES,
        rules: LEGACY_JAVA_RULES,
    }
}

pub(crate) fn rule_for_reference(reference: &JavaIndexedReference) -> Option<LegacyJavaRule> {
    if reference.kind != JavaReferenceKind::FieldAccess {
        return None;
    }

    let matching_member = LEGACY_JAVA_RULES
        .iter()
        .copied()
        .filter(|rule| reference.name == rule.modern_member())
        .collect::<Vec<_>>();
    if matching_member.len() == 1 {
        return matching_member.first().copied();
    }

    let target_id = reference.resolution.target_id.as_deref()?;
    matching_member
        .into_iter()
        .find(|rule| rule.target_id() == target_id)
}

pub(crate) fn qualifier_is_proven(
    rule: LegacyJavaRule,
    reference: &JavaIndexedReference,
    file: &JavaIndexFile,
) -> bool {
    let Some(qualifier) = reference.qualifier.as_deref() else {
        return false;
    };
    if qualifier == rule.owner {
        return true;
    }
    qualifier == rule.owner_simple_name()
        && file
            .imports
            .iter()
            .any(|import| !import.is_static && !import.wildcard && import.path == rule.owner)
}

pub(crate) fn is_exact_rule_target(rule: LegacyJavaRule, reference: &JavaIndexedReference) -> bool {
    reference.resolution.status == JavaResolutionStatus::Exact
        && reference.resolution.target_id.as_deref() == Some(rule.target_id().as_str())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{LegacyRuleEvidenceLevel, LEGACY_JAVA_RULES, LEGACY_RULE_SET, LEGACY_RULE_SOURCES};

    #[test]
    fn legacy_rule_ids_targets_and_evidence_are_unique_and_well_formed() {
        let mut ids = BTreeSet::new();
        let mut targets = BTreeSet::new();
        let source_ids = LEGACY_RULE_SOURCES
            .iter()
            .map(|source| source.id)
            .collect::<BTreeSet<_>>();

        for rule in LEGACY_JAVA_RULES {
            assert!(ids.insert(rule.id));
            assert!(targets.insert(rule.target_id()));
            assert_ne!(rule.modern_member(), rule.legacy_member());
            assert_eq!(rule.target_versions, ["1.8.8", "1.8.9"]);
            assert!(source_ids.contains(rule.legacy_source_id));
            if rule.evidence_level == LegacyRuleEvidenceLevel::VerifiedApiPair {
                assert!(rule
                    .modern_source_id
                    .is_some_and(|source| source_ids.contains(source)));
            }
            assert_eq!(
                rule.modern.split_once('.').map(|(owner, _)| owner),
                Some(rule.owner_simple_name())
            );
            assert_eq!(
                rule.legacy.split_once('.').map(|(owner, _)| owner),
                Some(rule.owner_simple_name())
            );
        }

        assert_eq!(LEGACY_RULE_SET, "minecraft_java_1_8_v2");
        assert_eq!(LEGACY_JAVA_RULES.len(), 26);
        assert_eq!(
            LEGACY_JAVA_RULES
                .iter()
                .filter(|rule| { rule.evidence_level == LegacyRuleEvidenceLevel::VerifiedApiPair })
                .count(),
            23
        );
    }

    #[test]
    fn duplicate_modern_members_are_explicitly_owned() {
        let firework_rules = LEGACY_JAVA_RULES
            .iter()
            .filter(|rule| rule.modern_member() == "FIREWORK_ROCKET")
            .collect::<Vec<_>>();
        assert_eq!(firework_rules.len(), 2);
        assert_ne!(firework_rules[0].owner, firework_rules[1].owner);
    }
}
