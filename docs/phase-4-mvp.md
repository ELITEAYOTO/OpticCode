# OpticCode - Phase 4 MVP Rust

Derniere mise a jour : 2026-07-06

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
- limiter la generation avec `--brief` et `--max-tokens`.

Il ne sait pas encore :

- modifier des fichiers ;
- produire un vrai patch unified diff ;
- compiler un projet Java ;
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
OK - 2 tests passes
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

1. Ajouter commande `build`.
2. Ajouter resume d'erreurs Maven/Gradle.
3. Ajouter generation de patch texte non applique.
