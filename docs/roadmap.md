# OpticCode - Roadmap

Etat audite et roadmap consolidee au 2026-07-11 :
[`project-audit-2026-07-11.md`](project-audit-2026-07-11.md).

Ce document conserve l'historique detaille des phases. L'audit consolide fait
foi pour les priorites et les criteres de sortie actuels.

Derniere mise a jour : 2026-08-03

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
- `docs/ideas-triage.md`

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
- commande `build` fonctionnelle pour Maven/Gradle ;
- commande `patch` fonctionnelle en preview deterministe ;
- verification `patch --check` fonctionnelle via `git apply --check` ;
- profil `minecraft-java-1.8` minimal ajoute ;
- coherence `plugin.yml` / `getCommand(...)` detectee ;
- memoire Markdown simple ajoutee ;
- benchmark JSONL ajoute dans `scripts/run-mini-benchmark.ps1` ;
- commande `pack-scan` ajoutee pour inventorier les resource packs externes sans les copier ;
- commande `rag-scan` ajoutee pour inventorier les plugins avances et PandaSpigot en lecture seule ;
- commandes `rag-index` et `rag-search` ajoutees pour un premier index JSONL local ;
- RAG branche dans `ask` et `plan` avec `--no-rag`, `--rag-index`, `--rag-limit` ;
- script `run-rag-comparison.ps1` ajoute pour comparer avec/sans RAG ;
- expansion de requete RAG ajoutee pour les synonymes legacy francais/anglais ;
- tri RAG ajoute pour privilegier `docs/` et `skills/` avant le code interne ;
- debug RAG ajoute pour afficher les requetes elargies et chunks injectes ;
- deduplication RAG ajoutee pour reduire les repetitions internes OpticCode ;
- filtre anti-bruit RAG ajoute pour ignorer les hits faibles sans concept legacy ;
- `matched_queries` ajoute au debug RAG pour expliquer chaque hit ;
- `query_scores` ajoute au debug RAG pour voir le score par requete elargie ;
- `weighted_score` ajoute pour favoriser les correspondances legacy precises par rapport aux synonymes generiques ;
- script `run-rag-quality.ps1` ajoute pour mesurer la qualite legacy avec/sans RAG ;
- regle spawn egg ajoutee : `Material.MONSTER_EGG` et `item.monsterPlacer` ;
- script `run-patch-build-quality.ps1` ajoute pour verifier build echec -> patch -> build OK sur copie temporaire ;
- compilation et tests OK.

Livrable :

- `docs/phase-4-mvp.md`
- `docs/mini-bukkit-benchmark.md`
- `docs/optimization-notes.md`
- `docs/java-project-intelligence.md`
- `docs/resource-pack-scan.md`
- `docs/rag-source-inventory.md`
- `docs/rag-index.md`
- `docs/rag-quality.md`
- `docs/patch-build-quality.md`
- `docs/safe-apply-roadmap.md`
- `docs/real-plugin-kspawners-test.md`

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
- `build` lance Maven/Gradle de facon controlee ;
- `build` resume les erreurs de compilation utiles ;
- test negatif `Material.GUNPOWDER` valide avec suggestion `Material.SULPHUR` ;
- `keep_alive=15m` reduit fortement le cout de modele froid ;
- `patch` propose un diff non applique pour `Material.GUNPOWDER` -> `Material.SULPHUR` ;
- `patch --check` verifie que le diff est applicable ;
- `run-patch-build-quality.ps1` valide plusieurs corrections legacy avec rebuild Maven OK ;
- roadmap `safe apply` ajoutee avant implementation de l'application reelle ;
- `apply --dry-run` ajoute pour verifier un plan d'application sans modifier de fichiers ;
- `apply --copy-to <path> --yes` ajoute pour appliquer uniquement dans une copie temporaire ;
- `apply --path <dossier> --yes` ajoute pour appliquer reellement dans le workspace courant uniquement ;
- journal `.opticcode/apply-log.jsonl` et patch rollback `.opticcode/runs/<run-id>/patch.diff` ajoutes apres apply reussi ;
- `apply --undo <run-id> --yes` ajoute pour annuler un apply depuis le patch sauvegarde ;
- `--allow-external` ajoute pour autoriser explicitement un apply hors workspace courant ; Git propre reste le defaut et `--allow-dirty` l'exception ;
- test sur copie reelle `Kspawners` effectue : analyse OK, build OK, patch `plugin.yml` OK, apply/undo OK ;
- preservation LF/CRLF ajoutee apres apply et undo ;
- Build Git State Guard ajoute : snapshots Git avant/apres, JSON et mode strict ;
- validation du guard terminee sur fixture Git et copie Kspawners ;
- BLAKE3, metriques snapshot, commande `git-state` et test CLI Rust ajoutes ;
- benchmark read-only : petite fixture, Kspawners et PandaSpigot ;
- `ask` et `plan` peuvent charger le profil `minecraft-java-1.8` ;
- `analyze-java` compare les commandes declarees avec `getCommand(...)` ;
- table legacy initiale ajoutee : gunpowder, nether wart, spawners, pelles/spades et quelques mobs ;
- `ask` et `plan` chargent une memoire global/profil par defaut ;
- `run-mini-benchmark.ps1` append des runs JSONL comparables ;
- process runner borne ajoute : timeout, cancellation, capture limitee et Job Object Windows ;
- test CLI d'un faux Maven bloque et test du PID descendant Windows valides ;
- apply transactionnel ajoute : journal prepare, backups BLAKE3, etats append-only,
  rollback automatique et recovery explicite ;
- concurrence optimiste ajoutee : verification before/after et refus de derive externe ;
- verrou OS workspace et refus des symlinks/jonctions sur cibles et journaux ;
- tests CLI reels valident apply, inspect, list, undo, repo sale, rollback_failed et recovery ;
- verification GIT-002 ajoutee : worktree detache, apply, build strict, diff et cleanup ;
- registre de leases et cleanup manuel fail-closed ajoutes pour les interruptions ;
- validation GIT-002 : 105 tests workspace et Clippy strict OK ;
- baseline Tree-sitter Java read-only ajoutee avec tests anti-faux-positifs ;
- index symbolique inter-fichiers B1 ajoute avec resolution conservatrice ;
- propositions d'edits Java B2 ajoutees avec hash, ranges, octets attendus,
  anti-shadow, simulation et reparse ;
- corpus LEGACY-002 de plus de 70 cas : 28/28 edits attendus et zero faux positif ;
- validation complete : 132 tests workspace, Clippy strict et build release OK ;
- pipeline B3 ajoute : double contrat, apply transactionnel, reparse, build,
  hashes Git finaux, cleanup et source inchangee ;
- LEGACY-002 : 26 regles, provenance SHA-256 et compilation Spigot 1.8.8 ;
- CONTEXT-001 : selection symbolique bornee et benchmark -70,87 % tokens estimes ;
- RAG-SAFE-001 : index v2 securise et publication atomique ;
- EVAL-001 : corpus de 45 cas, metriques testees et rapports reproductibles ;
- CONTEXT-002 : modes ask/plan explicites, fail-closed et A/B Qwen reel termine.

## Phase 5.1 - Safe Apply

Statut : terminee pour le scope legacy deterministe ; extension agent generale non commencee.

Objectif :

- appliquer des patchs seulement apres verification et confirmation explicite ;
- commencer par `apply --dry-run` ;
- tester l'application sur copie temporaire ;
- autoriser une premiere application reelle uniquement dans le workspace courant ;
- journaliser chaque application reussie ;
- annuler une application avec `apply --undo <run-id> --yes` ;
- autoriser un projet externe seulement avec `--allow-external`, Git propre par defaut et `--allow-dirty` explicite ;
- refuser toute modification silencieuse ;
- preparer rollback simple.

Ordre :

1. `apply --dry-run` sans modification. Fait.
2. Application sur copie temporaire. Fait.
3. Application reelle avec `--yes` dans le workspace courant. Fait.
4. Journal + rollback manuel. Fait.
5. Commande `apply --undo <run-id>`. Fait.
6. Regles d'elargissement hors workspace courant. Fait via `--allow-external`.
7. Verification build optionnelle. Fait via Build Git State Guard.
8. Test sur copie locale d'un vrai plugin. Fait avec Kspawners.
9. Preservation LF/CRLF avant originaux. Fait.
10. Isolation du bruit de build Maven. Fait.
11. Process runner borne avec timeout/cancellation. Fait.
12. Journal apply transactionnel et rollback automatique sur erreur de finalisation. Fait.

Livrable :

- `docs/safe-apply-roadmap.md`
- `docs/real-plugin-kspawners-test.md`
- `docs/build-git-state-guard.md`
- `docs/process-runner.md`
- `docs/apply-transaction.md`
- `docs/optimization-backlog.md`

## Phase 5.2 - Process runner borne

Statut : termine et valide sur Windows 10.

Acquis :

- timeout de build configurable, 600 secondes par defaut ;
- limite de sortie configurable, 1 Mio par flux par defaut et 16 Mio maximum ;
- drainage concurrent de `stdout` et `stderr` ;
- statuts structures `success`, `failed`, `timed_out`, `cancelled` ;
- token d'annulation utilisable par le futur core agent ;
- Job Object Windows et terminaison des descendants ;
- JSON CLI enrichi sans suppression des champs existants ;
- tests processus court, erreur, sortie massive, blocage, cancellation et arbre enfant.

Limites conservees :

- le fallback non-Windows ne gere que le processus racine ;
- les commandes Git historiques restent hors runner ;
- aucun shell arbitraire n'est expose.

## Phase 5.3 - Apply transactionnel

Statut : termine et valide sur depots temporaires Windows.

Acquis :

- patch, backups, manifeste et `prepared` durables avant toute mutation ;
- create/modify/delete avec sauvegardes brutes et BLAKE3 ;
- remplacements atomiques par fichier et verification apres ecriture ;
- huit etats explicites et transitions impossibles refusees ;
- rollback automatique, rollback partiel explicite et recovery idempotente ;
- repo Git propre obligatoire par defaut, `--allow-dirty` explicite ;
- protection des changements preexistants et des derives posterieures ;
- listing/inspection read-only des transactions incompletes ;
- JSON stable et codes de sortie transactionnels ;
- injections de panne deterministes et tests CLI du binaire reel.

Limites :

- pas d'atomicite globale multi-fichiers ;
- le verrou ne peut pas empecher un editeur externe de modifier un fichier ;
- pas de build inclus dans la transaction ;
- pas de recovery destructive automatique.

Reference : [`apply-transaction.md`](apply-transaction.md).

## Phase 5.4 - Worktree jetable de verification

Statut : termine et valide sur depots temporaires Windows.

Acquis :

- source Git propre et commit exact verifies avant creation ;
- worktree detache sous un stockage temporaire controle ;
- compatibilite des chemins Windows `\\?\` avec Git for Windows ;
- apply transactionnel puis build strict avec timeout/cancellation ;
- rapport JSON avec snapshots, diff, apply, build et cleanup ;
- preuve commit/etat Git source avant/apres ;
- detached HEAD, refs utilisateur et Git State Guard source verifies ;
- cleanup uniquement du worktree enregistre par OpticCode ;
- leases listables et nettoyables apres interruption ;
- resultat verification separe du cleanup et recovery idempotente ;
- codes distincts pour verification, cleanup, precondition et run id ;
- tests succes, build echoue, timeout, repo sale, traversal et recovery vide.

Limites :

- adaptateur public encore limite au patch legacy deterministe ;
- aucun transfert automatique vers le projet source ;
- recovery fail-closed si un dossier non enregistre contient des donnees.

Reference : [`worktree-verification.md`](worktree-verification.md).

## Phase 5.5 - Tree-sitter Java et contexte symbolique

Statut : baseline, index B1, edits B2/B3, LEGACY-002 et CONTEXT-001 termines.

Acquis :

- module `java_syntax` separe de worktree/apply ;
- Tree-sitter Java initialise et reutilise entre fichiers ;
- scan projet borne, deterministe et sans suivi de symlink ;
- package, imports, classes, interfaces, enums et annotations-types ;
- methodes, constructeurs, champs, constantes et parametres ;
- appels, acces de champs/enums, constructions, method references et annotations ;
- positions exactes et diagnostics de Java partiellement invalide ;
- commentaires, chaines et caracteres exclus des references code ;
- ranges UTF-8/CRLF testes comme offsets d'octets ;
- limites et troncatures explicites dans le schema JSON ;
- symlinks, jonctions et reparse points ignores ou refuses selon leur position ;
- commande `java-syntax` humaine/JSON ;
- commande `java-index` humaine/JSON et schema versionne ;
- identifiants stables pour types, overloads, champs et constantes ;
- resolution locale, meme package, imports, `java.lang` et noms qualifies ;
- statuts `exact`, `unique_candidate`, `ambiguous`, `unresolved` et
  `invalid_syntax_context` ;
- listes de candidats bornees sans choix arbitraire ;
- commande `java-edits` humaine/JSON et schema versionne ;
- propositions avec cible exacte, hash, noeud/octet attendu et ranges AST ;
- refus des shadows connus, mauvaises cibles, overlaps et derives source ;
- application simulee de fin vers debut et reparse en memoire ;
- corpus legacy de plus de 70 cas avec 26/26 regles et zero faux positif ;
- regression reelle Kspawners `SpawnReason.SPAWNER` correctement refusee ;
- tests read-only sur mini Bukkit, Kspawners et PandaSpigot borne ;
- commande `java-edits-verify` humaine/JSON et schema versionne ;
- reanalyse B2 dans un worktree detache au `HEAD` exact ;
- empreinte source/worktree, rematerialisation et apply transactionnel ;
- reparse apres ecriture, build borne et Git State Guard strict ;
- hashes finaux verifies, diff borne, cleanup/recovery et source inchangee ;
- aucun transfert automatique vers le projet original.
- catalogue legacy etendu a 26 regles prouvees et compilees contre Spigot 1.8.8 ;
- commande `java-context` humaine/JSON, read-only et guidee par la demande ;
- overloads, ambiguites, declarations, appelants et relations exactes expliques ;
- fichiers `pom.xml`/`plugin.yml` ajoutes uniquement quand la demande les vise ;
- budgets separes pour fichiers, symboles, relations, profondeur, snippets,
  octets, caracteres, tokens, diagnostics et avertissements ;
- analyse et selection completes distinguees, troncatures et ignores exposes ;
- sous-parseur Clap isole pour eviter le stack overflow debug Windows ;
- tests reels CLI, jonction, 400 fichiers, cycles, Unicode/CRLF et Java invalide ;
- benchmark : 4 140 -> 1 206 tokens estimes (-70,87 %) sur cinq demandes.
- validation complete : 162 tests workspace, Clippy strict et build release OK.

Limites :

- resolution volontairement inferieure a `javac` : classpath, heritage,
  generiques et types runtime non couverts ;
- pas de cache incremental ou persistant ;
- `analyze-java` conserve temporairement son parseur textuel historique.
- estimation de tokens non specifique au tokenizer Qwen ;
- le mode symbolique reste volontairement optionnel dans `ask` et `plan`.

Suite : LLM/PROTOCOL-001 est termine. Ajouter POLICY-001 avant toute boucle
agent ; ne promouvoir `symbol` qu'apres davantage de repetitions et de revues
humaines.

References : [`java-syntax.md`](java-syntax.md), [`java-index.md`](java-index.md),
[`java-edits.md`](java-edits.md) et
[`java-edit-worktree.md`](java-edit-worktree.md), ainsi que
[`java-legacy-rules.md`](java-legacy-rules.md) et
[`java-context.md`](java-context.md).

## Phase 5.6 - Evaluation reproductible

Statut : EVAL-001 et son integration CONTEXT-002 termines.

Acquis :

- schemas Serde versionnes pour suite, cas, configuration, observations et baseline ;
- corpus de 45 cas equilibres en cinq categories Java/Bukkit/legacy ;
- fixtures versionnees uniquement, PandaSpigot et Kspawners externes optionnels ;
- strategies `legacy`, `symbol`, `exact` et RAG securise v2 ;
- Hit@1/3/5, Recall@k, MRR, NDCG@5, doublons, diversite et p50/p95 ;
- couts contexte separes des vrais tokens provider ;
- rapports JSON et Markdown, comparaison et regressions ;
- fingerprint avant/apres garantissant les runs read-only ;
- latences RAG mesurees requete par requete, sans les masquer par un batch global ;
- CLI JSON pure et tests espaces/Unicode, corpus invalide, skip et troncature.

Acquis CONTEXT-002 :

- modes `legacy`, `symbol`, `compare` dans `ask` et `plan` ;
- fallback/refus explicite et comparaison sans double generation implicite ;
- vrais tokens, timings et debit Ollama dans les rapports ;
- A/B chaud sur trois cas : 1 902 -> 1 188 tokens prompt (-37,5 %) ;
- Recall@k 0,833 -> 1,000, mais qualite 0,556 -> 0,333 ;
- decision conservee : `legacy` reste le defaut.

Suite : `LLM/PROTOCOL-001` et `VSCODE-001` sont termines ; `POLICY-001` devient prioritaire.
RAG-002/Tantivy reste un prototype mesure distinct, sans embeddings dans sa
premiere iteration.

Reference : [`evaluation.md`](evaluation.md).

## Phase 5.7 - Profils, memoire et optimisation controlee

Statut : cadree, avec LLM/PROTOCOL-001 termine.

Objectif :

- transformer les idees de recherche en plan exploitable ;
- eviter les optimisations prematures ;
- preparer les profils specialises sans ralentir le MVP.

Priorites :

1. Streaming, abstraction provider, annulation et protocole LLM. Fait.
2. Politique deny-by-default avant agent ecrivant.
3. Contrats d'outils structures apres la politique.
4. Packs RAG lies aux profils.
5. Feedback accepted/rejected.
6. Benchmark Q4/Q5 plus tard.

Decisions :

- Q4_K_M reste le modele principal tant que le workflow agent/RAG n'est pas stabilise ;
- Q5_K_M sera teste plus tard avec benchmark qualite/vitesse ;
- llama.cpp direct reste une piste de benchmark, pas une dependance V1 ;
- pas de fine-tuning/LoRA avant collecte de patches acceptes/refuses.

Livrable :

- `docs/ideas-triage.md`
- `docs/profiles.md`
- `docs/memory.md`
- `docs/llm-protocol.md`

## Phase 5.8 - Protocole client et extension VS Code

Statut : `LLM/PROTOCOL-001` et `VSCODE-001` termines.

Acquis protocole :

- Ollama isole derriere `LlmProvider`, contrats versionnes et streaming NDJSON ;
- annulation cooperative Windows, timeout et terminal machine non ambigu ;
- commandes read-only `version --json`, `capabilities --json` et `doctor --json` ;
- resume assistant terminal avec contexte, tokens, timings et avertissements.

Acquis extension :

- client TypeScript strict et borne, sans shell ni logique metier dupliquee ;
- 13 commandes publiques, trois TreeViews natives, diagnostics et CodeAction ;
- Ask/Plan streames avec progression, metriques et annulation confirmee ;
- verification explicite en worktree, sans apply sur le projet original ;
- 20 tests unitaires, 3 integrations CLI reelles, smoke Qwen et Extension Host ;
- VSIX minimal versionne `0.1.0`, sans binaire, dependance ou donnee privee.

Limites :

- pas de LSP, daemon, boucle agent ou transfert automatique vers la source ;
- rapports et runs limites a la session VS Code ;
- `legacy` reste le contexte par defaut ; `symbol` et `compare` sont explicites.

Suite : `POLICY-001` devient le prochain jalon. Il doit definir une politique
deny-by-default avant toute boucle capable d'appeler des outils d'ecriture.

References : [`llm-protocol.md`](llm-protocol.md),
[`client-discovery.md`](client-discovery.md) et
[`vscode-extension.md`](vscode-extension.md).

## Phase 6 - RAG local et donnees metier

Statut : RAG-SAFE-001 termine ; migration scalable soumise au benchmark Tantivy.

Acquis RAG-SAFE-001 :

- ingestion fail-closed par racines explicitement autorisees ;
- allowlist de formats et denylist globale de secrets/caches/donnees privees ;
- detection bornee de cles, tokens, credentials et URI sans journaliser les valeurs ;
- refus des fichiers sans extension, symlinks, jonctions et reparse points ;
- verification de coherence avant/apres lecture et hashes BLAKE3 ;
- provenance portable sans racine absolue dans les records ;
- manifeste schema v2, generations immuables et pointeur `CURRENT` atomique ;
- recovery des staging tronques sans toucher l'ancienne generation active ;
- erreurs explicites pour les index legacy ;
- sorties JSON pour `rag-scan`, `rag-index`, `rag-search` et `rag-debug` ;
- recherche multi-requetes en une passe pour eviter de revalider chaque expansion ;
- tests de panne, integration CLI et benchmark `run-rag-safe-quality.ps1` ;
- validation globale : 172 tests workspace, Clippy strict et build release OK.

Objectif :

- indexer docs, conventions, exemples de plugins, mappings legacy ;
- indexer les regles legacy et les inventaires de resource packs utiles ;
- indexer les inventaires des plugins avances et du fork PandaSpigot ;
- garder les sources externes a leur emplacement d'origine ;
- melanger recherche texte, symboles et embeddings ;
- garder une base de connaissances reutilisable ;
- mesurer le cout en prompt, latence et qualite de reponse.

Approche recommandee :

1. Commencer par un index JSONL local et deterministe.
2. Extraire les fichiers `.md`, `.txt`, `.lang`, `.properties`, `.json` utiles.
3. Garder les images uniquement comme references de chemins et metadata.
4. Ameliorer la recherche locale avant d'ajouter des embeddings.
5. Ajouter SQLite pour metadata et memoire quand les schemas sont stables.
6. Ajouter Tantivy pour recherche full-text.
7. Ajouter Tree-sitter pour symboles/classes/methodes.
8. Ajouter Qdrant seulement quand les embeddings sont valides.

EVAL-001 a mesure Hit@5 0,000 et une p95 proche de 302 ms pour les requetes
naturelles du RAG lexical v2. RAG-002 doit donc prototyper Tantivy face au JSONL
sur les memes cas, avec manifest incremental et cache. SQLite, embeddings et
Qdrant ne sont pas encore des dependances justifiees.

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
