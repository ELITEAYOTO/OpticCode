# OpticCode - Roadmap Safe Apply

Derniere mise a jour : 2026-07-07

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
- sauvegarde ou strategie de rollback ;
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
- rollback complexe cross-files.

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

- ce mode ne doit pas encore etre utilise sur PandaSpigot ou tes plugins externes ;
- pour les projets externes, utiliser encore `--copy-to <path> --yes` ;
- l'application externe attend une commande `apply --undo <run-id>` en plus du journal rollback.

## Phase SA-4 - Rollback simple

Statut : terminee pour journal + rollback manuel.

Objectif :

- pouvoir annuler une application recente.

Approche V1 :

- sauvegarder le patch applique dans `.opticcode/runs/<run-id>/patch.diff` ;
- ajouter une ligne JSONL dans `.opticcode/apply-log.jsonl` ;
- afficher la commande Git manuelle de rollback dans la sortie `apply` ;
- si le projet est sous Git, recommander `git diff` puis revert manuel.

Commande de rollback affichee :

```powershell
git apply -R ".opticcode\runs\<run-id>\patch.diff"
```

Resultat valide :

```text
Apply log:
Run id: apply-...
Patch: .opticcode\runs\apply-...\patch.diff
Rollback: git apply -R ".opticcode\runs\apply-...\patch.diff"
```

Validation effectuee :

- application sur copie temporaire interne ;
- creation de `.opticcode/apply-log.jsonl` ;
- creation du `patch.diff` ;
- rollback manuel avec `git apply -R` ;
- verification que le fichier revient a l'etat avant patch.

Approche plus tard :

- commande `apply --undo <run-id>` ;
- backups fichier par fichier pour projets non Git.

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

## Tests obligatoires

### Tests unitaires Rust

```powershell
cargo test --workspace
```

Doit rester OK.

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
7. Ajouter `apply --undo <run-id>` avant d'elargir aux vrais projets externes.

## Risques

- chemins Windows et prefixes `\\?\` ;
- differences LF/CRLF ;
- workspace Git parent qui englobe une copie temporaire ;
- patch applicable au check mais fragile apres extraction ;
- build Maven lent ou dependant du cache local ;
- modification accidentelle d'un projet externe.

Mitigation :

- garder les tests dans `benchmarks/runs` ;
- verifier les chemins absolus ;
- ne jamais suivre les symlinks comme cible d'application ;
- utiliser `git apply --check` avant application ;
- separer preview, dry-run et apply reel.
