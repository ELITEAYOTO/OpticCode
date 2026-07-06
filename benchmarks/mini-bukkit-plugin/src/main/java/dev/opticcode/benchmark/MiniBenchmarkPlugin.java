package dev.opticcode.benchmark;

import dev.opticcode.benchmark.command.CoinsCommand;
import dev.opticcode.benchmark.listener.JoinListener;
import org.bukkit.command.PluginCommand;
import org.bukkit.plugin.java.JavaPlugin;

public final class MiniBenchmarkPlugin extends JavaPlugin {

    @Override
    public void onEnable() {
        PluginCommand coinsCommand = getCommand("coins");
        if (coinsCommand != null) {
            coinsCommand.setExecutor(new CoinsCommand());
        }

        getServer().getPluginManager().registerEvents(new JoinListener(), this);
    }
}
