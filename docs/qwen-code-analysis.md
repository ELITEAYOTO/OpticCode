# OpticCode - Analyse Qwen Code

Derniere mise a jour : 2026-07-06

## Contexte

Qwen Code a ete clone comme depot de reference externe, hors du depot OpticCode :

```text
C:\Users\timot\Desktop\OpticCode-research\qwen-code
```

Commit analyse :

```text
be0b074
```

Le depot OpticCode reste propre. Qwen Code est utilise uniquement comme reference d'architecture.

## Pourquoi Qwen Code est utile

Qwen Code est un agent code terminal complet. Il n'est pas ecrit en Rust, mais son architecture montre des idees solides pour OpticCode :

- separation entre interface utilisateur et coeur agentique ;
- registre d'outils ;
- validation stricte des appels outil ;
- approbation avant les actions dangereuses ;
- lecture avant edition ;
- recherche de fichiers ;
- shell encadre ;
- memoire et skills ;
- configuration projet/utilisateur.

## Structure observee

### Racine

Elements notables :

- `packages/cli`
- `packages/core`
- `packages/sdk-typescript`
- `packages/sdk-python`
- `packages/sdk-java`
- `packages/vscode-ide-companion`
- `docs`
- `docs-site`
- `integration-tests`

Conclusion :

Qwen Code est devenu une plateforme assez large. OpticCode ne doit pas copier cette taille au debut.

### CLI

Dossier :

```text
packages/cli/src
```

Role :

- entree utilisateur ;
- commandes ;
- affichage terminal ;
- mode interactif/non interactif ;
- pont vers le core.

Equivalent OpticCode MVP :

```text
crates/opticcode-cli
```

Le CLI OpticCode doit rester beaucoup plus simple au depart :

- commande `opticcode ask` ;
- commande `opticcode inspect` ;
- commande `opticcode plan` ;
- commande `opticcode patch` plus tard.

### Core

Dossier :

```text
packages/core/src
```

Sous-systemes reperes :

- `tools`
- `config`
- `permissions`
- `memory`
- `skills`
- `providers`
- `prompts`
- `services`
- `lsp`
- `mcp`
- `subagents`

Equivalent OpticCode MVP :

```text
crates/opticcode-core
crates/opticcode-llm
crates/opticcode-tools
crates/opticcode-index
```

## Outils

Dossier :

```text
packages/core/src/tools
```

Outils importants pour OpticCode :

| Qwen Code | Equivalent OpticCode MVP | Priorite |
| --- | --- | --- |
| `read-file.ts` | lire un fichier avec limites de lignes | haute |
| `grep.ts` / `ripGrep.ts` | recherche texte | haute |
| `glob.ts` | trouver des fichiers | haute |
| `edit.ts` | remplacement textuel cible | moyenne |
| `write-file.ts` | ecriture de fichier | moyenne |
| `shell.ts` | commandes build/test | moyenne |
| `todoWrite.ts` | plan interne | basse pour MVP |
| `memory` / `skill` | specialisation metier | plus tard |
| MCP | extensions externes | plus tard |

## Patterns a reprendre

### 1. Tool = definition + invocation validee

Qwen Code separe :

- schema de l'outil ;
- validation des parametres ;
- invocation preparee ;
- execution.

Pourquoi c'est bon :

- le modele produit des arguments non fiables ;
- l'outil doit valider avant d'agir ;
- une action preparee peut decrire ce qu'elle va faire avant execution.

Version OpticCode Rust recommandee :

```text
ToolDefinition
ToolCall
ValidatedToolCall
ToolResult
```

### 2. Chemins absolus et racine projet

Les outils Qwen Code demandent des chemins absolus et valident les chemins.

Pour OpticCode :

- accepter cote utilisateur les chemins relatifs ;
- resoudre en interne vers une racine projet ;
- refuser les chemins hors workspace sauf confirmation explicite ;
- journaliser les fichiers lus/modifies.

### 3. Lecture avant edition

Qwen Code impose une idee cruciale : ne pas modifier un fichier que le modele n'a pas lu.

Pour OpticCode, c'est prioritaire.

Regle MVP :

- un patch ne peut toucher qu'un fichier lu dans la session ;
- si le fichier a change depuis la lecture, demander une relecture ;
- pour Java legacy, compiler apres patch quand possible.

### 4. Diff avant modification

Qwen Code construit un diff de confirmation pour les editions.

Pour OpticCode MVP :

- generer un patch unified diff ;
- ne pas appliquer automatiquement au debut ;
- afficher les fichiers touches ;
- plus tard, appliquer apres confirmation.

### 5. Outils paresseux

Qwen Code enregistre certains outils via des factories lazy.

Pour OpticCode :

- pas necessaire au tout debut ;
- utile plus tard si Tree-sitter, Qdrant, embeddings ou providers lourds deviennent optionnels.

### 6. Sorties bornees

Qwen Code limite/tronque les resultats trop grands.

Pour OpticCode :

- limite de lignes par lecture ;
- limite de resultats de recherche ;
- pagination `offset/limit` ;
- resume ou index pour fichiers enormes.

### 7. Recherche optimisee

Qwen Code prefere une recherche outil plutot que laisser le modele lancer n'importe quelle commande shell.

Pour OpticCode :

- utiliser `rg` quand disponible ;
- fallback Rust pur plus tard ;
- pour le RAG, combiner `rg`/Tantivy/Tree-sitter.

### 8. Shell encadre

Qwen Code traite le shell comme dangereux par defaut.

Pour OpticCode :

- autoriser seulement une whitelist MVP :
  - `mvn test`
  - `mvn package`
  - `gradle build`
  - `java -version`
  - `git status`
  - `git diff`
- demander confirmation pour toute commande destructive ;
- jamais executer une commande proposee par le modele sans validation.

## Patterns a eviter pour OpticCode MVP

### 1. Trop de modes

Qwen Code gere beaucoup de modes, daemon, channels, extensions, MCP, SDK, subagents.

OpticCode doit commencer plus petit :

- CLI local ;
- provider Ollama ;
- lecture/recherche ;
- plan ;
- patch propose.

### 2. Dependances Node comme base runtime

Qwen Code est TypeScript/Node >= 22. OpticCode vise Rust.

Decision :

- Node peut servir a etudier Qwen Code ;
- Node ne doit pas devenir une dependance runtime d'OpticCode.

### 3. Edition automatique trop tot

Qwen Code sait editer directement. OpticCode doit d'abord proposer des patches.

Raison :

- contexte Minecraft legacy fragile ;
- modele parfois faux sur Bukkit 1.8.8 ;
- besoin de compilation Java 8 et garde-fous.

### 4. MCP trop tot

MCP est puissant, mais ce n'est pas le coeur du MVP.

OpticCode doit d'abord bien gerer :

- projet local ;
- docs locales ;
- Java 8 ;
- Maven/Gradle ;
- RAG local.

## Implications pour l'architecture OpticCode

### Crates recommandes

```text
crates/
  opticcode-cli/
  opticcode-core/
  opticcode-llm/
  opticcode-tools/
  opticcode-index/
  opticcode-java/
  opticcode-memory/
```

### MVP minimal inspire de Qwen Code

1. `opticcode-cli`
   - parse les commandes ;
   - charge la config ;
   - affiche reponses et patches.

2. `opticcode-core`
   - orchestre la boucle agent ;
   - construit le prompt ;
   - appelle le provider LLM ;
   - route les outils.

3. `opticcode-llm`
   - provider Ollama HTTP ;
   - abstraction future LM Studio / llama.cpp.

4. `opticcode-tools`
   - read file ;
   - list files ;
   - search text ;
   - propose patch ;
   - run Maven/Gradle apres confirmation.

5. `opticcode-index`
   - plus tard : Tantivy ;
   - plus tard : Tree-sitter Java.

6. `opticcode-java`
   - regles Java 8 ;
   - detection Maven/Gradle ;
   - parsing erreurs ;
   - regles Bukkit 1.8.8.

## Regles OpticCode a deriver de cette analyse

- Lire avant de modifier.
- Produire un diff avant application.
- Garder un registre d'outils clair.
- Classer les outils : lecture, recherche, edition, execution.
- Ne pas laisser le modele lancer du shell librement.
- Resoudre les chemins par rapport au workspace.
- Limiter la taille des sorties.
- Garder le core independant du CLI.
- Garder le provider LLM interchangeable.

## Prochaine etape

Phase 4 peut commencer par un squelette Rust tres simple :

```text
opticcode-cli -> opticcode-core -> opticcode-llm(Ollama)
```

Le premier prototype ne doit pas encore editer les fichiers. Il doit :

- lire la config ;
- appeler Ollama ;
- lire un dossier ;
- rechercher dans les fichiers ;
- proposer un plan ;
- produire un patch texte non applique.
