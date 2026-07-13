# OpticCode - Architecture cible

Derniere mise a jour : 2026-07-13

## Principe

OpticCode n'est pas un modele IA. C'est un agent local qui orchestre un modele open-weight deja entraine, des outils de code, une memoire projet et une base documentaire specialisee.

## Architecture logique

```text
Utilisateur
  |
  v
optic-cli
  |
  v
optic-core
  |
  +-- planner
  +-- session
  +-- context-builder
  +-- tool-router
  +-- safety-guard
  +-- patch-manager
  +-- verifier
  |
  +-- optic-llm
  |     +-- ollama provider
  |     +-- lm studio/openai-compatible provider
  |     +-- llama.cpp provider
  |
  +-- optic-tools
  |     +-- read/list/search files
  |     +-- preview/check patch
  |     +-- apply transactionnel + rollback/recovery
  |     +-- disposable Git worktree verifier
  |     +-- git diff/status
  |     +-- bounded process runner
  |     +-- maven/gradle build
  |     +-- Java Tree-sitter read-only syntax analysis
  |     +-- Java cross-file symbol index and conservative resolver
  |     +-- read-only AST-ranged Java edit proposals
  |
  +-- optic-rag
  |     +-- sqlite metadata
  |     +-- tantivy full-text
  |     +-- tree-sitter symbols
  |     +-- qdrant vectors later
  |
  +-- optic-memory
        +-- project memory
        +-- user preferences
        +-- known errors
        +-- legacy rules
```

## Choix de langage

| Partie | Choix recommande | Raison |
| --- | --- | --- |
| Core agent | Rust | robustesse, performance, async, outils systeme |
| CLI | Rust | distribution simple, coherence avec le core |
| Tools fichiers/build | Rust | controle, securite, integration Windows |
| RAG metadata | SQLite | simple, local, fiable |
| Recherche texte | Tantivy | Rust natif, BM25, rapide |
| Parsing code | Tree-sitter | robuste, multi-langages, bon pour Java |
| Runtime modele | Ollama puis llama.cpp | demarrage rapide puis optimisation |
| C++ | limite au runtime bas niveau | ne pas complexifier le core inutilement |

## Modules Rust envisages

```text
crates/
  optic-cli/
  optic-core/
  optic-llm/
  optic-tools/
  optic-rag/
  optic-memory/
  optic-config/
  optic-safety/
```

Les crates actuelles gardent le prefixe `opticcode-*`. Le decoupage ci-dessus reste la cible logique ; il pourra etre renomme ou affine quand le MVP sera stabilise.

## Regles de conception

- Le modele ne modifie jamais directement les fichiers.
- Le core Rust encadre les actions.
- Les tools ont des schemas clairs.
- Les editions passent par patches.
- Toute ecriture prepare patch, manifeste et backups avant la premiere mutation.
- Les transactions ont des etats versionnes, un rollback BLAKE3 et une recovery explicite.
- Les commandes shell doivent etre limitees et explicites.
- Tout nouvel outil externe potentiellement long passe par le process runner.
- Timeout, annulation, capture bornee et cause d'arret restent des donnees structurees.
- La compilation doit etre lancee quand c'est raisonnable.
- Les erreurs de build deviennent des donnees reutilisables.
- Les contraintes Java 8 / Bukkit 1.8.8 doivent etre injectees dans le contexte.

## Processus externes

Etat implemente :

- `ProcessRequest` et `ProcessResult` communs dans `opticcode-tools` ;
- timeout de build a 600 secondes par defaut ;
- `stdout` et `stderr` draines en parallele avec tails bornes ;
- `CancellationToken` distinct du timeout et relie a `Ctrl+C` pour `build` ;
- Job Object Win32 pour terminer `cmd.exe`, Maven/Gradle et leurs descendants ;
- sortie JSON stable pour `success`, `failed`, `timed_out` et `cancelled` ;
- aucun endpoint de commande shell arbitraire.

Le CLI expose le timeout et la limite de sortie. Le timeout reste compris entre
une seconde et une heure pour eviter une attente pratiquement non bornee.

## Apply transactionnel

Etat implemente :

- manifeste et journal append-only sous `.opticcode/runs/<transaction-id>` ;
- etats `prepared`, `applying`, `applied`, `finalizing`, `committed`,
  `rollback_started`, `rolled_back` et `rollback_failed` ;
- backups bruts avec BLAKE3, taille et permissions ;
- verification optimistic concurrency avant chaque ecriture et chaque undo ;
- remplacement atomique par fichier, avec `ReplaceFileW`/`MoveFileExW` sous Windows ;
- rollback automatique sur erreur apres preparation ;
- listing, inspection et recovery explicite ;
- politique Git propre par defaut, repo sale seulement via `--allow-dirty` ;
- verrou de fichier OS par workspace pour serialiser apply, undo et recovery ;
- refus des symlinks, jonctions et reparse points dans les chemins cibles/journaux ;
- sorties Serde/JSON et codes de sortie distincts.

La transaction n'est pas globalement atomique sur plusieurs fichiers. Elle
garantit qu'un etat partiel est journalise et recuperable sans ecraser une derive
externe inconnue. Voir [`apply-transaction.md`](apply-transaction.md).

## Worktree de verification

Etat implemente :

- source Git propre et commit `HEAD` exact obligatoires ;
- creation bornee d'un worktree detache sous `%TEMP%\opticcode-worktrees` ;
- apply transactionnel et build strict uniquement dans ce worktree ;
- capture de l'etat Git et du diff avant suppression ;
- seconde capture du commit et de l'etat de la source ;
- lease externe au worktree pour recovery apres crash ;
- cleanup Git uniquement apres validation du chemin et de l'enregistrement ;
- aucun transfert automatique vers la source.

Le cycle de vie est isole dans `opticcode-tools/src/worktree.rs`. Il ne doit pas
etre reintegre dans `apply_transaction.rs`. Voir
[`worktree-verification.md`](worktree-verification.md).

## Analyse syntaxique Java

Etat read-only implemente :

- `tree-sitter 0.26.11` et `tree-sitter-java 0.23.5` ;
- module independant `opticcode-tools/src/java_syntax/` ;
- scan borne et deterministe d'un fichier ou projet ;
- symboles, references, annotations et ranges byte/ligne/colonne ;
- commentaires et chaines identifies comme zones non-code ;
- Java partiellement invalide conserve avec diagnostics `ERROR`/`MISSING` ;
- sortie JSON versionnee et commande `java-syntax` ;
- usages de types et arite des appels pour la couche d'index ;
- aucune edition.

Etat index read-only implemente :

- module independant `opticcode-tools/src/java_index/` ;
- identifiants qualifies et signatures d'overloads deterministes ;
- contexte package/imports et resolution inter-fichiers explicable ;
- incertitude structuree, candidats bornes et aucune cible arbitraire ;
- timings separes pour lecture, parse, extraction, index et resolution ;
- commande `java-index` et JSON versionne.

Etat edits read-only implemente :

- module independant `opticcode-tools/src/java_edits/` ;
- 14 regles Bukkit 1.8 partagees avec le workflow legacy historique ;
- cible exacte, qualificateur/import prouve et shadows connus refuses ;
- hash BLAKE3, noeud/octet attendu, ranges non chevauchants et IDs stables ;
- simulation de fin vers debut et reparse en memoire ;
- sortie compacte `java-edits` humaine/JSON, sans aucune ecriture.

La prochaine etape est l'adaptateur `CODE-001B3` qui executera ces edits via
APPLY-001 dans GIT-002, puis un index incremental/persistant lorsque les mesures
le justifieront. Voir [`java-syntax.md`](java-syntax.md),
[`java-index.md`](java-index.md) et [`java-edits.md`](java-edits.md).

## Runtime LLM

Interface cible :

```rust
pub trait LlmProvider {
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse>;
}
```

Providers envisages :

- `OllamaProvider`
- `OpenAiCompatibleProvider`
- `LmStudioProvider`
- `LlamaCppProvider`

Le MVP doit commencer par un seul provider local stable, probablement Ollama ou LM Studio.

Regle d'optimisation :

- mesurer avant de changer de backend ;
- garder le modele chaud quand c'est possible ;
- reduire le contexte avant d'augmenter `num_ctx` ;
- tester Q5_K_M et llama.cpp seulement avec des benchmarks reproductibles.

## Profils

OpticCode doit pouvoir changer de comportement selon le domaine.

Exemples de profils :

```text
minecraft-java-1.8
rust-cli
cpp-perf
iot-embedded
web-backend
```

Chaque profil peut definir :

- prompt systeme court ;
- regles metier ;
- limites de contexte ;
- chemins inclus/exclus ;
- commandes de verification ;
- docs/RAG a injecter ;
- modeles et options recommandees.

Le premier profil cible est `minecraft-java-1.8`.

Etat actuel :

- profil Markdown minimal dans `skills/profiles/minecraft-java-1.8/profile.md` ;
- commande CLI `profile` pour verifier le chargement ;
- commandes `ask` et `plan` avec `--profile`, par defaut `minecraft-java-1.8`.

## Memoire

Memoire cible en trois couches :

```text
global memory
profile memory
project memory
```

V1 simple :

- fichiers Markdown/YAML versionnables ;
- regles chargees dans le prompt ;
- pas de fine-tuning.

V2 :

- SQLite ;
- feedback accepted/rejected ;
- lecons extraites des builds, patches et corrections.

## Daemon et IDE

Le CLI reste prioritaire.

Architecture future :

```text
VS Code extension
  -> opticd local
  -> OpticCode core
  -> index/RAG/memoire
  -> Ollama ou llama.cpp
```

Le daemon `opticd` est repousse apres :

1. patch preview CLI ;
2. build/test fiable ;
3. indexation locale ;
4. profils et memoire simples.

## Strategie RAG

Le RAG ne doit pas etre uniquement vectoriel.

Recherche cible :

1. Full-text exact avec Tantivy.
2. Symboles/classes/methodes avec Tree-sitter.
3. Metadata et memoire avec SQLite.
4. Vectoriel avec Qdrant plus tard.
5. Reranking si necessaire.

## Specialisation Minecraft legacy

OpticCode doit connaitre et appliquer :

- Java 8 obligatoire ;
- Bukkit/Spigot 1.8.8/1.8.9 ;
- PandaSpigot ;
- absence des API modernes ;
- noms legacy de `Material` ;
- patterns de plugins Bukkit anciens ;
- Maven/Gradle legacy ;
- performance serveur PvP/Faction ;
- conventions personnelles du projet.
