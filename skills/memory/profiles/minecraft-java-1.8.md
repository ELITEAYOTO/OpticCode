# Memory: minecraft-java-1.8

## Regles apprises

- Bukkit/Spigot/PandaSpigot 1.8.8 cible Java 8.
- Ne pas ajouter `api-version` dans `plugin.yml`.
- Ne pas proposer d'API Paper moderne sans preuve explicite.
- Pour gunpowder, utiliser `Material.SULPHUR`.
- Pour nether wart, utiliser `Material.NETHER_STALK`.
- Pour les pelles, utiliser les noms `*_SPADE`.
- Pour les spawners, utiliser `Material.MOB_SPAWNER`.

## Verification attendue

- Toujours comparer les commandes de `plugin.yml` avec les appels `getCommand(...)`.
- Lancer Maven/Gradle quand un patch Java est propose.
- Signaler les incertitudes sur les noms `Material` au lieu d'inventer.

## Packs locaux utiles plus tard

- Pack legacy 1.8 : `C:\Users\timot\Desktop\RAG-1.8-Minecraft\1.8-JavaDoc\resource-pack-1.8`
- Pack Volkaria : `C:\Users\timot\Desktop\minecraft\Volkaria\Pack-Volkaria`
