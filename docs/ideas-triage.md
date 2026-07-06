# OpticCode - Tri des idees de recherche

Derniere mise a jour : 2026-07-06

## Objectif

Ce document trie les idees issues des recherches GPT/Qwen et du dossier local `Idees-Vrac`.

But :

- garder les bonnes idees ;
- eviter les fausses optimisations ;
- transformer les pistes utiles en roadmap concrete ;
- viser un vrai outil local, pas seulement un chatbot.

Les notes sources ne sont pas ajoutees au depot Git. Elles restent des brouillons de recherche. Seule cette synthese doit servir de reference projet.

## Verdict court

Les recherches vont globalement dans la bonne direction.

Le meilleur levier pour OpticCode n'est pas de modifier Qwen, ni de reecrire llama.cpp. Le meilleur levier est de construire autour du modele :

1. un moteur de contexte intelligent ;
2. une indexation code/documentation rapide ;
3. des profils specialises ;
4. une memoire projet ;
5. un workflow patch/build/test controle ;
6. des benchmarks reproductibles.

La phrase a garder :

```text
OpticCode ne modifie pas Qwen.
OpticCode transforme Qwen en agent specialise, guide, verifie et optimise pour chaque projet.
```

## Ce qui est tres interessant maintenant

### 1. Reduire le contexte envoye au modele

Priorite : tres haute.

Pourquoi :

- chaque token envoye augmente le cout de prefill ;
- trop de contexte degrade aussi la qualite ;
- sur un modele local, la selection de contexte vaut souvent plus qu'un changement de quantization.

Action OpticCode :

- selectionner les fichiers utiles au lieu d'envoyer tout le projet ;
- extraire classes, methodes, imports, commandes Bukkit et listeners ;
- injecter du code exact seulement quand c'est necessaire ;
- garder un contexte court par defaut.

Etat :

- `context` existe deja ;
- `analyze-java` existe deja ;
- il faut maintenant passer d'un contexte "priorise" a un contexte "choisi selon la tache".

### 2. Profils specialises

Priorite : tres haute.

Exemples :

- `minecraft-java-1.8`
- `rust-cli`
- `cpp-perf`
- `iot-embedded`
- `web-backend`

Chaque profil doit pouvoir definir :

- regles de prompt ;
- limites de contexte ;
- modeles preferes ;
- commandes de verification ;
- RAG/doc locale ;
- regles anti-hallucination ;
- outils autorises.

Action OpticCode :

- commencer par un profil `minecraft-java-1.8` ;
- stocker les regles dans `skills/` ou `data/profiles/` ;
- ne pas tout figer dans un Modelfile Ollama.

### 3. Memoire en trois couches

Priorite : haute.

Structure recommandee :

```text
Global memory
  preferences utilisateur generales

Profile memory
  regles liees a un domaine, par exemple Minecraft 1.8

Project memory
  conventions et decisions propres au repo courant
```

Exemples :

- global : repondre en francais, eviter les solutions trop lourdes ;
- profil Minecraft : Java 8, Bukkit 1.8.8, pas d'API Paper moderne ;
- projet : PandaSpigot fork, conventions de packages, systemes custom.

Action OpticCode :

- commencer simple avec fichiers Markdown/YAML ;
- migrer vers SQLite quand l'indexation arrive ;
- ajouter plus tard `remember`, `learn`, `feedback accepted/rejected`.

### 4. Patch preview avant application

Priorite : tres haute.

Workflow cible :

```text
plan
-> analyse deterministe
-> generation patch texte
-> verification patch
-> confirmation utilisateur
-> apply
-> build/test
-> correction iterative
```

Action OpticCode :

- prochaine etape directe : produire un patch texte non applique ;
- ensuite : `git apply --check` ;
- ensuite : safe apply avec backup ou Git.

### 5. Streaming

Priorite : haute pour le confort, moyenne pour la vitesse reelle.

Le streaming ne rend pas le modele plus rapide en tokens/s, mais il reduit le temps ressenti avant le premier texte visible.

Action OpticCode :

- garder `stream=false` pour les benchmarks reproductibles ;
- ajouter un mode streaming pour l'usage interactif ;
- mesurer TTFT plus tard.

### 6. Benchmarks reproductibles

Priorite : haute.

Mesures a suivre :

- `load_duration` ;
- `prompt_eval_duration` ;
- `eval_duration` ;
- tokens prompt/generation ;
- tokens/s ;
- temps build/test ;
- taille contexte ;
- nombre de fichiers injectes ;
- build vert/rouge apres patch.

Etat :

- `--metrics` existe ;
- `--metrics-json` existe ;
- `load_duration` et `keep_alive` existent ;
- il faut ajouter un runner JSONL/CSV pour comparer les profils.

## Ce qui est interessant, mais pas maintenant

### 1. llama.cpp direct

Priorite : moyenne, apres stabilisation du CLI.

Raison :

- peut donner plus de controle : prompt cache, slots, OpenAI-compatible server, Vulkan ;
- mais ne corrige pas un mauvais contexte ou un mauvais workflow agent ;
- demande un benchmark propre avant decision.

Decision :

- garder Ollama pour V1 ;
- creer une abstraction provider plus propre ;
- benchmarker llama.cpp plus tard sur les memes prompts.

### 2. Qwen2.5-Coder 14B Q5_K_M

Priorite : moyenne.

Interet :

- qualite potentiellement meilleure ;
- peut reduire certaines erreurs de generation.

Risque :

- modele plus lourd ;
- chargement plus long ;
- debit potentiellement plus bas ;
- pas forcement meilleur sur les details Bukkit legacy sans RAG.

Decision :

- rester sur Q4_K_M tant que le workflow agent/RAG n'est pas meilleur ;
- tester Q5_K_M uniquement avec un benchmark qualite/vitesse reproductible ;
- ne pas migrer juste parce que le chiffre de quantization est plus eleve.

### 3. Modelfile Ollama dedie

Priorite : moyenne.

Interet :

- temperature/top_p/num_ctx stables ;
- system prompt de base simple.

Limite :

- mauvais endroit pour gerer les profils, les packs RAG et les commandes de verification.

Decision :

- Modelfile minimal possible plus tard ;
- intelligence metier dans OpticCode, pas dans un Modelfile geant.

### 4. VS Code extension + opticd

Priorite : V1/V2, pas avant un bon CLI.

Architecture cible :

```text
VS Code extension
-> opticd local
-> index/RAG/memoire
-> Ollama ou llama.cpp
-> patch/build/test
```

Interet :

- UX propre ;
- diff natif ;
- apply via l'editeur ;
- futur mode IDE.

Decision :

- ne pas commencer par VS Code ;
- construire d'abord le coeur CLI ;
- concevoir les APIs pour ne pas bloquer le futur daemon.

### 5. MCP, LSP, subagents, worktrees

Priorite : plus tard.

Ce sont de bonnes idees, mais seulement quand le coeur local est fiable.

Ordre realiste :

1. CLI fiable.
2. Patch/build/test.
3. Index/RAG.
4. Memoire/profils.
5. Daemon.
6. VS Code.
7. MCP/LSP/worktrees/subagents.

## Ce qui est a eviter maintenant

### 1. Modifier les poids Qwen ou patcher le GGUF

Verdict : non pour V1.

Raison :

- tres fragile ;
- difficile a evaluer ;
- risque de casser le modele ;
- le RAG, la memoire et les verificateurs donneront un meilleur ratio effort/gain.

### 2. Refaire llama.cpp ou ecrire des kernels custom

Verdict : non pour V1.

Raison :

- effort enorme ;
- ton goulot actuel est surtout orchestration/contexte/workflow ;
- sur Windows + AMD, les gains GPU bas niveau demandent beaucoup de validation.

### 3. Fine-tuning / LoRA immediat

Verdict : pas maintenant.

Raison :

- il faut d'abord collecter de bonnes donnees ;
- un mauvais dataset peut rendre le modele pire ;
- la memoire/RAG peuvent deja corriger les erreurs legacy les plus importantes.

Strategie long terme :

- stocker les patches acceptes/refuses ;
- construire un dataset propre ;
- envisager LoRA seulement apres avoir des centaines ou milliers d'exemples utiles.

### 4. RAG vectoriel lourd des le debut

Verdict : repousser.

Raison :

- le code a besoin de noms exacts ;
- Tantivy/SQLite FTS + symboles Tree-sitter seront plus utiles au depart ;
- les embeddings arrivent en complement, pas en remplacement.

## Configuration Ollama : ce qui est raisonnable

Etat actuel :

- `keep_alive=15m` est deja envoye par OpticCode ;
- les metriques affichent `load_duration` ;
- le modele chaud repond beaucoup plus vite qu'un modele froid.

Pistes a tester, pas a appliquer aveuglement :

```text
OLLAMA_FLASH_ATTENTION=1
OLLAMA_KV_CACHE_TYPE=q8_0
OLLAMA_MAX_LOADED_MODELS=1
OLLAMA_NUM_PARALLEL=1
```

Pourquoi tester :

- Flash Attention peut reduire le cout memoire avec grands contextes ;
- KV cache quantifie peut reduire la memoire ;
- un seul modele charge et peu de parallelisme conviennent a un assistant interactif local.

Pourquoi rester prudent :

- certains reglages dependent fortement du backend et du hardware ;
- Qwen peut etre sensible a certains compromis de cache ;
- toute modification doit passer par benchmark.

Sources officielles verifiees :

- Ollama `/api/generate` expose `load_duration`, `prompt_eval_duration`, `eval_duration` et `keep_alive` : https://docs.ollama.com/api/generate
- Ollama documente `OLLAMA_KV_CACHE_TYPE` et Flash Attention : https://docs.ollama.com/faq
- llama.cpp server documente `cache_prompt` et les options de cache : https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md

## Classement des gains attendus

Tres gros gain :

- garder le modele chaud ;
- reduire les tokens generes ;
- selectionner le contexte par tache ;
- outils deterministes avant LLM ;
- patch/build/test bouclee.

Gros gain qualite :

- profil Minecraft Java 1.8 ;
- RAG docs legacy ;
- memoire de corrections ;
- detection deterministe des APIs modernes interdites.

Gain moyen :

- streaming ;
- Modelfile propre ;
- llama.cpp direct ;
- Q5_K_M si benchmark positif ;
- prompt cache quand le backend le permet.

Gain faible ou incertain maintenant :

- micro-optimisations Rust ;
- C++ custom hors inference ;
- compression de prompts maison ;
- embeddings avant recherche lexicale/symbolique.

Non rentable maintenant :

- patcher GGUF ;
- kernels custom ;
- fine-tuning immediat ;
- daemon/VS Code avant patch CLI fiable.

## Roadmap issue de ce tri

### Maintenant

1. Ajouter patch texte non applique.
2. Ajouter verification de coherence `plugin.yml` <-> `getCommand(...)`.
3. Ajouter profil `minecraft-java-1.8` minimal.
4. Ajouter benchmark JSONL/CSV pour prompts et modeles.
5. Ajouter streaming pour usage interactif.

### Ensuite

1. Ajouter index SQLite simple : fichiers, hash, metadata.
2. Ajouter Tree-sitter Java pour symboles.
3. Ajouter recherche Tantivy ou SQLite FTS.
4. Ajouter memoire projet/profile/global.
5. Ajouter feedback `accepted/rejected` sur patches.

### Plus tard

1. Benchmark Q5_K_M vs Q4_K_M.
2. Benchmark llama.cpp direct vs Ollama.
3. VS Code extension + `opticd`.
4. MCP/LSP/worktrees.
5. Dataset pour LoRA eventuel.

## Questions a garder ouvertes

- Est-ce que Q5_K_M ameliore vraiment les corrections Bukkit legacy par rapport a Q4_K_M + RAG ?
- Est-ce que llama.cpp/Vulkan est plus rapide que Ollama sur la Radeon RX 9060 XT sous Windows 10 ?
- Est-ce que Tantivy suffit ou faut-il SQLite FTS5 aussi ?
- Quand faut-il passer du CLI a `opticd` ?
- Quel est le premier vrai projet complexe a analyser apres le mini plugin : plugin reel ou fork PandaSpigot ?
