use crate::java_index::{JavaIndexFile, JavaIndexedReference, JavaResolutionStatus};
use crate::java_syntax::JavaReferenceKind;

#[derive(Debug, Clone, Copy)]
pub(crate) struct LegacyJavaRule {
    pub(crate) id: &'static str,
    pub(crate) owner: &'static str,
    pub(crate) modern: &'static str,
    pub(crate) legacy: &'static str,
    pub(crate) reason: &'static str,
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

pub(crate) const LEGACY_JAVA_RULES: &[LegacyJavaRule] = &[
    LegacyJavaRule {
        id: "MC18-MATERIAL-001",
        owner: "org.bukkit.Material",
        modern: "Material.GUNPOWDER",
        legacy: "Material.SULPHUR",
        reason: "Bukkit 1.8.8 uses SULPHUR for gunpowder.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-002",
        owner: "org.bukkit.Material",
        modern: "Material.NETHER_WART",
        legacy: "Material.NETHER_STALK",
        reason: "Bukkit 1.8.8 uses NETHER_STALK for nether wart.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-003",
        owner: "org.bukkit.Material",
        modern: "Material.SPAWNER",
        legacy: "Material.MOB_SPAWNER",
        reason: "Bukkit 1.8.8 uses MOB_SPAWNER for spawner blocks.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-004",
        owner: "org.bukkit.Material",
        modern: "Material.MONSTER_SPAWNER",
        legacy: "Material.MOB_SPAWNER",
        reason: "Bukkit 1.8.8 uses MOB_SPAWNER for spawner blocks.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-005",
        owner: "org.bukkit.Material",
        modern: "Material.SPAWN_EGG",
        legacy: "Material.MONSTER_EGG",
        reason: "Bukkit 1.8.8 uses MONSTER_EGG for spawn egg items.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-006",
        owner: "org.bukkit.Material",
        modern: "Material.WOODEN_SHOVEL",
        legacy: "Material.WOOD_SPADE",
        reason: "Bukkit 1.8.8 uses SPADE names for shovels.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-007",
        owner: "org.bukkit.Material",
        modern: "Material.STONE_SHOVEL",
        legacy: "Material.STONE_SPADE",
        reason: "Bukkit 1.8.8 uses SPADE names for shovels.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-008",
        owner: "org.bukkit.Material",
        modern: "Material.IRON_SHOVEL",
        legacy: "Material.IRON_SPADE",
        reason: "Bukkit 1.8.8 uses SPADE names for shovels.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-009",
        owner: "org.bukkit.Material",
        modern: "Material.DIAMOND_SHOVEL",
        legacy: "Material.DIAMOND_SPADE",
        reason: "Bukkit 1.8.8 uses SPADE names for shovels.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-010",
        owner: "org.bukkit.Material",
        modern: "Material.GOLDEN_SHOVEL",
        legacy: "Material.GOLD_SPADE",
        reason: "Bukkit 1.8.8 uses SPADE names for shovels.",
    },
    LegacyJavaRule {
        id: "MC18-MATERIAL-011",
        owner: "org.bukkit.Material",
        modern: "Material.GOLD_SHOVEL",
        legacy: "Material.GOLD_SPADE",
        reason: "Bukkit 1.8.8 uses SPADE names for shovels.",
    },
    LegacyJavaRule {
        id: "MC18-ENTITY-001",
        owner: "org.bukkit.entity.EntityType",
        modern: "EntityType.ZOMBIFIED_PIGLIN",
        legacy: "EntityType.PIG_ZOMBIE",
        reason: "Bukkit 1.8.8 predates zombified piglin naming.",
    },
    LegacyJavaRule {
        id: "MC18-ENTITY-002",
        owner: "org.bukkit.entity.EntityType",
        modern: "EntityType.MOOSHROOM",
        legacy: "EntityType.MUSHROOM_COW",
        reason: "Bukkit 1.8.8 uses MUSHROOM_COW for mooshrooms.",
    },
    LegacyJavaRule {
        id: "MC18-ENTITY-003",
        owner: "org.bukkit.entity.EntityType",
        modern: "EntityType.SNOW_GOLEM",
        legacy: "EntityType.SNOWMAN",
        reason: "Bukkit 1.8.8 uses SNOWMAN for snow golems.",
    },
];

pub(crate) fn rule_for_reference(reference: &JavaIndexedReference) -> Option<LegacyJavaRule> {
    if reference.kind != JavaReferenceKind::FieldAccess {
        return None;
    }
    LEGACY_JAVA_RULES
        .iter()
        .copied()
        .find(|rule| reference.name == rule.modern_member())
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

    use super::LEGACY_JAVA_RULES;

    #[test]
    fn legacy_rule_ids_and_targets_are_unique_and_well_formed() {
        let mut ids = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for rule in LEGACY_JAVA_RULES {
            assert!(ids.insert(rule.id));
            assert!(targets.insert(rule.target_id()));
            assert_ne!(rule.modern_member(), rule.legacy_member());
            assert_eq!(
                rule.modern.split_once('.').map(|(owner, _)| owner),
                Some(rule.owner_simple_name())
            );
            assert_eq!(
                rule.legacy.split_once('.').map(|(owner, _)| owner),
                Some(rule.owner_simple_name())
            );
        }
        assert_eq!(LEGACY_JAVA_RULES.len(), 14);
    }
}
