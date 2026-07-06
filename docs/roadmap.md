# OpticCode - Roadmap

Derniere mise a jour : 2026-07-06

## Vision courte

OpticCode doit devenir un agent code local specialise pour tes projets Java Minecraft 1.8.8 / 1.8.9, PandaSpigot, Bukkit/Spigot legacy, plugins et documentation personnelle.

Le modele IA n'est pas entraine depuis zero. OpticCode construit une couche agentique propre autour d'un modele deja entraine, d'abord Qwen2.5-Coder 14B Instruct GGUF Q4_K_M.

## Phase 0 - Audit environnement Windows 10

Statut : terminee.

Objectif :

- verifier les outils presents ;
- corriger Java/Maven vers JDK 8 ;
- installer les outils de build necessaires ;
- eviter les installations inutiles.

Resultat :

- Git, Rust, Cargo, MSVC Build Tools, CMake, Ninja, JDK 8, Maven, Ollama, LM Studio CLI, Vulkan et SQLite sont OK.
- Java/Maven sont maintenant alignes sur JDK 8.

## Phase 1 - Documentation de cadrage

Statut : en cours.

Objectif :

- figer l'etat de l'environnement ;
- definir l'architecture cible ;
- definir les premieres decisions techniques ;
- preparer l'ordre de recherche des depots externes ;
- eviter de coder avant d'avoir un cadre stable.

Livrables :

- `docs/environment-audit.md`
- `docs/architecture.md`
- `docs/roadmap.md`
- `docs/repository-research.md`
- `docs/decisions.md`
- `docs/model-benchmark.md`

## Phase 1.5 - Initialisation projet local

Statut : terminee.

Objectif :

- creer `README.md` ;
- creer `.gitignore` ;
- preparer l'arborescence locale ;
- initialiser Git ;
- garder une base propre avant les benchmarks et le code.

Etat :

- `README.md` cree ;
- `.gitignore` cree ;
- dossiers `crates/`, `data/`, `skills/`, `models/`, `benchmarks/`, `scripts/` crees ;
- Git initialise ;
- commit initial cree : `4df52e4 Initialisation du projet OpticCode`.

## Phase 2 - Benchmark modele local

Statut : terminee pour le premier passage Ollama.

Objectif :

- verifier que Qwen2.5-Coder 14B GGUF Q4_K_M est utilisable sur la machine ;
- comparer Ollama et LM Studio en usage reel ;
- mesurer vitesse, latence, qualite de reponse et confort avec contexte 8K/16K.

Tests minimum :

1. Generer un mini plugin Bukkit Java 8 compatible 1.8.8.
2. Corriger volontairement une erreur Maven/Java 8 simple.
3. Demander une analyse d'un petit projet Java local.
4. Tester une question legacy Bukkit : materiaux, events, API modernes interdites.

Sortie attendue :

- runtime principal provisoire choisi pour le MVP : Ollama API locale ;
- modele installe : `qwen2.5-coder:14b` ;
- taille locale : 9.0 GB ;
- limites constatees et documentees dans `docs/model-benchmark-results.md`.

Conclusion courte :

- Ollama est suffisant pour demarrer le MVP agentique ;
- la qualite Java 8 generale est correcte ;
- les details Bukkit legacy precis ne sont pas assez fiables sans RAG et regles projet ;
- llama.cpp reste une piste d'optimisation apres le MVP.

## Phase 3 - Recherche depots externes

Statut : terminee pour le cadrage initial.

Objectif :

- etudier les depots utiles sans les copier aveuglement ;
- identifier les patterns a reprendre ;
- decider quels depots cloner localement.

Ordre propose :

1. Qwen Code : premier depot a cloner pour analyse d'architecture.
2. Ollama API : provider MVP deja valide, pas besoin de cloner.
3. llama.cpp : runtime GGUF a cloner plus tard pour benchmark avance.
4. Tree-sitter Java : a utiliser comme dependance Rust au moment de l'indexation.
5. Tantivy : a utiliser comme dependance Rust pour la recherche texte.
6. Qdrant : a repousser apres preuve de besoin vectoriel.
7. PandaSpigot fork : a cloner apres squelette MVP et idealement avec l'URL du fork exact.

Livrable :

- `docs/repository-research.md`
- `docs/qwen-code-analysis.md`

Etat :

- Qwen Code clone hors depot OpticCode dans `C:\Users\timot\Desktop\OpticCode-research\qwen-code` ;
- analyse initiale terminee ;
- prochaine phase : squelette Rust MVP.

## Phase 4 - Prototype OpticCode minimal

Statut : demarre, squelette Rust initial fonctionnel.

Objectif :

Construire un CLI Rust minimal capable de :

- charger une configuration ;
- parler a un provider LLM local ;
- lire un dossier projet ;
- rechercher dans les fichiers ;
- proposer un plan ;
- produire un patch sans l'appliquer automatiquement.

Scope volontairement limite :

- pas de RAG vectoriel complet ;
- pas d'autonomie dangereuse ;
- pas d'edition automatique sans confirmation ;
- pas de GUI.

Etat actuel :

- workspace Cargo cree ;
- crates `opticcode-cli`, `opticcode-core`, `opticcode-llm`, `opticcode-tools` crees ;
- commande `inspect` fonctionnelle ;
- commande `search` fonctionnelle ;
- commande `ask` fonctionnelle avec Ollama ;
- commande `plan` fonctionnelle avec Ollama ;
- commande `context` fonctionnelle ;
- metriques LLM disponibles via `--metrics` ;
- export JSON des metriques disponible via `--metrics-json` ;
- mode bref disponible via `--brief` ;
- limite de generation disponible via `--max-tokens` ;
- mini projet benchmark Bukkit Java 8 ajoute ;
- commande `analyze-java` fonctionnelle ;
- compilation et tests OK.

Livrable :

- `docs/phase-4-mvp.md`
- `docs/mini-bukkit-benchmark.md`
- `docs/optimization-notes.md`
- `docs/java-project-intelligence.md`

## Phase 5 - Tools code et Java legacy

Statut : demarree.

Objectif :

- ajouter les outils de lecture, recherche, diff, patch ;
- detecter Maven/Gradle ;
- lancer la compilation ;
- analyser les erreurs Java ;
- appliquer des corrections simples.

Priorite metier :

- Java 8 strict ;
- Bukkit/Spigot 1.8.8 ;
- compatibilite PandaSpigot ;
- eviter les API modernes.

Etat actuel :

- `analyze-java` detecte Maven, `pom.xml`, `plugin.yml`, commandes, listeners et risques simples ;
- prochaine commande cible : `build`.

## Phase 6 - RAG local

Statut : a faire.

Objectif :

- indexer docs, conventions, exemples de plugins, mappings legacy ;
- melanger recherche texte, symboles et embeddings ;
- garder une base de connaissances reutilisable.

Approche recommandee :

1. SQLite pour metadata et memoire.
2. Tantivy pour recherche full-text.
3. Tree-sitter pour symboles/classes/methodes.
4. Qdrant seulement quand les embeddings sont valides.

## Phase 7 - Agent iteratif

Statut : a faire.

Objectif :

- planifier ;
- lire ;
- modifier ;
- compiler ;
- lire les erreurs ;
- corriger ;
- produire un diff final.

Regle importante :

OpticCode doit etre utile avant d'etre autonome. La fiabilite passe avant le spectacle.
