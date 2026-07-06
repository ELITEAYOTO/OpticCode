# OpticCode - Architecture cible

Derniere mise a jour : 2026-07-06

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
  |     +-- edit/apply patch
  |     +-- git diff/status
  |     +-- maven/gradle build
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
- Les commandes shell doivent etre limitees et explicites.
- La compilation doit etre lancee quand c'est raisonnable.
- Les erreurs de build deviennent des donnees reutilisables.
- Les contraintes Java 8 / Bukkit 1.8.8 doivent etre injectees dans le contexte.

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
