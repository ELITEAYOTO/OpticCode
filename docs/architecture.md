# OpticCode - Architecture cible

Derniere mise a jour : 2026-08-04

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
  +-- opticcode-policy
  |     +-- typed action model and deny-by-default engine
  |     +-- path, Git and worktree boundaries
  |     +-- one-shot state-bound approvals
  |     +-- bounded workspace-namespaced audit
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
- Le core Rust soumet chaque action a `opticcode-policy`.
- Le LLM, le Chat et TypeScript ne sont jamais l'autorite de securite.
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
- 26 regles Bukkit 1.8 partagees avec le workflow legacy historique ;
- catalogue V2 avec versions, niveau de preuve et sources Spigot SHA-256 ;
- cible exacte, qualificateur/import prouve et shadows connus refuses ;
- hash BLAKE3, noeud/octet attendu, ranges non chevauchants et IDs stables ;
- simulation de fin vers debut et reparse en memoire ;
- sortie compacte `java-edits` humaine/JSON, sans aucune ecriture.

Etat verification B3 implemente :

- module d'orchestration separe `opticcode-tools/src/java_edit_worktree.rs` ;
- empreinte complete des contrats B2 source/worktree au meme commit ;
- rematerialisation hash/ranges/octets/overlaps juste avant APPLY-001 ;
- mutations transactionnelles uniquement dans le worktree GIT-002 ;
- verification des octets et reparse apres ecriture ;
- build borne avec Git State Guard strict ;
- validation des hashes attendus dans le snapshot Git final ;
- diff et patch bornes, cleanup/recovery et source recontrolee ;
- aucune promotion vers le projet source.

Etat contexte CONTEXT-001 implemente :

- module independant `opticcode-tools/src/java_context/` ;
- requete normalisee, correspondances bornees et scores expliques ;
- declarations principales, overloads, ambiguites et appelants exacts ;
- expansion read-only a un niveau avec visited set et cycles signales ;
- ranges AST relues une seule fois par fichier et hashes controles ;
- `pom.xml` et `plugin.yml` selectionnes seulement selon la demande ;
- budgets fichiers/symboles/relations/snippets/octets/caracteres/tokens ;
- diagnostics, avertissements, ignores et troncatures exposes dans un JSON plat ;
- `analysis_complete` separe de `selection_complete` ;
- commande `java-context` isolee du grand derive Clap pour la pile Windows ;
- aucun branchement automatique au runtime LLM avant comparaison A/B.

Etat evaluation EVAL-001 implemente :

- module independant `opticcode-tools/src/eval/` separe par schema, metriques,
  runner et rapports ;
- corpus versionne de 45 cas sur fixtures artificielles ;
- PandaSpigot/Kspawners restent des sources externes optionnelles read-only ;
- strategies legacy, symbolique, exacte et RAG v2 sans nouveau moteur ;
- rapports Serde versionnes, configuration hashee et identite RAG validee ;
- fingerprint complet avant/apres et refus de publication en cas de derive ;
- sous-parseur CLI isole et sortie JSON pure.

Etat integration CONTEXT-002 implemente :

- `ask` et `plan` partagent un runtime versionne sans supprimer les anciennes sorties ;
- modes explicites `legacy`, `symbol` et `compare`, legacy restant le defaut ;
- comparaison des contextes sans generation par defaut et double appel uniquement
  avec autorisation explicite ;
- refus ou fallback visible si analyse incomplete, limite critique, ambiguite,
  derive source ou projet non supporte ;
- prompt stable compose systeme, politique, outils, profil, historique, demande,
  puis contexte dynamique ;
- RAG v2 uniquement, valide via `CURRENT`, sans lecture permissive d'un index legacy ;
- URL Ollama locale verifiee, timeout, seed, temperature et limite explicites ;
- rapport JSON pur sans contenu source dans les metadata de contexte ;
- enrichissement EVAL avec vrais tokens et temps Ollama.

Le cycle GIT-002 accepte maintenant une etape apply injectee, mais conserve une
seule implementation du worktree, du build, du diff et du cleanup. Les futures
regles Java ne doivent pas ajouter leur logique dans `worktree.rs`.

LEGACY-002, CONTEXT-001, RAG-SAFE-001, EVAL-001, CONTEXT-002 et
LLM/PROTOCOL-001 sont termines.
Le contexte symbolique reste optionnel : il reduit le prefill mais n'a pas encore
depasse la qualite legacy sur l'echantillon Qwen. Un index incremental/persistant
attendra un prototype Tantivy mesure. Voir [`java-syntax.md`](java-syntax.md),
[`java-index.md`](java-index.md), [`java-edits.md`](java-edits.md) et
[`java-edit-worktree.md`](java-edit-worktree.md), puis
[`java-context.md`](java-context.md), [`evaluation.md`](evaluation.md) et
[`context-integration.md`](context-integration.md).

## Runtime LLM

Etat LLM/PROTOCOL-001 implemente :

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn endpoint(&self) -> &str;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn health(&self, request: HealthRequest) -> Result<HealthReport, ProviderError>;
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;
    async fn generate(&self, request: GenerationRequest, cancellation: CancellationToken)
        -> Result<GenerationResult, ProviderError>;
    async fn stream(&self, request: GenerationRequest, events: EventSink,
        cancellation: CancellationToken) -> Result<GenerationResult, ProviderError>;
}
```

- contrats provider Serde sous `opticcode.llm` schema 1 ;
- cycle assistant sous `opticcode.assistant` schema 1 ;
- request IDs, tailles, timeouts, canaux et lignes NDJSON bornes ;
- sequences strictes et exactement un terminal valide par flux ;
- streaming, backpressure et annulation cooperative de bout en bout ;
- `OllamaProvider` local comme seule implementation de production ;
- injection `Arc<dyn LlmProvider>` testee dans le coeur ;
- anciens appels non streames et EVAL preserves.

Les providers OpenAI-compatible, LM Studio et llama.cpp restent des options
futures. Ils ne doivent etre ajoutes qu'avec leurs propres benchmarks et sans
affaiblir la restriction locale du chemin Ollama.

Reference : [`llm-protocol.md`](llm-protocol.md).

## Autorite Policy

Etat POLICY-001 implemente :

- crate independant `opticcode-policy` ;
- protocole `opticcode.policy` schema 1 ;
- actions typees et decisions `Allow`, `RequireApproval`, `Deny` ;
- modes `read_only`, `worktree_edit` et `approved_apply` ;
- canonicalisation, refus secrets/symlinks/jonctions/reparse et empreintes
  TOCTOU ;
- frontieres Git explicites pour worktree, gitdir, commondir, index et objects ;
- worktree lie a sa lease, son workspace et son request ID ;
- Maven/Gradle allowlistes avec wrapper source inchange, cwd confine, arguments,
  environnement, timeout, sortie et reseau declares ;
- approvals opaques one-shot lies a request/workspace/mode/HEAD/working tree/
  diff/fichiers/actions/transaction ;
- claim de consommation atomique et fail-closed apres crash ;
- audit atomique, borne, hors source et namespace par workspace ;
- CLI `policy check`, `policy explain`, `policy audit` et codes stables ;
- discovery enrichi sans suppression de champs historiques.

Le preflight expose une revalidation des chemins et une revalidation contre un
nouvel etat observe. La politique ne remplace ni APPLY-001, ni GIT-002, ni le
Process Runner : elle decide si leur invocation structuree peut avoir lieu.

Reference : [`policy-engine.md`](policy-engine.md).

## Interface Chat VS Code

Etat VSCODE-CHAT-001 + POLICY-001 implemente :

```text
ChatRequest VS Code
  -> normalisation bornee et namespace workspace/session
  -> opticcode.chat schema 1 sur stdin
  -> PolicyEngine Rust avec mode effectif read_only
  -> contexte/RAG/Java et LlmProvider existants
  -> evenements NDJSON sequences
  -> ChatResponseStream natif
```

- un participant stable `opticcode.chat`, visible sous `@opticcode` ;
- aucune proposed API, webview, LSP ou daemon ;
- transport NDJSON factorise avec le client Assistant existant ;
- references fichiers/ranges/selections canonicalisees et relues par Rust ;
- historique borne et metadata de session separees par workspace ;
- un terminal unique et annulation structuree ;
- decision, `rule_id` et mode Policy effectif dans l'evenement d'acceptation ;
- commandes d'edition fermees jusqu'au pipeline CHAT-EDIT-001 verifie.

La couche TypeScript ne devient pas une seconde implementation du core. Elle
adapte uniquement les objets VS Code au protocole machine et les evenements aux
composants natifs du Chat.

Reference : [`vscode-chat.md`](vscode-chat.md).

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
