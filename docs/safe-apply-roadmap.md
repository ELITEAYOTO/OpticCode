# OpticCode - Roadmap Safe Apply

Derniere mise a jour : 2026-07-11

## Objectif

Ajouter une application de patch prudente, locale et verifiable.

Le but n'est pas encore de rendre OpticCode autonome. Le but est de passer de :

```text
proposer un patch
```

a :

```text
proposer -> verifier -> appliquer avec confirmation -> verifier apres application
```

## Principe directeur

OpticCode ne doit jamais modifier silencieusement un projet.

Toute application reelle doit respecter :

- patch genere de facon deterministe ou explicitement valide ;
- verification `git apply --check` avant application ;
- confirmation utilisateur explicite ;
- liste claire des fichiers touches ;
- patch, manifeste et sauvegardes durables avant la premiere ecriture ;
- rollback automatique et recovery explicite ;
- verification apres application, idealement build Maven/Gradle ;
- aucune modification de projets externes originaux pendant les tests.

## Hors scope V1

La V1 ne fera pas encore :

- generation LLM de patch applique automatiquement ;
- correction multi-iteration autonome ;
- modification de fichiers hors Java legacy deterministic ;
- resolution automatique de conflits ;
- edition de projets externes sans copie ;
- interface graphique ;
- atomicite globale cross-files ;
- resolution ou fusion automatique de conflits.

## Phase SA-0 - Etat actuel

Statut : terminee.

Disponible :

- `patch` produit un diff preview ;
- `patch --check` valide le diff ;
- `run-patch-build-quality.ps1` prouve sur copie temporaire : build casse -> patch -> build OK ;
- corrections legacy supportees : gunpowder, nether wart, spawner, spawn egg, pelles/spades, quelques mobs.

Run valide :

```text
benchmarks/runs/patch-build-quality-20260706-221333/summary.md
```

## Phase SA-1 - Safe apply dry-run

Statut : terminee.

Objectif :

- ajouter une commande ou option qui prepare l'application sans modifier ;
- afficher le plan d'application ;
- afficher les fichiers touches ;
- afficher le resultat de `git apply --check`.

Commande :

```powershell
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --dry-run
```

Decision :

- commande separee `apply` pour ne pas surcharger `patch` ;
- `--dry-run` obligatoire dans cette premiere implementation ;
- sans `--dry-run`, la commande refuse d'agir.

Critere de reussite :

- aucun fichier modifie ;
- sortie claire ;
- exit code non-zero si le patch n'est pas applicable.

Resultat sur projet sain :

```text
Mode: apply dry-run
Changes: 0
No deterministic Java legacy patch is currently needed.
```

Resultat sur copie temporaire cassee :

```text
Mode: apply dry-run
Changes: 1
Patch check:
Status: OK
Dry run: no file was modified.
```

Resultat sans `--dry-run` :

```text
Error: apply currently requires --dry-run; real file modification is not enabled yet
```

## Phase SA-2 - Safe apply sur copie temporaire

Statut : terminee.

Objectif :

- appliquer le patch dans une copie temporaire ;
- verifier que le workflow fonctionne sans toucher au projet source ;
- rendre le test automatisable.

Commande :

```powershell
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --copy-to benchmarks/runs/apply-test --yes
```

Decision :

- `--copy-to` applique uniquement dans la copie ;
- `--yes` est obligatoire pour creer et modifier la copie ;
- la cible ne doit pas deja exister ;
- la cible ne doit pas etre a l'interieur du projet source ;
- le projet source n'est pas modifie.

Critere de reussite :

- copie creee dans un dossier autorise ;
- patch applique dans la copie ;
- build apres application OK ;
- projet source intact.

Resultat sur copie temporaire cassee :

```text
Mode: apply copy
Changes: 1
Patch check:
Status: OK
Patch apply:
Status: OK
Applied in copy only; source project was not modified.
```

Resultat sans `--yes` :

```text
Error: apply with --copy-to requires --yes
```

## Phase SA-3 - Safe apply reel avec confirmation

Statut : terminee pour le workspace courant.

Objectif :

- appliquer dans le vrai projet local uniquement si l'utilisateur confirme explicitement ;
- limiter la premiere version au workspace OpticCode courant ;
- refuser les chemins externes tant que rollback/log n'est pas disponible.

Commande cible :

```powershell
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --yes
```

Commande validee sur copie de test interne :

```powershell
cargo run -q -- apply --path benchmarks/runs/apply-real-20260707-012409/workspace --yes
```

Garde-fous :

- afficher `Changes: N` ;
- afficher chemins modifies ;
- lancer `git apply --check` ;
- demander `--yes` pour mode non interactif ;
- refuser les chemins hors du workspace courant ;
- ne pas lancer automatiquement un build long sauf option `--build`.

Critere de reussite :

- fichiers modifies uniquement apres confirmation ;
- `git diff` montre exactement le patch attendu ;
- build optionnel passe.

Resultat sur copie temporaire interne cassee :

```text
Mode: apply
Changes: 1
Patch check:
Status: OK
Patch apply:
Status: OK
Applied in source project.
```

Resultat sur chemin externe :

```text
Error: real apply is currently limited to the current workspace
```

Limite volontaire :

- le mode par defaut reste limite au workspace courant ;
- les projets externes doivent passer par `--allow-external` ;
- `--copy-to <path> --yes` reste le mode recommande pour les premiers essais sur PandaSpigot ou les plugins personnels.

## Phase SA-4 - Rollback simple

Statut : terminee et remplacee par le moteur transactionnel APPLY-001.

Objectif :

- pouvoir annuler une application recente.

Approche actuelle :

- creer `.opticcode/runs/<run-id>/` exclusivement ;
- sauvegarder `patch.diff`, `manifest.json`, les backups bruts et les evenements ;
- publier `prepared` avant toute modification cible ;
- enregistrer tailles, permissions et BLAKE3 avant/apres ;
- appliquer chaque fichier par remplacement atomique sur le meme volume ;
- rollbacker automatiquement toute erreur apres preparation ;
- ajouter `apply --undo <run-id> --yes` ;
- ajouter `transactions` pour liste, inspection et recovery ;
- garder `.opticcode/apply-log.jsonl` comme index de compatibilite secondaire ;
- utiliser le reverse patch uniquement pour les anciens runs sans manifeste.

Commande undo transactionnelle :

```powershell
cargo run -q -- apply --path <projet> --undo <run-id> --yes
```

Commande recovery apres rollback incomplet :

```powershell
cargo run -q -- transactions --path <projet> --recover <run-id> --yes
```

Etats valides :

```text
prepared -> applying -> applied -> finalizing -> committed
rollback_started -> rolled_back | rollback_failed
```

Validation effectuee :

- create/modify/delete sur depots temporaires ;
- application, inspect, list et undo via le binaire reel ;
- repo propre, refus repo sale et `--allow-dirty` explicite ;
- dix points de panne deterministes ;
- rollback automatique, partiel et seconde recovery ;
- corruption du journal/backup refusee ;
- restauration octet exacte LF/CRLF et chemins Unicode ;
- code `rollback_failed` distinct et recuperation apres resolution.

Details : [`apply-transaction.md`](apply-transaction.md).

## Phase SA-4.5 - Apply externe explicite

Statut : terminee sur copies temporaires Git externes.

Objectif :

- autoriser un apply hors workspace courant seulement avec une intention explicite ;
- refuser les dossiers externes non Git ;
- refuser les repos externes avec modifications existantes ;
- permettre undo externe avec le meme verrou explicite.

Commandes :

```powershell
cargo run -q -- apply --path C:\path\to\external-git-project --yes --allow-external
cargo run -q -- apply --path C:\path\to\external-git-project --undo <run-id> --yes --allow-external
```

Conditions :

- `--yes` reste obligatoire ;
- `--allow-external` est obligatoire hors workspace courant ;
- la cible externe doit etre un worktree Git ;
- avant apply, le Git State Guard doit etre propre par defaut ;
- `--allow-dirty` est l'unique exception explicite ;
- `.opticcode/` est tolere comme trace locale OpticCode ;
- undo externe verifie les hashes before/after et refuse une derive inconnue.

Validation effectuee :

- chemin externe sans `--allow-external` refuse ;
- chemin externe non Git refuse ;
- repo Git externe propre accepte ;
- apply externe cree le log et le patch rollback ;
- `apply --undo` externe restaure le fichier ;
- repo Git externe sale refuse avant apply.
- repo sale autorise preserve exactement le changement preexistant ;
- rollback incomplet produit le code 3 puis peut etre repris explicitement.

## Phase SA-5 - Integration agent

Objectif :

- utiliser safe apply dans un cycle plus complet :

```text
analyze -> patch -> apply -> build -> summarize
```

Regle :

- le LLM peut proposer ;
- les tools deterministes verifient ;
- l'application reste confirmee.

## Phase SA-4.6 - Test copie reelle Kspawners

Statut : termine sur copie.

Objectif :

- valider safe apply sur une copie d'un vrai plugin personnel ;
- ne pas modifier l'original ;
- mesurer les limites reelles avant PandaSpigot ou plugins originaux.

Resultats :

- copie Git creee dans `benchmarks/runs/real-plugin-kspawners-20260707-015407/Kspawners-copy` ;
- `inspect` OK ;
- `analyze-java` OK ;
- risque detecte : `plugin.yml` contient `api-version` ;
- build Maven OK avant patch ;
- correction deterministe ajoutee pour neutraliser `api-version` avec un commentaire YAML ;
- `patch --check` OK ;
- `apply --yes` OK ;
- build Maven OK apres apply ;
- `apply --undo <run-id> --yes` OK apres alignement whitespace/CRLF ;
- build Maven OK apres undo.

Limite traitee :

- `plugin.yml` etait marque modifie par Git apres undo a cause d'un bruit de fin de ligne ;
- OpticCode restaure maintenant le style LF/CRLF dominant des fichiers touches apres apply et undo ;
- apres apply puis undo sans build, seul `.opticcode/` reste non suivi.

Limite restante :

- apres build Maven, `dependency-reduced-pom.xml` peut etre modifie ;
- ce bruit doit etre distingue des changements OpticCode avant de toucher des originaux.

Decision actuelle :

- le bruit de build Maven est maintenant encadre par le Build Git State Guard ;
- APPLY-001 encadre les ecritures et rollback ;
- continuer sur copies/worktrees tant que les transformations Java restent
  textuelles ; Tree-sitter et l'index inter-fichiers sont integres en lecture
  seule mais pas encore aux edits.

## Tests obligatoires

### Tests unitaires Rust

```powershell
cargo test --workspace
```

Doit rester OK.

### Test transactionnel APPLY-001

```powershell
.\scripts\run-apply-transaction-quality.ps1
```

La variante `-Full` execute fmt, Clippy workspace, tests workspace et build release.

### Test patch/build

```powershell
.\scripts\run-patch-build-quality.ps1
```

Doit rester OK.

### Test projet sain

Sur le mini plugin sain :

```powershell
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin
```

Doit retourner :

```text
Changes: 0
```

### Test source intacte

Apres un test sur copie temporaire :

```powershell
git status --short
```

Ne doit pas montrer de modification dans `benchmarks/mini-bukkit-plugin`.

## Ordre recommande maintenant

1. Implementer `apply --dry-run`. Fait.
2. Ajouter un test unitaire ou integration leger pour l'application de patch sur dossier temporaire. Fait pour dry-run.
3. Ajouter `apply --yes` sur copie temporaire. Fait via `--copy-to <path> --yes`.
4. Valider avec `run-patch-build-quality.ps1`. Fait.
5. Activer application reelle avec confirmation stricte dans le workspace courant. Fait.
6. Ajouter rollback/log local avant d'autoriser les projets externes. Fait pour journal + rollback manuel.
7. Ajouter `apply --undo <run-id>` avant d'elargir aux vrais projets externes. Fait.
8. Decider les conditions d'elargissement aux projets externes. Fait via `--allow-external`, Git propre par defaut et `--allow-dirty` explicite.
9. Tester sur une copie locale de projet reel avant tout vrai dossier personnel. Fait avec Kspawners.
10. Ajouter un garde-fou LF/CRLF avant projets originaux. Fait.
11. Ajouter un garde-fou d'etat Git apres build. Fait.
12. Ajouter journal prepare, rollback automatique et recovery. Fait via APPLY-001.
13. Verifier patch/build dans un worktree jetable. Fait via GIT-002.
14. Ajouter Tree-sitter Java read-only. Fait via CODE-001.
15. Construire l'index read-only CODE-001B1. Fait.
16. Remplacer les transformations textuelles par des ranges AST verifies dans
    CODE-001B2.

## Risques

- chemins Windows et prefixes `\\?\` ;
- differences LF/CRLF ;
- workspace Git parent qui englobe une copie temporaire ;
- patch applicable au check mais fragile apres extraction ;
- absence d'atomicite globale multi-fichiers ;
- handle Windows ou antivirus bloquant `ReplaceFileW` ;
- transaction concurrente sans verrou global de workspace ;
- build Maven lent ou dependant du cache local ;
- modification accidentelle d'un projet externe.
- repo externe deja sale avant patch.
- bruit de build Maven apres verification.

Mitigation :

- garder les tests dans `benchmarks/runs` ;
- verifier les chemins absolus ;
- ne jamais suivre les symlinks comme cible d'application ;
- utiliser `git apply --check` avant application ;
- separer preview, dry-run et apply reel.
- exiger `--allow-external` et Git propre par defaut pour les chemins externes ; `--allow-dirty` reste explicite.
- verifier l'etat Git apres build et separer les changements outil/build.
- preparer backups et journal avant ecriture, puis refuser les derives par BLAKE3.
- garder recovery destructive explicite avec `--yes`.
