package dev.opticcode.eval;

import org.bukkit.plugin.java.JavaPlugin;

public final class GradlePlugin extends JavaPlugin {
    @Override
    public void onEnable() {
        getCommand("evalreload").setExecutor(new ReloadCommand(this));
    }
}
