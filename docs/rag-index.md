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

## Prochaine etape

Ajouter un mode benchmark compare :

- avec/sans RAG ;
- plusieurs prompts legacy ;
- export JSONL deja exploitable ;
- comparaison qualite manuelle des reponses.
