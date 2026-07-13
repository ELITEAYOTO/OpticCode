package dev.opticcode.edits.negative;

import org.bukkit.event.entity.CreatureSpawnEvent.SpawnReason;

final class OtherOwners {
    enum Names {
        GUNPOWDER,
        NETHER_WART
    }

    enum MobType {
        MOOSHROOM
    }

    Object spawnReason = SpawnReason.SPAWNER;
    Object powder = Names.GUNPOWDER;
    Object wart = Names.NETHER_WART;
    Object mob = MobType.MOOSHROOM;
}
