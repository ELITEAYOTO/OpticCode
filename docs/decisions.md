# OpticCode - Decisions techniques

Derniere mise a jour : 2026-07-06

## Decisions validees

### D-001 - Ne pas entrainer un modele depuis zero

Statut : valide.

OpticCode utilisera un modele deja entraine. Le projet porte sur l'agent, les tools, le RAG, la memoire et la specialisation metier.

### D-002 - Rust comme langage principal

Statut : valide provisoirement.

Rust est retenu pour le core, le CLI, les tools, la configuration, la memoire et le RAG.

### D-003 - C++ limite au runtime bas niveau

Statut : valide provisoirement.

C++ ne doit pas devenir le langage principal du projet. Il intervient surtout via llama.cpp ou des dependances natives.

### D-004 - Java 8 comme cible Minecraft legacy

Statut : valide.

Java 8 est obligatoire pour PandaSpigot / Bukkit / Spigot 1.8.8.

### D-005 - Maven doit utiliser JDK 8

Statut : valide et corrige.

Maven utilise maintenant Temurin JDK 8.

## Decisions a prendre bientot

### D-006 - Runtime principal du MVP

Statut : valide provisoirement.

Options :

- Ollama ;
- LM Studio OpenAI-compatible ;
- llama.cpp direct.

Decision :

- commencer le MVP avec Ollama via API locale ;
- garder LM Studio comme alternative de comparaison ;
- garder llama.cpp pour l'optimisation runtime et l'integration GGUF plus bas niveau.

Raison :

- Ollama 0.31.1 fonctionne sur la machine ;
- `qwen2.5-coder:14b` est installe et repond correctement via API locale ;
- la vitesse mesuree apres chargement est acceptable pour un MVP ;
- cela permet de construire les couches agent, tools et RAG sans attendre l'integration C++.

Limite :

- le modele hallucine encore sur certains details Bukkit/Spigot 1.8.8 ;
- la fiabilite legacy devra venir des regles OpticCode, de la documentation locale et de tests de compilation.

### D-007 - Mode d'edition des fichiers

Options :

- proposer seulement des patches ;
- appliquer apres confirmation ;
- appliquer automatiquement dans un dossier autorise.

Recommandation actuelle :

- MVP : proposer des patches, puis confirmation explicite.

### D-008 - Premier projet de benchmark

Options :

- mini plugin Bukkit de test ;
- plugin existant ;
- fork PandaSpigot.

Recommandation actuelle :

- commencer par un mini plugin Bukkit Java 8 ;
- passer ensuite a un plugin reel ;
- garder PandaSpigot pour un benchmark plus lourd.

### D-009 - RAG vectoriel

Options :

- Qdrant des le debut ;
- SQLite + Tantivy + Tree-sitter d'abord ;
- embeddings plus tard.

Recommandation actuelle :

- ne pas commencer par Qdrant ;
- construire d'abord recherche texte + symboles + metadata.

### D-010 - Initialisation Git locale

Statut : valide.

Git a ete initialise manuellement dans `C:\Users\timot\Desktop\OpticCode`.

Decision :

- le commit initial `4df52e4 Initialisation du projet OpticCode` sert de point de depart propre.

### D-011 - RAG legacy obligatoire pour Bukkit 1.8.8

Statut : valide provisoirement.

Decision :

- OpticCode ne devra pas s'appuyer uniquement sur le modele pour les details legacy ;
- les mappings Bukkit/Spigot 1.8.8, exemples PandaSpigot et conventions Java 8 devront etre indexes ;
- les sorties du modele devront etre relues par des regles et, quand possible, par compilation.

Raison :

- le benchmark a montre une bonne reponse avec `Material.SULPHUR` dans un cas ;
- le meme modele a hallucine une correction incorrecte pour `Material.GUNPOWDER` dans un autre cas.

### D-012 - Politique de clonage des depots externes

Statut : valide provisoirement.

Decision :

- les depots de recherche seront clones hors du depot OpticCode ;
- Qwen Code a ete clone hors du depot OpticCode, pour analyse d'architecture ;
- llama.cpp sera clone seulement au moment du benchmark runtime avance ;
- PandaSpigot sera clone quand OpticCode aura un squelette capable de lire, chercher et resumer.

Raison :

- garder le depot OpticCode propre ;
- eviter de melanger code source externe, benchmarks et code projet ;
- progresser par apprentissage cible au lieu de collectionner des depots.

### D-013 - Architecture MVP inspiree par Qwen Code, mais simplifiee

Statut : valide provisoirement.

Decision :

- OpticCode reprend les principes utiles : core separe du CLI, registre d'outils, validation des appels, lecture avant edition, diff avant application ;
- OpticCode ne reprend pas la complexite complete : daemon, nombreux SDK, channels, extensions, MCP et subagents sont repousses ;
- le MVP Rust commence par `opticcode-cli`, `opticcode-core`, `opticcode-llm` et `opticcode-tools`.

Raison :

- Qwen Code montre des garde-fous solides pour un agent code ;
- OpticCode doit rester specialise Minecraft Java 8 et avancer par etapes ;
- le premier prototype doit etre utile avant d'etre une plateforme complete.

### D-014 - Premier workspace Rust

Statut : valide provisoirement.

Decision :

- creer un workspace Cargo avec quatre crates initiales ;
- `opticcode-cli` pour l'interface ligne de commande ;
- `opticcode-core` pour l'orchestration ;
- `opticcode-llm` pour le provider Ollama ;
- `opticcode-tools` pour inspection et recherche locale.

Raison :

- garder une separation claire des responsabilites ;
- permettre de remplacer Ollama plus tard par LM Studio ou llama.cpp ;
- eviter que le CLI devienne le coeur du projet ;
- preparer l'ajout futur de l'indexation, du RAG et des outils Java.

### D-015 - Commande plan avant edition

Statut : valide provisoirement.

Decision :

- ajouter une commande `plan` avant toute commande de patch ou d'edition ;
- le mode plan doit produire une strategie, les fichiers probables, les risques legacy et les verifications ;
- le mode plan ne doit pas modifier les fichiers ni pretendre l'avoir fait.

Raison :

- OpticCode doit rester fiable avant d'etre autonome ;
- les details Bukkit/Spigot 1.8.8 demandent une phase de verification ;
- separer plan et patch rendra le futur agent plus controlable.

### D-016 - Optimiser par mesure, pas par intuition

Statut : valide provisoirement.

Decision :

- mesurer les temps des tools locaux, builds Java et appels LLM ;
- optimiser d'abord le prompt et le contexte ;
- comparer llama.cpp/C++ seulement avec des benchmarks reproductibles.

Raison :

- le mini benchmark montre que les outils Rust locaux sont rapides ;
- le temps dominant actuel est l'inference Qwen via Ollama ;
- descendre trop tot en C++ ne corrigera pas un mauvais preprompt ou un contexte trop large.

### D-017 - Contexte enrichi mais borne

Statut : valide provisoirement.

Decision :

- ajouter une commande `context` ;
- inclure des extraits limites de fichiers importants ;
- prioriser `pom.xml`, `plugin.yml`, classe principale, commandes et listeners ;
- garder une limite stricte de taille.

Raison :

- le modele raisonne mieux quand il voit le vrai code ;
- les premiers tests montrent que le cout principal reste la generation, pas l'evaluation du contexte ;
- ce contexte servira de base aux futurs patches non appliques.

### D-018 - Mode bref pour iteration rapide

Statut : valide provisoirement.

Decision :

- ajouter `--brief` aux commandes LLM ;
- ajouter `--max-tokens` pour controler la generation Ollama ;
- utiliser ce mode pour les boucles rapides de verification.

Raison :

- le benchmark mini Bukkit passe d'environ 26 s a environ 6.4 s ;
- le contexte reste enrichi ;
- la reduction vient surtout du nombre de tokens generes.

### D-019 - Metriques exportables

Statut : valide provisoirement.

Decision :

- ajouter `--metrics-json` aux commandes LLM ;
- imprimer les metriques en JSON pour faciliter les comparaisons ;
- garder la sortie texte humaine avec `--metrics`.

Raison :

- les futures comparaisons Ollama / llama.cpp / prompts doivent etre reproductibles ;
- recopier les chiffres a la main devient vite source d'erreurs ;
- JSON est suffisant avant de construire un vrai runner benchmark.

### D-020 - Analyse Java deterministe avant patch

Statut : valide provisoirement.

Decision :

- ajouter `analyze-java` avant `patch` ;
- analyser Maven, `plugin.yml`, classes Java, commandes et listeners sans LLM ;
- utiliser cette analyse comme base pour les futurs prompts et outils de build.

Raison :

- OpticCode doit comprendre un projet Bukkit avant de le modifier ;
- une analyse deterministe est plus rapide et plus fiable qu'un appel modele ;
- cela reduira le contexte necessaire pour les futures generations.

### D-021 - Build controle avant correction

Statut : valide provisoirement.

Decision :

- ajouter une commande `build` avant la correction assistee ;
- lancer uniquement les commandes connues du type de projet detecte ;
- resumer les erreurs utiles plutot que transmettre tout le log brut ;
- garder la sortie complete en queue courte pour diagnostiquer si besoin.

Raison :

- un agent code doit verifier ses hypotheses par compilation ;
- Maven produit beaucoup de bruit, surtout sur Windows ;
- isoler les erreurs comme `cannot find symbol` permet de relier plus vite un probleme a une correction legacy, par exemple `Material.GUNPOWDER` vers `Material.SULPHUR`.

### D-022 - Les idees de recherche doivent etre triees avant implementation

Statut : valide.

Decision :

- conserver les notes brutes hors Git ;
- ajouter une synthese projet dans `docs/ideas-triage.md` ;
- classer les idees en maintenant, ensuite, plus tard, a eviter.

Raison :

- les recherches GPT/Qwen contiennent de bonnes directions et des pistes trop avancees ;
- la roadmap doit rester executable ;
- OpticCode doit viser l'excellence sans courir apres chaque optimisation annoncee.

### D-023 - Q4_K_M reste le modele principal avant benchmark Q5_K_M

Statut : valide provisoirement.

Decision :

- garder `qwen2.5-coder:14b` Q4_K_M comme modele principal ;
- tester Q5_K_M plus tard seulement avec un benchmark qualite/vitesse ;
- ne pas migrer avant d'avoir ameliore le contexte, les tools, le RAG et le patch workflow.

Raison :

- Q5_K_M peut ameliorer la qualite mais augmenter le cout memoire/latence ;
- les erreurs Bukkit legacy viennent surtout d'un manque de contexte specialise ;
- Q4_K_M est deja utilisable avec `keep_alive` et mode bref.

### D-024 - Ne pas patcher Qwen, GGUF ou llama.cpp pour la V1

Statut : valide.

Decision :

- ne pas modifier les poids Qwen ;
- ne pas patcher le GGUF ;
- ne pas ecrire de kernels custom ;
- ne pas fork llama.cpp pour la V1.

Raison :

- le meilleur ratio effort/gain est dans l'orchestration, la selection de contexte, le RAG et la verification ;
- les optimisations bas niveau demandent un protocole de benchmark solide ;
- un agent fiable vaut plus qu'un runtime exotique difficile a maintenir.

### D-025 - Profils dynamiques plutot que Modelfile geant

Statut : valide provisoirement.

Decision :

- garder Ollama/Modelfile pour le socle modele et quelques parametres ;
- gerer les profils, regles, RAG et verifications dans OpticCode ;
- commencer par `minecraft-java-1.8`.

Raison :

- les profils doivent fonctionner en CLI, futur daemon et futur IDE ;
- les regles doivent pouvoir changer selon le projet ;
- un Modelfile trop gros deviendrait rigide et difficile a benchmarker.

Etat :

- profil `minecraft-java-1.8` ajoute dans `skills/profiles` ;
- `ask` et `plan` chargent ce profil par defaut ;
- `--profile none` permet de desactiver l'injection.

### D-026 - Patch preview avant safe apply

Statut : valide provisoirement.

Decision :

- ajouter une commande `patch` qui produit un diff texte sans modifier les fichiers ;
- ajouter `patch --check` pour valider le diff avec `git apply --check` ;
- commencer par des corrections deterministes Java legacy ;
- repousser l'application automatique a une commande separee avec verification.

Raison :

- OpticCode doit rester controlable ;
- un patch visible est plus fiable qu'une modification directe ;
- le meme format servira plus tard aux patchs generes par LLM, a `git apply --check` et au futur safe apply.

### D-027 - Coherence Bukkit plugin.yml et getCommand

Statut : valide provisoirement.

Decision :

- extraire les appels `getCommand("...")` des fichiers Java ;
- comparer ces commandes avec celles declarees dans `plugin.yml` ;
- signaler les commandes declarees mais non enregistrees, et inversement.

Raison :

- beaucoup de bugs Bukkit viennent d'une commande oubliee dans `plugin.yml` ou mal enregistree ;
- cette verification est rapide et deterministe ;
- elle reduit le besoin d'appeler le modele pour un probleme structurel simple.

### D-028 - Memoire Markdown avant SQLite

Statut : valide provisoirement.

Decision :

- ajouter une memoire simple dans `skills/memory` ;
- charger une memoire globale et une memoire par profil dans `ask` et `plan` ;
- permettre `--no-memory` pour benchmarker sans memoire ;
- repousser SQLite a une phase ou les donnees seront plus nombreuses.

Raison :

- OpticCode doit apprendre son contexte avant d'entrainer un modele ;
- Markdown est lisible, versionnable et suffisant pour les premieres regles ;
- cela prepare la future memoire global/profile/project sans complexite prematuree.

### D-029 - Scanner les resource packs avant de les indexer

Statut : valide provisoirement.

Decision :

- ajouter une commande `pack-scan` read-only ;
- inventorier les packs externes sans les deplacer et sans les copier ;
- classer les fichiers par categories utiles : blockstates, models, textures, lang, CIT ;
- extraire une courte liste de chemins legacy suspects pour preparer le RAG.

Raison :

- les packs donnes par l'utilisateur sont des sources de contexte, pas des dependances a modifier ;
- un scan leger permet de savoir quoi indexer avant de creer une base RAG ;
- les images ne doivent pas etre injectees directement dans le prompt, mais leurs chemins et noms peuvent aider le contexte Minecraft 1.8.

### D-030 - Scanner les projets externes en lecture seule

Statut : valide provisoirement.

Decision :

- ajouter une commande `rag-scan` read-only ;
- ne jamais modifier les plugins avances ni le fork PandaSpigot pendant l'inventaire ;
- ignorer les dossiers de dependances et sorties de build : `libs`, `lib`, `target`, `build`, `bin`, `classes`, `out` ;
- compter les fichiers texte indexables et reperer les fichiers importants.

Raison :

- l'utilisateur n'a pas forcement de backup de ces projets ;
- le futur RAG doit apprendre des sources metier sans risquer de les alterer ;
- les dependances extraites et `.class` polluent les mesures et doivent rester hors index V1.

### D-031 - Index JSONL avant Tantivy/Qdrant

Statut : valide provisoirement.

Decision :

- ajouter `rag-index` qui ecrit `documents.jsonl` et `chunks.jsonl` dans `data/index` ;
- ajouter `rag-search` pour verifier la recherche locale sans embeddings ;
- garder les artefacts d'index hors Git via `.gitignore` ;
- retarder Tantivy, SQLite et Qdrant jusqu'a ce que le schema et les mesures soient plus clairs.

Raison :

- JSONL est simple, inspectable et suffisant pour valider les sources ;
- l'index doit d'abord prouver son utilite sur des requetes Minecraft legacy ;
- cela evite d'installer une brique lourde avant de connaitre le volume et les besoins reels.

### D-032 - RAG injecte sous limite stricte

Statut : valide provisoirement.

Decision :

- brancher le RAG local dans `ask` et `plan` ;
- utiliser `data/index` par defaut ;
- limiter les resultats avec `--rag-limit` ;
- permettre `--no-rag` pour comparer avec/sans RAG ;
- ne pas faire echouer `ask` ou `plan` si l'index n'existe pas encore.

Raison :

- le RAG doit ameliorer le contexte sans exploser les tokens ;
- les benchmarks doivent rester comparables ;
- OpticCode doit rester utilisable meme avant la construction d'un index local.

### D-033 - Comparer le RAG par prompts repetables

Statut : valide provisoirement.

Decision :

- ajouter `scripts/run-rag-comparison.ps1` ;
- executer chaque prompt avec RAG puis sans RAG ;
- stocker un resume Markdown et un detail JSONL dans `benchmarks/runs` ;
- garder les artefacts de benchmark hors Git.

Raison :

- le RAG doit etre juge sur le cout et la qualite, pas seulement sur sa presence ;
- les prompts legacy doivent couvrir les cas Minecraft reels : spawners, pelles/spades, nether wart/stalk, materials modernes ;
- les resultats montrent aussi quand la requete RAG est trop vague et doit etre enrichie.

### D-034 - Expansion de requete RAG legacy

Statut : valide provisoirement.

Decision :

- enrichir les requetes RAG avec des synonymes Minecraft legacy ;
- mapper `pelle/pelles` vers `shovel`, `spade`, `*_SPADE` ;
- mapper `spawner` vers `mob_spawner` et `MOB_SPAWNER` ;
- mapper `nether wart` vers `nether_stalk` et `NETHER_STALK` ;
- mapper `spawn egg` vers `spawn_egg` et `monster_placer` ;
- mapper `gunpowder` vers `SULPHUR`.

Raison :

- les demandes utilisateur peuvent etre en francais alors que les sources sont souvent en anglais ou en noms Bukkit ;
- la recherche stricte multi-mots reduit le bruit, mais elle a besoin de synonymes courts ;
- cette approche reste deterministe et mesurable avant d'ajouter embeddings ou Tantivy.

### D-035 - Prioriser les sources RAG lisibles avant le code interne

Statut : valide provisoirement.

Decision :

- trier les chunks injectes dans `ask` et `plan` par type de source ;
- privilegier `opticcode:docs/` puis `opticcode:skills/` ;
- garder les sources plugin et resource-pack utiles ;
- placer `opticcode:crates/` apres les documents metier.

Raison :

- le code interne d'OpticCode peut contenir les memes regles que la documentation, mais il est moins lisible pour le modele ;
- pour repondre a une question Minecraft legacy, une regle documentee est souvent plus utile qu'une implementation Rust ;
- le tri est applique au contexte RAG injecte, sans casser `rag-search` qui reste un outil de diagnostic brut.

### D-036 - Debug RAG sans appel modele

Statut : valide provisoirement.

Decision :

- ajouter la commande `rag-debug` ;
- ajouter l'option `--rag-debug` a `ask` et `plan` ;
- afficher les requetes elargies et les chunks injectes ;
- garder `rag-search` comme recherche brute d'index.

Raison :

- il faut comprendre ce que le modele recoit vraiment avant d'optimiser la qualite ;
- diagnostiquer le RAG sans appeler Qwen evite de perdre du temps et des tokens ;
- le debug facilite la correction des synonymes, du tri et des doublons.

### D-037 - Deduplication des regles RAG repetitives

Statut : valide provisoirement.

Decision :

- detecter quelques concepts legacy dans les previews RAG ;
- dedupliquer les repetitions `opticcode:docs`, `opticcode:skills` et `opticcode:crates` quand elles portent la meme regle ;
- conserver les sources plugin/resource-pack separees, car elles peuvent montrer un usage concret ;
- prioriser `docs/minecraft-legacy-rules.md` avant les autres documents OpticCode.

Raison :

- une meme regle peut exister dans la doc, le profil, la memoire et le code Rust ;
- le modele doit voir la regle la plus lisible, pas trois variantes internes ;
- les usages plugin restent utiles pour comprendre le contexte metier reel.

### D-038 - Filtrer les hits RAG faibles sans concept legacy

Statut : valide provisoirement.

Decision :

- ignorer les hits RAG avec un score tres faible quand aucun concept legacy connu n'est detecte dans la preview ;
- conserver les docs et skills meme avec un score faible ;
- conserver les hits faibles s'ils contiennent un concept legacy explicite.

Raison :

- certaines requetes elargies comme `DIAMOND_SPADE` peuvent matcher des configs contenant beaucoup de `DIAMOND_*` sans rapport avec Bukkit legacy ;
- un hit faible sans concept metier ajoute du bruit au prompt ;
- le filtre garde les exemples utiles plugin/resource-pack quand ils mentionnent vraiment une regle ou un nom legacy.

### D-039 - Afficher les requetes qui produisent chaque hit RAG

Statut : valide provisoirement.

Decision :

- conserver le `chunk_id` dans le contexte RAG debug ;
- conserver les requetes elargies qui ont produit chaque hit ;
- fusionner les requetes quand le meme chunk est retrouve plusieurs fois ;
- afficher `matched_queries` dans `rag-debug` et `--rag-debug`.

Raison :

- le score seul n'explique pas pourquoi un chunk a ete selectionne ;
- savoir si un hit vient de `spade`, `MOB_SPAWNER` ou `nether_stalk` aide a corriger les synonymes ;
- cela rend le RAG auditable sans appel modele supplementaire.

### D-040 - Score detaille par requete RAG elargie

Statut : valide provisoirement.

Decision :

- conserver un score par requete elargie pour chaque chunk RAG ;
- fusionner les scores quand le meme chunk est retrouve par plusieurs synonymes ;
- afficher `query_scores` dans `rag-debug` et `--rag-debug`.

Raison :

- `matched_queries` dit quelles requetes ont match, mais pas lesquelles portent vraiment le resultat ;
- un score detaille permet de voir rapidement si un hit est principalement porte par `spawner`, `nether wart`, `shovel`, etc. ;
- cela prepare une future ponderation plus fine sans embeddings.

### D-041 - Score RAG pondere par requete elargie

Statut : valide provisoirement.

Decision :

- ajouter un `weighted_score` par hit RAG ;
- ponderer les requetes legacy precises plus fortement que les synonymes generiques ;
- trier les hits d'une meme priorite de source par `weighted_score`, puis par score brut ;
- afficher les poids dans `query_scores` avec la forme `requete=scorexpoids`.

Raison :

- `shovel` est utile pour retrouver les fichiers de langue, mais moins fiable qu'un identifiant 1.8 comme `MOB_SPAWNER`, `NETHER_STALK` ou `Material.SULPHUR` ;
- le score brut peut favoriser un terme generique tres frequent ;
- la ponderation est deterministe, peu couteuse et evite d'ajouter un moteur vectoriel trop tot ;
- l'optimisation vise surtout la qualite du contexte injecte, donc le cout en tokens utiles, pas le debit brut du modele.
