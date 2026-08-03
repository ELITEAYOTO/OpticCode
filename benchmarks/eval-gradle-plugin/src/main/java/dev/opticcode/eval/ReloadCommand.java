package dev.opticcode.eval;

import org.bukkit.command.Command;
import org.bukkit.command.CommandExecutor;
import org.bukkit.command.CommandSender;

public final class ReloadCommand implements CommandExecutor {
    private final GradlePlugin plugin;

    public ReloadCommand(GradlePlugin plugin) {
        this.plugin = plugin;
    }

    @Override
    public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        plugin.reloadConfig();
        return true;
    }
}
