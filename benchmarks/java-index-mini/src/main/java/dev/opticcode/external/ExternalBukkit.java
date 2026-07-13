package dev.opticcode.external;

import org.bukkit.Material;

import static org.bukkit.Bukkit.broadcastMessage;

public final class ExternalBukkit {
    private Material material = Material.GUNPOWDER;

    public void announce(String message) {
        broadcastMessage(message);
    }
}
