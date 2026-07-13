# OpticCode - Backlog canonique d'optimisation

Derniere mise a jour : 2026-07-11

Statut : source de verite pour les optimisations et idees futures.

## Objectif

Ce document consolide :

- l'audit complet OpticCode ;
- les mesures Ollama/RAG/build ;
- la revue du Build Git State Guard ;
- `docs/ideas-triage.md` ;
- les deux brouillons locaux de `Idées-Vrac/` ;
- les recherches sur une future integration VS Code/`opticd`.

Les brouillons restent intacts et ignores par Git. Ils contiennent des idees
utiles, mais aussi des affirmations contradictoires, des chiffres non verifies
et des recommandations CUDA peu pertinentes pour la machine AMD actuelle.

Regle : une idee n'entre dans la roadmap active que si elle possede :

1. un probleme observe ;
2. une mesure de depart ;
3. un livrable borne ;
4. un test de non-regression ;
5. un critere d'arret.

## Principes stables

1. Optimiser le travail autour du modele avant de modifier le runtime.
2. Reduire le contexte inutile avant d'augmenter `num_ctx`.
3. Utiliser les tools deterministes avant un appel LLM.
4. Privilegier recherche exacte et symbolique pour le code legacy.
5. Garder les ecritures explicites, journalisees et reversibles.
6. Mesurer temps, tokens, qualite et effets de bord.
7. Garder C++ limite au backend d'inference tant qu'un profil ne prouve pas un autre goulot.

## Mesures deja etablies

| Sujet | Mesure | Conclusion |
| --- | --- | --- |
| Qwen chaud | environ 26,5 tokens/s | la longueur de sortie domine |
| Mode bref | environ 4x plus rapide sur le cas mesure | optimisation validee |
| Chargement froid | jusqu'a environ 70 s | `keep_alive` est indispensable |
| RAG legacy | 100 % vs 80 % sur cinq cas | gain qualitatif, corpus encore petit |
| Patch/build | build rouge puis vert | tool deterministe utile |
| Snapshot petite fixture | 49,668 ms moyen, 5 runs | cout Git fixe dominant |
| Snapshot Kspawners | 63,462 ms moyen, 5 runs | 61 fichiers, 896 222 octets hashes |
| Snapshot PandaSpigot | 166,312 ms moyen, 5 runs | 10 fichiers, 186 384 octets hashes |

Le benchmark snapshot est stocke localement sous :

```text
benchmarks/runs/git-snapshot-benchmark-20260711-054601/
```

Conclusion : BLAKE3 n'est pas le goulot actuel. Sur PandaSpigot, la majeure
partie du temps vient de `git status` sur le grand worktree, pas des dix fichiers
empreintes.

## Termine et a conserver

| ID | Capacite | Etat |
| --- | --- | --- |
| DONE-001 | metriques Ollama et export JSONL | termine |
| DONE-002 | `keep_alive=15m` | termine |
| DONE-003 | mode bref et limite de generation | termine |
| DONE-004 | contexte projet borne | termine |
| DONE-005 | profil et memoire Minecraft Java 1.8 | termine |
| DONE-006 | RAG JSONL lexical avec debug/ponderation | prototype valide |
| DONE-007 | analyse Java/Bukkit deterministe | prototype valide |
| DONE-008 | patch/check/apply/undo sur copies | termine pour scope legacy |
| DONE-009 | Build Git State Guard | termine |
| DONE-010 | BLAKE3, metriques snapshot et test CLI Rust | termine |
| DONE-011 | process runner borne, timeout/cancellation et Job Object Windows | termine |
| DONE-012 | apply transactionnel, rollback et recovery explicite | termine |
| DONE-013 | concurrence optimiste before/after et refus de derive | termine pour le refus |

## P0 - Securite avant agent

### PROC-001 - Process runner borne

Statut : termine et valide le 2026-07-11.

Probleme : Maven/Gradle et les futurs tools peuvent rester bloques.

Livrable :

- runner de processus commun ;
- timeout configurable avec valeur par defaut prudente ;
- capture stdout/stderr sans deadlock ;
- terminaison de l'arbre de processus sous Windows ;
- statut structure `success`, `failed`, `timed_out`, `cancelled` ;
- tests avec processus court, en erreur et bloque ;
- aucune commande shell arbitraire ajoutee.

Critere de sortie : un build simule bloque est termine avec un rapport JSON et
aucun processus enfant restant.

Validation : test Rust du PID descendant Windows, test CLI avec faux Maven
bloque, capture bornee, cancellation distincte et non-regression du guard Git.
Voir [`process-runner.md`](process-runner.md).

### APPLY-001 - Apply transactionnel

Statut : termine et valide le 2026-07-11.

Probleme resolu : le journal final etait auparavant ecrit apres application.

Livrable realise :

- patch et journal `prepared` ecrits avant modification ;
- transitions versionnees jusqu'a `committed` ou rollback ;
- ecritures atomiques par fichier temporaire + rename ;
- rollback automatique si la finalisation echoue ;
- etat `rollback_failed` explicite ;
- journal append-only des apply, undo et recovery ;
- backups bruts, tailles, permissions et BLAKE3 ;
- injection de dix points de panne dans les tests ;
- commandes `transactions` de liste, inspection et recovery ;
- verrou OS workspace sans verrou orphelin apres crash ;
- refus des symlinks/jonctions et double revalidation TOCTOU ;
- JSON versionne et codes de sortie 0/2/3/4/5 ;
- aucune application originale pendant le developpement.

Critere de sortie : chaque panne simulee laisse soit l'original intact, soit un
etat journalise et recuperable sans ambiguite.

Critere atteint. Voir [`apply-transaction.md`](apply-transaction.md).

### APPLY-002 - Concurrence optimiste

Statut : termine pour le refus de conflit ; 3-way merge volontairement differe.

Probleme : un fichier peut changer entre preview et apply.

Livrable :

- empreinte ou blob Git attendu dans le plan ;
- re-check immediat avant apply ;
- refus si le contenu a change ;
- plus tard, rebase ou 3-way explicite, jamais silencieux.

Implementation : les octets et BLAKE3 avant/apres sont enregistres, chaque cible
est revalidee juste avant ecriture et le rollback refuse toute derive inconnue.

### GIT-002 - Worktree jetable

Statut : termine et valide le 2026-07-13.

Objectif atteint : verifier les patchs multi-fichiers sans toucher au worktree courant.

Livrable realise :

- creation sous un dossier temporaire controle ;
- build/test dans le worktree ;
- rapport final JSON avec snapshots, apply, build et diff ;
- nettoyage uniquement du worktree cree et apres validation de chemin ;
- lease listable et recovery manuelle fail-closed ;
- aucun reset/clean du projet utilisateur.

Validation : succes, build echoue, timeout, repo sale, traversal, cleanup et
source inchangee testes sur de vrais depots temporaires. Voir
[`worktree-verification.md`](worktree-verification.md).

## P1 - Intelligence code et contexte

### CODE-001 - Tree-sitter Java

Statut : pipeline read-only B1/B2, verification B3 et LEGACY-002 termines le 2026-07-13.

Premier scope :

- classes, interfaces, enums ;
- methodes et constructeurs ;
- imports, package et annotations ;
- positions exactes ;
- exclusion commentaires/chaines pour les remplacements legacy ;
- Java incomplet ou partiellement invalide.

Realise :

- dependances `tree-sitter` et `tree-sitter-java` figees ;
- module `java_syntax` separe ;
- symboles, references, zones non-code, diagnostics et positions JSON ;
- limites fichiers/taille/items/avertissements et troncatures JSON explicites ;
- ranges octets verifies avec UTF-8 et CRLF ;
- symlinks, jonctions et reparse points refuses ou ignores sans parcours ;
- tests anti-faux-positifs commentaires/chaines/text blocks ;
- mesures read-only mini Bukkit, Kspawners et PandaSpigot borne.

Realise pour `CODE-001B1` read-only :

- index symbolique inter-fichiers en memoire avec hashes source ;
- identites stables pour classes imbriquees, overloads et membres ;
- resolution bornee des imports explicites, wildcard et statiques ;
- statuts d'incertitude, raisons, candidats et CLI JSON versionnee ;
- baseline de performance jusqu'a 5 000 fichiers PandaSpigot.

Reste apres B1 :

- API de requetes de symboles utile a `CONTEXT-001` ;
- index incremental/persistant par hash pour depasser 5 000 fichiers ;
- classpath/JAR uniquement si les mesures qualite le justifient.

Realise pour `CODE-001B2` :

- edits legacy sur ranges AST avec hash et octets attendus ;
- refus des ranges invalides ou chevauchantes et reparse du resultat ;
- refus des mauvaises cibles, resolutions incertaines et shadows connus ;
- sortie read-only compacte, corpus qualite et vraie regression Kspawners.

Realise pour `CODE-001B3` :

- recalculer le contrat B2 dans le worktree et comparer son empreinte ;
- revalider les preconditions juste avant ecriture ;
- convertir les propositions en mutations APPLY-001 uniquement dans GIT-002 ;
- reparse avant/apres ecriture, build borne, hashes Git et diff final ;
- conserver la promotion vers la source hors scope.

Ne pas commencer par Rust/C++/JavaScript. Java couvre le besoin produit direct.

### CONTEXT-001 - Selection selon la tache

Retenu : oui, tres gros gain probable.

Pipeline cible :

```text
demande
-> identifiants et intentions
-> recherche exacte/symbolique
-> fichiers candidats expliques
-> extraits AST bornes
-> contexte final mesure
```

Metriques : tokens/characters, fichiers candidats, fichiers injectes, temps de
selection, qualite du plan et du patch.

### LEGACY-002 - Extension mesuree des regles

Statut : termine le 2026-07-13.

- 26 regles Bukkit 1.8 dans un catalogue JSON V2 ;
- 23 paires moderne/legacy confirmees dans deux JAR sources Spigot epingles ;
- trois alias historiques conserves au niveau de preuve inferieur explicite ;
- 28 propositions attendues, 26/26 regles et zero faux positif sur corpus ;
- compilation Java 8 reelle des 24 cibles distinctes contre Spigot 1.8.8 ;
- resolution des membres homonymes par proprietaire symbolique complet.

### INDEX-001 - Metadata incrementales SQLite

Retenu : oui.

Stocker : chemin, hash BLAKE3, langage, taille, mtime, symboles, version de
schema et derniere indexation. SQLite sert aux metadata et a la memoire ; il ne
doit pas devenir automatiquement le moteur unique de recherche.

### INDEX-002 - Tantivy lexical/BM25

Retenu : oui, apres schema et evaluation du JSONL.

But : remplacer les parcours complets de `chunks.jsonl`, garder les recherches
d'identifiants exacts et mesurer le rappel sur les cas legacy.

Question encore ouverte : Tantivy seul ou Tantivy + SQLite FTS5. Ne pas
installer les deux avant un prototype mesure.

### INDEX-003 - Embeddings

Retenu sous condition.

Ajouter uniquement pour les requetes semantiques que lexical + symboles ne
retrouvent pas. Les embeddings ne doivent pas remplacer les noms exacts Bukkit,
NMS ou Java.

Declenchement : corpus d'evaluation montrant des echecs repetes.

### CACHE-001 - Cache par hash

Retenu par etapes :

1. hash -> AST/symboles ;
2. hash -> resume technique ;
3. requete normalisee -> candidats ;
4. embeddings par chunk si INDEX-003 est justifie.

Le cache de reponse LLM exacte est repousse : il peut renvoyer une reponse
obsolete si le projet, le profil ou le modele change.

## P1 - Fondations agent

### AGENT-001 - Tools structures

- entrees/sorties Serde ;
- schema versionne ;
- categories read/write/build/process ;
- politique deny-by-default ;
- budget temps/tokens/fichiers ;
- journal d'execution.

### AGENT-002 - Boucle bornee

```text
plan -> read/search -> patch -> check -> confirmation
-> worktree/copie -> build/test -> diagnostic -> iteration bornee
```

Pas de shell arbitraire en V1. Maximum d'iterations et budget obligatoires.

### MEMORY-001 - Feedback accepte/refuse

Retenu apres des patchs generes par LLM :

- identifiant de patch ;
- accepte/refuse ;
- raison ;
- build/test avant/apres ;
- profil et modele ;
- aucune auto-regle sans validation humaine.

`learn` et `remember` restent des commandes futures. Elles ne doivent pas
transformer automatiquement du code existant en regles fiables.

## P2 - LLM et experience interactive

### LLM-001 - Streaming

Retenu : oui.

Gain : temps percu jusqu'au premier token, pas tokens/s. Garder `stream=false`
pour les benchmarks comparables et ajouter le streaming pour usage interactif.

### LLM-002 - Trait provider

Retenu avant llama.cpp :

- Ollama ;
- provider OpenAI-compatible ;
- metriques communes ;
- streaming/cancellation ;
- model info et token count quand disponible.

### PROMPT-001 - Builder compose et deduplique

Retenu : oui.

Les regles sont aujourd'hui reparties entre prompt, profil, memoire et docs.
Definir une seule precedence, mesurer chaque section et produire un debug de la
composition.

### PROFILE-001 - Packs dynamiques

Retenu : oui. Un Modelfile reste minimal ; profils, RAG, outils autorises et
commandes de verification restent dans OpticCode.

## P3 - Runtime et modele

### RUNTIME-001 - Benchmark llama.cpp/Vulkan

Retenu apres LLM-002 et benchmarks stables.

Comparer sur la RX 9060 XT :

- chargement froid/chaud ;
- TTFT ;
- prompt tokens/s ;
- generation tokens/s ;
- RAM/VRAM ;
- stabilite ;
- qualite identique sur le corpus legacy.

Ne pas adopter un fork sur la base d'un chiffre communautaire isole.

### MODEL-001 - Q4_K_M vs Q5_K_M

Retenu apres stabilisation du corpus qualite. Le meilleur resultat est celui
qui maximise qualite utile sous une latence/memoire acceptable, pas le quant le
plus lourd.

### MODEL-002 - Fine-tuning/LoRA

Repousse loin.

Prerequis : plusieurs centaines ou milliers de corrections acceptees, corpus
nettoye, split train/evaluation et preuve que RAG + validateurs ne suffisent
pas. Aucun entrainement n'est recommande maintenant.

## P4 - IDE et daemon

### IDE-001 - `opticd`

Idee utile, mais apres agent CLI fiable.

Retenir pour plus tard :

- transport localhost authentifie ;
- origine validee ;
- journal append-only ;
- profils partages CLI/IDE ;
- protocole versionne ;
- aucun secret en clair.

### IDE-002 - Extension VS Code

Retenir : `vscode.diff`, `WorkspaceEdit`, versions de documents, Workspace
Trust, Tasks/Testing API et SecretStorage.

Ne pas faire maintenant : sidebar complexe, telemetry, remote development,
Marketplace et MCP complet.

### IDE-003 - MCP/LSP

MCP sera un complement d'outillage, pas le coeur de l'apply. LSP devient utile
si OpticCode fournit navigation, diagnostics ou code actions. Aucun des deux
n'est necessaire au prochain sprint.

## Idees refusees ou gelees

| Idee | Verdict | Raison |
| --- | --- | --- |
| reecrire Qwen/llama.cpp | refuse | aucun avantage produit, cout enorme |
| patch binaire GGUF | refuse | ne cree pas une connaissance Bukkit fiable |
| kernels CUDA custom | refuse | machine AMD et goulot non prouve |
| chiffres 2-3x garantis d'un fork | non fiable | hardware/protocole differents |
| TurboQuant immediat | gele | backend et support AMD non valides |
| QLoRA immediat | gele | aucun dataset accepte/refuse suffisant |
| Qdrant maintenant | refuse pour V1 | Tantivy/SQLite a prouver d'abord |
| RAG embeddings-only | refuse | mauvais pour identifiants exacts |
| contexte maximal | refuse | cout prefill et bruit |
| compression de prompt maison | gele | mesurer selection/deduplication d'abord |
| CRDT | refuse | Git + versions + 3-way suffisent pour la V1 |
| shell arbitraire agent | refuse pour V1 | surface de risque trop large |
| telemetry distante par defaut | refuse | local-first et vie privee |

## Regle de modularisation

`git_state.rs` reste un module unique pour l'instant. Sa taille seule ne
justifie pas une refactorisation. Le decouper en `porcelain`, `snapshot`,
`classification` et `policy` devient obligatoire quand au moins un de ces
declencheurs arrive :

- ajout de worktrees ou 3-way merge ;
- ajout d'un verrou de concurrence Git ;
- deux implementations de capture ;
- tests prives difficiles a isoler ;
- module depassant clairement le domaine snapshot/attribution.

`opticcode-tools/src/lib.rs`, lui, reste le monolithe a reduire avant
Tree-sitter/Tantivy.

## Ordre recommande

1. Figer Build Git State Guard apres BLAKE3, test CLI et benchmark. Fait.
2. Implementer PROC-001 timeout/cancellation. Fait.
3. Implementer APPLY-001 apply transactionnel avec pannes simulees. Fait.
4. Ajouter APPLY-002 concurrence optimiste. Fait pour le refus de conflit.
5. Ajouter GIT-002 worktree jetable. Fait.
6. Decouper les modules tools necessaires. Fait pour worktree ; poursuivre selon besoin.
7. Integrer CODE-001 Tree-sitter Java et CODE-001B1. Fait.
8. Construire CODE-001B2, edits cibles read-only. Fait.
9. Construire CODE-001B3, verification des edits en worktree. Fait.
10. Construire CONTEXT-001 selection par tache.
11. Migrer vers INDEX-001/002 SQLite + Tantivy.
12. Construire AGENT-001/002.
13. Ajouter streaming/provider.
14. Evaluer llama.cpp, Q5 et IDE seulement sur preuves.

## Prochain sprint propose

`CONTEXT-001 - Selection symbolique selon la tache`.

Le parseur, l'index, B2, B3 et LEGACY-002 fournissent maintenant un pipeline
complet jusqu'au build et au diff sans exposer le projet source. La prochaine
valeur produit est de reduire le contexte envoye au modele avec l'index
symbolique.

Contraintes du prochain sprint : contexte explique, extraits AST bornes,
budget explicite, comparaison avec le selecteur historique et aucune ecriture.
