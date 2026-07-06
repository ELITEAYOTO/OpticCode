# OpticCode - Qualite RAG legacy

Derniere mise a jour : 2026-07-06

## Objectif

Ce document mesure si le RAG ameliore vraiment les reponses legacy Minecraft/Bukkit 1.8.8.

La mesure ne cherche pas seulement la vitesse. Elle verifie que les reponses contiennent les noms legacy attendus :

- `Material.SULPHUR` ;
- `Material.MOB_SPAWNER` ;
- `Material.NETHER_STALK` ;
- `Material.WOOD_SPADE` ;
- `Material.DIAMOND_SPADE` ;
- `Material.MONSTER_EGG` / `monsterPlacer`.

## Commande

```powershell
.\scripts\run-rag-quality.ps1 -MaxTokens 120
```

Le script lance chaque cas :

- avec RAG ;
- sans RAG.

Il genere :

```text
benchmarks/runs/rag-quality-<timestamp>.md
benchmarks/runs/rag-quality-<timestamp>.jsonl
```

Les reponses completes restent aussi dans les fichiers `mini-bukkit-*.answer.md`.

## Cas testes

| Cas | Attendu |
| --- | --- |
| `gunpowder-material` | `Material.SULPHUR` |
| `spawner-block` | `Material.MOB_SPAWNER` |
| `nether-wart` | `Material.NETHER_STALK` ou `netherStalk` |
| `shovel-materials` | `WOOD_SPADE` et `DIAMOND_SPADE` |
| `spawn-egg` | `MONSTER_EGG`, `monster_placer` ou `monsterPlacer` |

## Premier run apres scoring pondere

Fichier :

```text
benchmarks/runs/rag-quality-20260706-200710.md
```

Resultat :

| Mode | Score moyen |
| --- | ---: |
| avec RAG | 80 % |
| sans RAG | 60 % |

Observation :

- le RAG a ameliore le cas des pelles legacy ;
- le cas `spawn-egg` a echoue avec et sans RAG ;
- le debug RAG montrait surtout `spawn_egg.json`, mais pas assez la regle Bukkit `Material.MONSTER_EGG`.

## Correction appliquee

Ajouts :

- regle `Material.SPAWN_EGG`, `Material.*_SPAWN_EGG` -> `Material.MONSTER_EGG` dans `docs/minecraft-legacy-rules.md` ;
- expansion RAG vers `monsterPlacer` et `Material.MONSTER_EGG` ;
- detection du concept `monsterPlacer` en plus de `monster_placer`.

Index reconstruit :

```text
Sources: 6
Documents: 2651
Chunks: 5063
Indexed bytes: 12206793
```

Debug verifie :

```text
matched_queries: MONSTER_EGG, Material.MONSTER_EGG, monsterPlacer, spawn_egg
query_scores: spawn_egg=3x4, MONSTER_EGG=2, Material.MONSTER_EGG=2x4, monsterPlacer=2x4
```

## Run apres correction

Fichier :

```text
benchmarks/runs/rag-quality-20260706-201036.md
```

Resultat :

| Mode | Score moyen |
| --- | ---: |
| avec RAG | 100 % |
| sans RAG | 80 % |

Cas discriminant :

```text
spawn-egg avec RAG : Verifier Material.MONSTER_EGG et MonsterEggMeta.
spawn-egg sans RAG : reponse generique sans MONSTER_EGG.
```

## Interpretation

- Le RAG pondere ameliore bien la qualite sur les mappings legacy peu connus.
- Les cas faciles (`SULPHUR`, `MOB_SPAWNER`, `NETHER_STALK`) peuvent passer sans RAG car ils sont deja dans le profil et/ou connus du modele.
- Les cas plus fins comme spawn egg profitent vraiment de la doc metier locale.
- Le debit modele reste stable autour de 26 tokens/s ; le gain est donc qualitatif, pas une acceleration brute de Qwen.

## Limites

- Le score automatique ne remplace pas une revue humaine.
- Le modele peut varier legerement d'un run a l'autre.
- Les regex verifient la presence de termes attendus, pas la validite complete d'un patch.
- Les prochains tests doivent inclure des prompts de correction de code avec build Maven.
