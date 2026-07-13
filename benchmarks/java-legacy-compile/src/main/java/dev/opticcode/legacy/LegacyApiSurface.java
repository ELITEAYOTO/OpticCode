package dev.opticcode.legacy;

import org.bukkit.Material;
import org.bukkit.entity.EntityType;

final class LegacyApiSurface {
    private final Material[] materials = {
        Material.SULPHUR,
        Material.NETHER_STALK,
        Material.MOB_SPAWNER,
        Material.MONSTER_EGG,
        Material.WOOD_SPADE,
        Material.STONE_SPADE,
        Material.IRON_SPADE,
        Material.DIAMOND_SPADE,
        Material.GOLD_SPADE,
        Material.WORKBENCH,
        Material.WEB,
        Material.WATCH,
        Material.FIREWORK,
        Material.FIREWORK_CHARGE,
        Material.PORTAL,
        Material.ENDER_PORTAL,
        Material.ENDER_PORTAL_FRAME
    };

    private final EntityType[] entities = {
        EntityType.PIG_ZOMBIE,
        EntityType.MUSHROOM_COW,
        EntityType.SNOWMAN,
        EntityType.PRIMED_TNT,
        EntityType.FIREWORK,
        EntityType.FISHING_HOOK,
        EntityType.LIGHTNING
    };

    int verifiedConstantCount() {
        return materials.length + entities.length;
    }
}
