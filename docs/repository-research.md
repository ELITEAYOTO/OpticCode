# OpticCode - Recherche depots externes

Derniere mise a jour : 2026-07-06

## Objectif Phase 3

Etudier les depots et documentations utiles sans cloner inutilement. Le but est de savoir ce qui aide vraiment OpticCode :

- agent code local ;
- runtime local Qwen2.5-Coder GGUF ;
- outils lecture/recherche/patch/compilation ;
- RAG local ;
- specialisation Java 8 / Bukkit / Spigot / PandaSpigot 1.8.8.

## Regle de clonage

On ne clone pas un depot externe parce qu'il est populaire. On le clone seulement si l'une de ces raisons est vraie :

- il faut lire son code localement ;
- il faut compiler ou tester un binaire ;
- il faut extraire des exemples, patterns ou fichiers de reference ;
- il devient une cible metier directe, comme PandaSpigot.

Les depots externes ne doivent pas etre copies dans le code OpticCode. Ils servent a apprendre, comparer et valider des choix.

## Sources verifiees

Snapshot GitHub realise le 2026-07-06.

| Projet | Depot / doc | Langage principal | Licence | Utilite OpticCode | Decision |
| --- | --- | --- | --- | --- | --- |
| Ollama API | https://docs.ollama.com/api/generate | N/A | N/A | Provider LLM local du MVP | utiliser maintenant, pas besoin de cloner |
| Qwen2.5-Coder GGUF | https://huggingface.co/Qwen/Qwen2.5-Coder-14B-Instruct-GGUF | N/A | voir Hugging Face | Modele cible GGUF Q4_K_M | utiliser via Ollama maintenant, llama.cpp plus tard |
| Qwen Code | https://github.com/QwenLM/qwen-code | TypeScript | Apache-2.0 | Architecture agent, CLI/core/tools, permissions, memoire | cloner en premier pour lecture locale |
| llama.cpp | https://github.com/ggml-org/llama.cpp | C++ | MIT | Runtime GGUF, serveur local, Vulkan AMD | cloner plus tard pour compilation/test |
| tree-sitter-java | https://github.com/tree-sitter/tree-sitter-java | JavaScript | MIT | Grammaire Java pour analyse de code | utiliser via crate/dependance d'abord |
| Tantivy | https://github.com/quickwit-oss/tantivy | Rust | MIT | Recherche texte locale type BM25 | utiliser comme dependance Rust |
| Qdrant | https://github.com/qdrant/qdrant | Rust | Apache-2.0 | Base vectorielle RAG plus avancee | attendre |
| Qdrant Rust client | https://github.com/qdrant/rust-client | Rust | Apache-2.0 | Client Rust si Qdrant est retenu | attendre |
| PandaSpigot | https://github.com/hpfxd/PandaSpigot | Shell / Java | GPL-3.0 | Cible metier Minecraft 1.8.8 | cloner apres le squelette MVP |

## Analyse par depot

### 1. Ollama API

Role :

- provider LLM principal du MVP ;
- endpoint simple en local ;
- mesures propres deja obtenues via `POST /api/generate`.

Pourquoi c'est important :

- le benchmark Phase 2 a valide Ollama comme runtime provisoire ;
- l'API retourne des metriques utiles : `total_duration`, `prompt_eval_count`, `eval_count`, `eval_duration` ;
- OpticCode peut commencer par un provider HTTP tres simple.

Decision :

- ne pas cloner Ollama maintenant ;
- documenter le provider dans le futur crate Rust ;
- privilegier l'API locale pour le MVP.

Points a tester plus tard :

- `/api/chat` pour conversations multi-messages ;
- `stream=false` pour tests reproductibles ;
- OpenAI compatibility si on veut rendre le provider interchangeable avec llama.cpp ou LM Studio.

### 2. Qwen2.5-Coder 14B Instruct GGUF

Role :

- modele principal vise par OpticCode ;
- version GGUF Q4_K_M utilisable avec Ollama et llama.cpp.

Constat :

- le modele Ollama `qwen2.5-coder:14b` est deja installe ;
- Hugging Face documente aussi l'usage direct avec `llama serve -hf Qwen/Qwen2.5-Coder-14B-Instruct-GGUF:Q4_K_M`.

Decision :

- continuer avec Ollama pour le MVP ;
- garder le GGUF officiel Qwen comme cible llama.cpp future ;
- ne pas multiplier les variantes de quant tant que le MVP agentique n'existe pas.

### 3. Qwen Code

Role :

- reference principale pour l'architecture agent code ;
- pas une base a copier, mais une source d'inspiration serieuse.

Ce qu'il faut etudier :

- separation CLI / core ;
- gestion des outils ;
- politique d'approbation ;
- lecture multi-fichiers ;
- shell avec garde-fous ;
- memoire ;
- configuration projet/utilisateur ;
- eventuellement daemon plus tard, mais pas pour le MVP.

Elements reperes :

- depot TypeScript Apache-2.0 ;
- architecture documentee autour de `packages/cli`, `packages/core` et `packages/core/src/tools`;
- tools typiques : fichiers, shell, recherche, web, MCP ;
- design oriente modularite, extensibilite, securite et configuration multi-niveaux.

Decision :

- cloner Qwen Code en premier dans un dossier separe de reference, par exemple `C:\Users\timot\Desktop\OpticCode-research\qwen-code` ;
- ne rien importer directement dans OpticCode ;
- produire ensuite une note courte : ce qu'on reprend, ce qu'on refuse, ce qu'on simplifie.

Pourquoi pas maintenant dans `OpticCode/` :

- le depot OpticCode doit rester propre ;
- un clone externe volumineux ne doit pas polluer l'historique ;
- Qwen Code est en TypeScript alors qu'OpticCode vise Rust.

### 4. llama.cpp

Role :

- runtime GGUF C/C++ de reference ;
- possible remplacement ou backend alternatif a Ollama ;
- important pour l'optimisation locale et Vulkan AMD.

Ce qu'il faut etudier :

- build Windows ;
- `llama-server` OpenAI-compatible ;
- options Vulkan : `-DGGML_VULKAN=ON` ou `-DGGML_VULKAN=1` selon contexte ;
- lancement avec `-ngl` pour decharger des couches sur GPU ;
- comportement avec Qwen2.5-Coder 14B Q4_K_M.

Decision :

- ne pas compiler maintenant ;
- cloner seulement quand on passe au benchmark runtime avance ;
- avant de compiler, installer/verifier Vulkan SDK si necessaire.

Raison :

- Ollama fonctionne deja ;
- compiler llama.cpp maintenant consommerait du temps avant d'avoir un MVP agentique ;
- on y reviendra quand les providers seront abstraits.

### 5. Tree-sitter / tree-sitter-java

Role :

- parser Java local ;
- extraction de classes, methodes, imports, packages, annotations ;
- base pour index symbolique et contexte RAG.

Pourquoi c'est pertinent :

- Tree-sitter est rapide, robuste avec du code incomplet, et fait pour l'analyse incremental ;
- la grammaire Java existe officiellement.

Decision :

- ne pas cloner le depot principal maintenant ;
- utiliser les crates Rust et la grammaire Java quand le crate `opticcode-index` demarrera ;
- cloner `tree-sitter-java` seulement si on doit inspecter la grammaire ou debugger des captures.

### 6. Tantivy

Role :

- moteur de recherche full-text local en Rust ;
- indexation docs/code ;
- complement simple et rapide avant embeddings/vectoriel.

Pourquoi c'est pertinent :

- OpticCode doit retrouver rapidement des docs, conventions et morceaux de code ;
- BM25 suffit pour beaucoup de recherches utiles au MVP ;
- Tantivy evite de commencer par une base vectorielle lourde.

Decision :

- ne pas cloner maintenant ;
- l'ajouter comme dependance Rust quand l'index local commence ;
- priorite avant Qdrant.

### 7. Qdrant

Role :

- recherche vectorielle et RAG avance ;
- utile quand on aura valide embeddings, chunking et scoring hybride.

Constat :

- Qdrant peut tourner localement via Docker ;
- le client Rust officiel existe ;
- mais cela ajoute une dependance runtime supplementaire.

Decision :

- attendre ;
- ne pas installer Docker juste pour Qdrant maintenant ;
- commencer par SQLite + Tantivy + Tree-sitter.

Quand y revenir :

- quand le RAG texte/symboles existe ;
- quand on a un vrai dataset Minecraft legacy ;
- quand on sait quel modele d'embedding local utiliser.

### 8. PandaSpigot

Role :

- cible metier centrale ;
- source de verite pour build, patches, conventions, erreurs legacy ;
- benchmark realiste apres le mini plugin.

Decision :

- cloner apres la creation du squelette MVP ;
- idealement cloner ton fork exact, pas seulement `hpfxd/PandaSpigot`, si ton fork contient tes modifications ;
- garder le clone hors du depot OpticCode, par exemple `C:\Users\timot\Desktop\OpticCode-workspaces\PandaSpigot`.

Pourquoi pas tout de suite :

- PandaSpigot est lourd conceptuellement ;
- sans outils de lecture/indexation, on va juste le regarder a la main ;
- mieux vaut d'abord construire le minimum qui lit, cherche et resume.

## Ordre de travail recommande

### Phase 3A - Terminee

- verifier les depots et docs ;
- confirmer l'ordre de priorite ;
- mettre a jour cette documentation.

### Phase 3B - A faire ensuite

Cloner uniquement Qwen Code en reference externe.

Statut : fait.

Objectif :

- lire son architecture localement ;
- identifier les patterns a reprendre pour OpticCode ;
- produire `docs/qwen-code-analysis.md`.

Commande proposee, hors depot OpticCode :

```powershell
mkdir C:\Users\timot\Desktop\OpticCode-research
cd C:\Users\timot\Desktop\OpticCode-research
git clone https://github.com/QwenLM/qwen-code.git
```

Clone realise :

```text
C:\Users\timot\Desktop\OpticCode-research\qwen-code
```

Analyse produite :

```text
docs/qwen-code-analysis.md
```

### Phase 3C - Plus tard

Cloner llama.cpp seulement quand on veut tester le runtime GGUF optimise :

```powershell
cd C:\Users\timot\Desktop\OpticCode-research
git clone https://github.com/ggml-org/llama.cpp.git
```

### Phase 3D - Apres squelette MVP

Cloner le fork PandaSpigot cible :

```powershell
mkdir C:\Users\timot\Desktop\OpticCode-workspaces
cd C:\Users\timot\Desktop\OpticCode-workspaces
git clone <URL_DE_TON_FORK_PANDASPIGOT>
```

## Decisions Phase 3

1. Ollama reste le provider MVP.
2. Qwen Code a ete clone hors du depot OpticCode pour analyse d'architecture.
3. llama.cpp est important, mais pas prioritaire tant qu'Ollama suffit.
4. Tree-sitter Java et Tantivy seront des dependances Rust, pas des clones au debut.
5. Qdrant attendra la preuve de besoin.
6. PandaSpigot sera clone quand OpticCode saura deja lire/rechercher/indexer.

## Questions restantes

1. Quelle est l'URL exacte de ton fork PandaSpigot ?
2. Veux-tu garder les clones de recherche sur le Bureau, dans `C:\Users\timot\Desktop\OpticCode-research` ?
3. Souhaites-tu que Qwen Code soit uniquement etudie comme reference, ou aussi installe/teste en CLI ?
