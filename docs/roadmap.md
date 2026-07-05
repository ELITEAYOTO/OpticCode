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

Statut : en cours.

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
- initialisation Git bloquee temporairement par une regle Windows `DENY` sur `.git`.

Action manuelle :

- voir `docs/git-setup.md`.

## Phase 2 - Benchmark modele local

Statut : a faire.

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

- runtime principal choisi pour le MVP ;
- quant recommande ;
- taille de contexte de depart ;
- limites constatees.

## Phase 3 - Recherche depots externes

Statut : a faire apres Phase 2.

Objectif :

- etudier les depots utiles sans les copier aveuglement ;
- identifier les patterns a reprendre ;
- decider quels depots cloner localement.

Ordre propose :

1. Qwen Code : architecture agent, tools, modes, memoire, skills.
2. llama.cpp : runtime GGUF, serveur OpenAI-compatible, Vulkan, embeddings/reranking.
3. Tree-sitter : parsing code, surtout Java.
4. Tantivy : recherche texte locale en Rust.
5. Qdrant : RAG vectoriel plus tard.
6. PandaSpigot fork : cible metier principale.

## Phase 4 - Prototype OpticCode minimal

Statut : a faire.

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

## Phase 5 - Tools code et Java legacy

Statut : a faire.

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
