package dev.opticcode.edits.negative;

import org.bukkit.event.entity.CreatureSpawnEvent.SpawnReason;

final class OtherOwners {
    enum Names {
        GUNPOWDER,
        NETHER_WART,
        CRAFTING_TABLE,
        FIREWORK_ROCKET
    }

    enum MobType {
        MOOSHROOM
    }

    Object spawnReason = SpawnReason.SPAWNER;
    Object powder = Names.GUNPOWDER;
    Object wart = Names.NETHER_WART;
    Object table = Names.CRAFTING_TABLE;
    Object rocket = Names.FIREWORK_ROCKET;
    Object mob = MobType.MOOSHROOM;
}
