# OpticCode - Phase 4 MVP Rust

Derniere mise a jour : 2026-07-07

## Objectif

Demarrer un premier prototype Rust minimal, sans edition automatique, pour valider la chaine :

```text
opticcode-cli -> opticcode-core -> opticcode-llm -> Ollama
                       |
                       -> opticcode-tools
```

## Etat

Statut : squelette initial fonctionnel.

Le prototype sait deja :

- inspecter un dossier projet ;
- detecter Git, Maven et Gradle ;
- lister des fichiers echantillons ;
- compter les extensions ;
- chercher du texte dans les fichiers ;
- appeler Ollama via `POST /api/generate` ;
- injecter des garde-fous Java 8 / Bukkit 1.8.8 dans le prompt ;
- produire un plan d'action sans modification de fichiers ;
- construire un contexte projet enrichi avec extraits limites ;
- afficher des metriques LLM avec `--metrics` ;
- exporter des metriques JSON avec `--metrics-json` ;
- limiter la generation avec `--brief` et `--max-tokens` ;
- produire un patch preview deterministe sans modifier les fichiers ;
- verifier un patch preview avec `git apply --check` ;
- charger un profil `minecraft-java-1.8` depuis `skills/profiles` ;
- comparer les commandes `plugin.yml` avec les appels `getCommand(...)` ;
- charger une memoire Markdown simple depuis `skills/memory` ;
- scanner des resource packs externes en lecture seule ;
- inventorier des sources RAG externes en lecture seule ;
- construire et interroger un premier index RAG JSONL local ;
- injecter le RAG dans `ask` et `plan` avec `--no-rag` ;
- enrichir les requetes RAG avec des synonymes legacy.
- prioriser les docs et profils avant le code interne dans le contexte RAG.
- dedupliquer les regles RAG repetitives cote OpticCode.
- filtrer les hits RAG faibles sans concept legacy.
- afficher et utiliser un score RAG pondere par requete legacy.
- mesurer automatiquement la qualite legacy avec/sans RAG.
- verifier automatiquement un patch legacy jusqu'au rebuild Maven sur copie temporaire.
- cadrer `safe apply` avant toute application reelle.
- verifier un plan d'application avec `apply --dry-run` sans modifier de fichiers.
- appliquer un patch dans une copie temporaire avec `apply --copy-to ... --yes`.
- appliquer un patch reel dans le workspace courant avec `apply --yes`.
- journaliser une application reussie dans `.opticcode/apply-log.jsonl`.
- sauvegarder le patch de rollback dans `.opticcode/runs/<run-id>/patch.diff`.
- annuler une application avec `apply --undo <run-id> --yes`.

Il ne sait pas encore :

- appliquer un patch reel sur des projets externes ;
- indexer avec Tantivy ;
- parser Java avec Tree-sitter ;
- utiliser une memoire persistante.

## Workspace Rust

```text
Cargo.toml
crates/
  opticcode-cli/
  opticcode-core/
  opticcode-llm/
  opticcode-tools/
```

### `opticcode-cli`

Role :

- expose la commande `opticcode` ;
- gere les sous-commandes utilisateur.

Commandes actuelles :

```powershell
cargo run -q -- inspect --path .
cargo run -q -- context --path benchmarks/mini-bukkit-plugin
cargo run -q -- analyze-java --path benchmarks/mini-bukkit-plugin
cargo run -q -- build --path benchmarks/mini-bukkit-plugin
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin --check
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --dry-run
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --copy-to benchmarks/runs/apply-test --yes
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --yes
cargo run -q -- apply --path benchmarks/runs/apply-test --undo <run-id> --yes
cargo run -q -- profile --path benchmarks/mini-bukkit-plugin --profile minecraft-java-1.8
cargo run -q -- memory --path benchmarks/mini-bukkit-plugin --profile minecraft-java-1.8
cargo run -q -- pack-scan --path "C:\Users\timot\Desktop\RAG-1.8-Minecraft\1.8-JavaDoc\resource-pack-1.8\LegacyPack" --limit 25
cargo run -q -- pack-scan --path "C:\Users\timot\Desktop\minecraft\Volkaria\Pack-Volkaria" --limit 25
cargo run -q -- rag-scan --limit 8 --path "C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kspawners" --path "C:\Users\timot\Desktop\KhopeSpigot\PandaSpigot-Fork\PandaSpigot"
cargo run -q -- rag-index --output data/index --path . --path "C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kspawners"
cargo run -q -- rag-search "nether wart" --index data/index --limit 5
cargo run -q -- rag-debug "Quels risques legacy verifier pour des pelles et spawners ?" --index data/index --limit 3
cargo run -q -- plan "Verifier nether wart et spawner dans un plugin Bukkit 1.8.8" --path benchmarks/mini-bukkit-plugin --brief --max-tokens 80 --metrics-json --rag-limit 3
cargo run -q -- plan "Verifier nether wart et spawner dans un plugin Bukkit 1.8.8" --path benchmarks/mini-bukkit-plugin --brief --max-tokens 80 --metrics-json --no-rag
.\scripts\run-rag-comparison.ps1
.\scripts\run-rag-quality.ps1 -MaxTokens 120
.\scripts\run-patch-build-quality.ps1
cargo run -q -- search Material.SULPHUR --path . --limit 5
cargo run -q -- ask "Reponds en une phrase : quelle regle Bukkit 1.8.8 dois-tu respecter pour gunpowder ?" --path .
cargo run -q -- plan "Ajouter une commande /coins dans un plugin Bukkit 1.8.8" --path . --metrics
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin --brief --metrics
```

### `opticcode-core`

Role :

- orchestre le prompt ;
- ajoute les contraintes projet ;
- appelle le provider LLM ;
- prepare le contexte projet.

Garde-fous actuels :

- Java 8 strict ;
- Bukkit/Spigot/PandaSpigot 1.8.8 / 1.8.9 ;
- pas d'`api-version` dans `plugin.yml` legacy ;
- `Material.SULPHUR` au lieu de `Material.GUNPOWDER`.

### `opticcode-llm`

Role :

- client Ollama HTTP ;
- endpoint utilise : `/api/generate` ;
- mode `stream=false` pour un premier MVP simple.

### `opticcode-tools`

Role :

- inspection workspace ;
- recherche texte simple ;
- filtrage des dossiers inutiles : `.git`, `target`, `build`, `.gradle`, `.idea`, `.vscode`, `node_modules`, `models`, `data`.

## Verifications effectuees

### Compilation

```powershell
cargo check --workspace
```

Resultat :

```text
OK
```

### Tests

```powershell
cargo test --workspace
```

Resultat :

```text
OK - 38 tests passes
```

### Inspection locale

```powershell
cargo run -q -- inspect --path .
```

Resultat observe :

- Git detecte ;
- Maven non detecte dans le depot OpticCode ;
- Gradle non detecte dans le depot OpticCode ;
- fichiers Rust et docs detectes.

### Recherche locale

```powershell
cargo run -q -- search Material.SULPHUR --path . --limit 5
```

Resultat observe :

- retrouve la regle legacy dans la documentation ;
- retrouve le garde-fou dans `opticcode-core`.

### Appel Ollama

```powershell
cargo run -q -- ask "Reponds en une phrase : quelle regle Bukkit 1.8.8 dois-tu respecter pour gunpowder ?" --path .
```

Resultat observe :

```text
Pour Bukkit 1.8.8, il est preferable d'utiliser Material.SULPHUR au lieu de Material.GUNPOWDER.
```

### Plan sans modification

```powershell
cargo run -q -- plan "Ajouter une commande /coins dans un plugin Bukkit 1.8.8" --path .
```

Resultat attendu :

- resume l'objectif ;
- liste les fichiers probables ;
- propose les etapes ;
- rappelle Java 8 / Bukkit 1.8.8 ;
- ne produit pas de bloc de code complet ;
- ne modifie aucun fichier.

### Metriques LLM

Les commandes `ask` et `plan` acceptent maintenant :

```powershell
--metrics
--metrics-json
--brief
--max-tokens 320
```

Metriques affichees :

- temps client ;
- taille du prompt ;
- temps total Ollama ;
- tokens de prompt ;
- tokens generes ;
- debit de generation.

Le mode `--brief` reduit la longueur attendue de la reponse et passe une limite de generation a Ollama.

Le mode `--metrics-json` produit une sortie JSON exploitable pour comparer plusieurs runs.

## Decisions MVP

1. Le premier provider LLM est Ollama.
2. L'edition automatique reste interdite.
3. Les prochaines modifications devront passer par une proposition de patch.
4. Les regles legacy doivent etre injectees tres tot dans le prompt.
5. La recherche texte simple suffit pour le tout premier prototype.

## Prochaines etapes

1. Relier le profil et la memoire aux futures regles RAG.
2. Ajouter un benchmark JSONL/CSV reproductible.
3. Ajouter un cycle `build -> analyse erreur -> suggestion correction`.
