# OpticCode - Index RAG JSONL

Derniere mise a jour : 2026-07-06

## Objectif

Cette page documente le premier index RAG local d'OpticCode.

Cette V1 n'utilise pas encore Tantivy, SQLite, Qdrant ni embeddings. Elle sert a valider les sources, le decoupage en chunks et la recherche locale simple avant d'ajouter des briques plus lourdes.

## Commandes ajoutees

Construire un index :

```powershell
cargo run -q -- rag-index --output data/index --path . --path "C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kspawners"
```

Chercher dans l'index :

```powershell
cargo run -q -- rag-search "nether wart" --index data/index --limit 5
```

Utiliser le RAG dans un plan :

```powershell
cargo run -q -- plan "Verifier nether wart et spawner dans un plugin Bukkit 1.8.8" --path benchmarks/mini-bukkit-plugin --brief --rag-limit 3
```

Comparer sans RAG :

```powershell
cargo run -q -- plan "Verifier nether wart et spawner dans un plugin Bukkit 1.8.8" --path benchmarks/mini-bukkit-plugin --brief --no-rag
```

## Fichiers generes

Les artefacts sont ecrits dans :

```text
data/index/
```

Fichiers :

```text
documents.jsonl
chunks.jsonl
```

Le dossier `data/` est ignore par Git. L'index peut donc etre reconstruit localement sans polluer le depot.

## Index test effectue

Sources indexees :

```text
.
C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kspawners
C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kgui
C:\Users\timot\Desktop\minecraft\SparrowMCALL\KjobsUltimate
C:\Users\timot\Desktop\RAG-1.8-Minecraft\1.8-JavaDoc\resource-pack-1.8\LegacyPack
C:\Users\timot\Desktop\minecraft\Volkaria\Pack-Volkaria
```

Resultat :

```text
Sources: 6
Documents: 2647
Chunks: 5048
Indexed bytes: 12155587
```

## Recherches testees

### `spawner`

La recherche remonte bien le plugin `Kspawners`, notamment :

```text
plugin:src/main/java/me/krunsh/kspawner/data/PlayerSpawnerData.java
plugin:src/main/java/me/krunsh/kspawner/database/DatabaseCache.java
plugin:src/main/java/me/krunsh/kspawner/managers/SpawnerManager.java
```

### `nether wart`

La recherche remonte maintenant les sources utiles :

```text
opticcode:docs/minecraft-legacy-rules.md
resource-pack:assets/minecraft/lang/en_US.lang
resource-pack:assets/minecraft/lang/en_CA.lang
```

Le scoring impose que tous les mots d'une requete multi-mots soient presents dans le chunk. Cela evite de remonter trop haut des resultats comme `nether_brick` quand la requete est `nether wart`.

## Limites actuelles

- Pas encore de stemming ni synonymes.
- Pas encore de recherche fuzzy.
- Pas encore de Tantivy.
- Pas encore de recherche semantique.
- Pas encore de selection automatique de chunks pour le prompt Qwen.

## Integration `ask` / `plan`

Statut : ajoute.

Options disponibles :

```text
--no-rag
--rag-index data/index
--rag-limit 4
```

Comportement :

- `ask` et `plan` cherchent automatiquement dans `data/index` ;
- si l'index n'existe pas, la commande continue sans contexte RAG ;
- les extraits RAG sont ajoutes dans une section separee du prompt ;
- la taille totale injectee est bornee.

## Mesure initiale avec/sans RAG

Commande test :

```powershell
cargo run -q -- plan "Verifier nether wart et spawner dans un plugin Bukkit 1.8.8" --path benchmarks/mini-bukkit-plugin --brief --max-tokens 80 --metrics-json
```

Mesure chaude observee :

| Mode | Prompt | Temps client | Prompt eval | Generation | Debit |
| --- | ---: | ---: | ---: | ---: | ---: |
| avec RAG, `--rag-limit 3` | 11 299 caracteres | 3.70 s | 0.04 s | 3.08 s | 25.95 tok/s |
| sans RAG, `--no-rag` | 10 969 caracteres | 4.05 s | 0.04 s | 3.10 s | 25.78 tok/s |

Interpretation :

- sur ce test, le RAG ajoute environ 330 caracteres ;
- le cout prompt reste negligeable face aux tokens generes ;
- la mesure est courte, donc il faudra repeter via le script benchmark.

## Benchmark compare

Statut : ajoute.

Commande :

```powershell
.\scripts\run-rag-comparison.ps1
```

La commande lance chaque prompt en deux modes :

- avec RAG ;
- sans RAG.

Elle ecrit :

```text
benchmarks/runs/rag-comparison-<timestamp>.md
benchmarks/runs/rag-comparison-<timestamp>.jsonl
```

Les reponses completes restent dans :

```text
benchmarks/runs/mini-bukkit-*.answer.md
```

Premier essai court :

| Prompt | Mode | Prompt chars | Client s | Eval tok/s |
| --- | --- | ---: | ---: | ---: |
| `Verifier nether wart et spawner...` | avec RAG | 11 299 | 2.87 s | 25.97 |
| `Verifier nether wart et spawner...` | sans RAG | 10 969 | 2.85 s | 26.03 |
| `Quels risques legacy verifier pour des pelles et spawners ?` | avec RAG | 10 987 | 4.31 s | 26.36 |
| `Quels risques legacy verifier pour des pelles et spawners ?` | sans RAG | 10 969 | 2.82 s | 26.46 |

Interpretation provisoire :

- le debit modele reste stable autour de 26 tokens/s ;
- le RAG est peu couteux quand il ramene peu ou quelques extraits ;
- les prompts vagues en francais peuvent mal matcher les sources anglaises/Java ;
- la prochaine optimisation doit porter sur la requete RAG et les synonymes legacy.

## Expansion de requete legacy

Statut : ajoute.

OpticCode enrichit maintenant les requetes RAG avec des synonymes deterministes :

| Terme utilisateur | Requetes ajoutees |
| --- | --- |
| `pelle`, `pelles` | `shovel`, `spade`, `WOOD_SPADE`, `DIAMOND_SPADE` |
| `spawner` | `spawner`, `mob_spawner`, `MOB_SPAWNER` |
| `nether wart` | `nether wart`, `nether_stalk`, `NETHER_STALK` |
| `spawn egg`, `oeuf` | `spawn_egg`, `monster_placer`, `MONSTER_EGG` |
| `gunpowder`, `poudre` | `gunpowder`, `SULPHUR`, `Material.SULPHUR` |

Verification rapide :

```powershell
cargo run -q -- rag-search "spade" --index data/index --limit 5
```

Resultats utiles observes :

```text
opticcode:crates/opticcode-tools/src/lib.rs
opticcode:docs/minecraft-legacy-rules.md
opticcode:skills/profiles/minecraft-java-1.8/profile.md
resource-pack:assets/minecraft/lang/en_AU.lang
```

Deuxieme essai court apres expansion :

| Prompt | Mode | Prompt chars | Client s | Prompt eval | Eval tok/s |
| --- | --- | ---: | ---: | ---: | ---: |
| `Verifier nether wart et spawner...` | avec RAG | 11 961 | 4.53 s | 1.86 s | 26.22 |
| `Verifier nether wart et spawner...` | sans RAG | 10 969 | 2.86 s | 0.07 s | 26.37 |
| `Quels risques legacy verifier pour des pelles et spawners ?` | avec RAG | 11 961 | 2.85 s | 0.07 s | 25.95 |
| `Quels risques legacy verifier pour des pelles et spawners ?` | sans RAG | 10 969 | 2.81 s | 0.07 s | 26.26 |

Interpretation provisoire :

- l'expansion augmente le rappel sur les termes francais ;
- le cout normal ajoute environ 1 000 caracteres dans ce test ;
- le premier `prompt_eval` a montre un pic isole, a confirmer sur plus de runs ;
- le debit de generation reste stable.

## Priorite des sources injectees

Statut : ajoute.

Le tri utilise pour `ask` et `plan` favorise maintenant les sources dans cet ordre :

1. `opticcode:docs/`
2. `opticcode:skills/`
3. `plugin:`
4. `resource-pack:assets/minecraft/lang/`
5. `resource-pack:`
6. `pandaspigot:` patches
7. `pandaspigot:`
8. `opticcode:crates/`

Important : `rag-search` reste volontairement brut pour diagnostiquer l'index. Le tri priorise seulement le contexte envoye au modele.

Verification :

```text
spade -> docs et profil avant code interne dans le prompt RAG
```

Mesure chaude rapide apres ce changement :

```text
prompt_chars: 11926
client_seconds: 1.84
prompt_eval_seconds: 0.04
eval_count: 30
eval_tokens_per_second: 25.10
```

## Debug RAG

Statut : ajoute.

Commande sans appel modele :

```powershell
cargo run -q -- rag-debug "Quels risques legacy verifier pour des pelles et spawners ?" --index data/index --limit 3
```

Option sur `ask` et `plan` :

```powershell
cargo run -q -- plan "Quels risques legacy verifier pour des pelles et spawners ?" --path benchmarks/mini-bukkit-plugin --brief --rag-debug
```

Le debug affiche :

- l'index utilise ;
- les requetes RAG elargies ;
- les chunks réellement injectes dans le prompt ;
- les requetes elargies qui ont produit chaque chunk.

Exemple observe :

```text
Expanded queries:
- DIAMOND_SPADE
- MOB_SPAWNER
- WOOD_SPADE
- mob_spawner
- shovel
- spade
- spawner

Injected hits:
- opticcode:docs/minecraft-legacy-rules.md
- opticcode:skills/profiles/minecraft-java-1.8/profile.md
- plugin:src/main/java/me/krunsh/kspawner/data/PlayerSpawnerData.java
```

Le debug affiche aussi :

```text
chunk: 2024feecdafcac5f:0
matched_queries: DIAMOND_SPADE, NETHER_STALK, WOOD_SPADE, nether wart, nether_stalk, spade
```

## Deduplication RAG

Statut : ajoute.

OpticCode detecte maintenant quelques concepts legacy dans les previews RAG :

- `gunpowder` / `SULPHUR` ;
- `spade` / `shovel` ;
- `spawner` / `MOB_SPAWNER` ;
- `nether_stalk` / `nether_wart` ;
- `spawn_egg` / `monster_placer`.

Pour les sources OpticCode, les doublons sont filtres afin de garder plutot :

```text
opticcode:docs/minecraft-legacy-rules.md
```

avant :

```text
opticcode:skills/...
opticcode:crates/...
autres docs de benchmark
```

Les sources `plugin:` et `resource-pack:` restent separees, car elles peuvent apporter un exemple concret ou un nom d'asset.

Verification :

```powershell
cargo run -q -- rag-debug "Verifier pelles spawners nether wart gunpowder" --index data/index --limit 6
```

Observation :

- le contexte commence par `docs/minecraft-legacy-rules.md` ;
- le code interne Rust ne remonte plus en haut ;
- des sources plugin/resource-pack restent disponibles.

## Filtre anti-bruit

Statut : ajoute.

OpticCode ignore maintenant les hits avec :

- score `<= 1` ;
- aucun concept legacy detecte dans la preview ;
- source differente de `opticcode:docs/` ou `opticcode:skills/`.

Exemple corrige :

- une config plugin contenant beaucoup de valeurs `DIAMOND_*` pouvait remonter via `DIAMOND_SPADE` ;
- elle est maintenant filtree si elle ne contient pas de concept legacy utile.

Debug apres filtre :

```text
Injected hits:
source: opticcode:docs/minecraft-legacy-rules.md
matched_queries: DIAMOND_SPADE, NETHER_STALK, WOOD_SPADE, nether wart, nether_stalk, spade

source: plugin:src/main/java/me/krunsh/kspawner/data/PlayerSpawnerData.java
matched_queries: spawner

source: resource-pack:assets/minecraft/lang/en_US.lang
matched_queries: nether wart, shovel
```

Limite restante :

- le filtre reste volontairement simple ;
- il faudra mesurer sur plus de prompts avant de le rendre plus agressif.

## Prochaine etape

Ameliorer encore la requete RAG :

- afficher un score detaille par requete elargie, pas seulement la liste des requetes ;
- mesurer de nouveau avec plus de prompts.
