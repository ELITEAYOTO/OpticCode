package dev.opticcode.benchmark.command;

import java.util.HashMap;
import java.util.Map;
import java.util.UUID;
import org.bukkit.ChatColor;
import org.bukkit.command.Command;
import org.bukkit.command.CommandExecutor;
import org.bukkit.command.CommandSender;
import org.bukkit.entity.Player;

public final class CoinsCommand implements CommandExecutor {

    private final Map<UUID, Integer> coinsByPlayer = new HashMap<UUID, Integer>();

    @Override
    public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        if (!(sender instanceof Player)) {
            sender.sendMessage(ChatColor.RED + "Only players can use this command.");
            return true;
        }

        Player player = (Player) sender;
        int coins = getCoins(player);

        if (args.length == 0) {
            player.sendMessage(ChatColor.GOLD + "Coins: " + coins);
            return true;
        }

        if ("add".equalsIgnoreCase(args[0])) {
            return handleAdd(player, args, coins);
        }

        player.sendMessage(ChatColor.RED + "Usage: /coins or /coins add <amount>");
        return true;
    }

    private boolean handleAdd(Player player, String[] args, int currentCoins) {
        if (args.length != 2) {
            player.sendMessage(ChatColor.RED + "Usage: /coins add <amount>");
            return true;
        }

        int amount;
        try {
            amount = Integer.parseInt(args[1]);
        } catch (NumberFormatException ignored) {
            player.sendMessage(ChatColor.RED + "Amount must be a number.");
            return true;
        }

        if (amount <= 0) {
            player.sendMessage(ChatColor.RED + "Amount must be positive.");
            return true;
        }

        int newTotal = currentCoins + amount;
        coinsByPlayer.put(player.getUniqueId(), Integer.valueOf(newTotal));
        player.sendMessage(ChatColor.GREEN + "Coins: " + newTotal);
        return true;
    }

    private int getCoins(Player player) {
        Integer coins = coinsByPlayer.get(player.getUniqueId());
        return coins == null ? 0 : coins.intValue();
    }
}
