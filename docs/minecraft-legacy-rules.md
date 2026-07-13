# OpticCode - Regles Minecraft Java 1.8 legacy

Derniere mise a jour : 2026-07-06

## Objectif

Ce document liste les premieres correspondances legacy que OpticCode doit connaitre pour Bukkit/Spigot/PandaSpigot 1.8.8.

Ces regles servent a trois endroits :

- profil `minecraft-java-1.8` ;
- analyse `analyze-java` ;
- patch preview `patch` ;
- propositions AST read-only `java-edits` ;
- scan preparatoire `pack-scan`.

## Sources locales consultees

Les packs originaux restent a leur place. OpticCode ne les deplace pas.

```text
C:\Users\timot\Desktop\RAG-1.8-Minecraft\1.8-JavaDoc\resource-pack-1.8
C:\Users\timot\Desktop\minecraft\Volkaria\Pack-Volkaria
```

Observations utiles :

- le pack legacy contient `mob_spawner`, `nether_wart`, `nether_stalk`, `spawn_egg` et les assets shovel ;
- le pack Volkaria contient des CIT autour des spawners, mobs shop, nether wart et shovels custom ;
- les fichiers `.lang` exposent des noms 1.8 comme `tile.mobSpawner`, `tile.netherStalk`, `item.netherStalk`, `item.monsterPlacer`.

La commande `pack-scan` confirme aussi la presence de nombreux `models/block`, `models/item`, textures, blockstates et fichiers CIT utiles pour le futur RAG.

## Materials legacy supportes par OpticCode

| Nom moderne repere | Nom Bukkit 1.8.8 recommande | Note |
| --- | --- | --- |
| `Material.GUNPOWDER` | `Material.SULPHUR` | Gunpowder |
| `Material.NETHER_WART` | `Material.NETHER_STALK` | Nether wart/stalk |
| `Material.SPAWNER` | `Material.MOB_SPAWNER` | Bloc spawner |
| `Material.MONSTER_SPAWNER` | `Material.MOB_SPAWNER` | Bloc spawner |
| `Material.SPAWN_EGG`, `Material.*_SPAWN_EGG` | `Material.MONSTER_EGG` | Spawn egg item ; verifier entity id / durability / NBT |
| `Material.WOODEN_SHOVEL` | `Material.WOOD_SPADE` | Pelle bois |
| `Material.STONE_SHOVEL` | `Material.STONE_SPADE` | Pelle pierre |
| `Material.IRON_SHOVEL` | `Material.IRON_SPADE` | Pelle fer |
| `Material.DIAMOND_SHOVEL` | `Material.DIAMOND_SPADE` | Pelle diamant |
| `Material.GOLDEN_SHOVEL` | `Material.GOLD_SPADE` | Pelle or |
| `Material.GOLD_SHOVEL` | `Material.GOLD_SPADE` | Variante moderne a corriger |

## EntityType legacy supportes par OpticCode

| Nom moderne repere | Nom Bukkit 1.8.8 recommande | Note |
| --- | --- | --- |
| `EntityType.ZOMBIFIED_PIGLIN` | `EntityType.PIG_ZOMBIE` | Pig zombie legacy |
| `EntityType.MOOSHROOM` | `EntityType.MUSHROOM_COW` | Mooshroom legacy |
| `EntityType.SNOW_GOLEM` | `EntityType.SNOWMAN` | Snow golem legacy |

## Regles importantes

- Ne pas convertir automatiquement les laines, vitres, tapis, bois ou variantes de blocs colorees sans verifier les data values.
- Pour les blocks/items avec metadata en 1.8, preferer signaler le risque plutot que produire une correction approximative.
- Les noms issus du resource pack ne correspondent pas toujours directement aux enums Bukkit `Material`.
- Un nom identique ne suffit jamais : `SpawnReason.SPAWNER` ne doit pas etre
  confondu avec `Material.SPAWNER` ; l'identite qualifiee doit etre prouvee.
- `item.monsterPlacer` est le nom resource-pack 1.8 des spawn eggs ; cote Bukkit 1.8, verifier `Material.MONSTER_EGG` et les donnees d'entite associees.
- Les spawners custom Volkaria devront etre indexes plus tard via RAG/CIT, pas hardcodes dans le moteur.

## Prochaines extensions possibles

- `Material.COMPARATOR` / redstone comparator selon usage item/block.
- `Material.WORKBENCH` pour crafting table moderne.
- `Material.WOOD` / `LOG` avec metadata pour les variantes bois.
- `Material.WOOL`, `STAINED_GLASS`, `STAINED_GLASS_PANE` avec data values.
- `EntityType` supplementaires selon les erreurs rencontrees dans les projets reels.
