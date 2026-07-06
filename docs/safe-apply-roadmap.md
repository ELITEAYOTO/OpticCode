# OpticCode - Roadmap Safe Apply

Derniere mise a jour : 2026-07-06

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

Objectif :

- ajouter une commande ou option qui prepare l'application sans modifier ;
- afficher le plan d'application ;
- afficher les fichiers touches ;
- afficher le resultat de `git apply --check`.

Commande cible possible :

```powershell
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --dry-run
```

ou :

```powershell
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin --apply-dry-run
```

Decision recommandee :

- creer une commande separee `apply` pour ne pas surcharger `patch`.

Critere de reussite :

- aucun fichier modifie ;
- sortie claire ;
- exit code non-zero si le patch n'est pas applicable.

## Phase SA-2 - Safe apply sur copie temporaire

Objectif :

- appliquer le patch dans une copie temporaire ;
- verifier que le workflow fonctionne sans toucher au projet source ;
- rendre le test automatisable.

Commande cible :

```powershell
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --copy-to benchmarks/runs/apply-test --yes
```

Critere de reussite :

- copie creee dans un dossier autorise ;
- patch applique dans la copie ;
- build apres application OK ;
- projet source intact.

## Phase SA-3 - Safe apply reel avec confirmation

Objectif :

- appliquer dans le vrai projet local uniquement si l'utilisateur confirme explicitement ;
- refuser par defaut si le workspace Git est sale, sauf option explicite.

Commande cible :

```powershell
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --yes
```

Garde-fous :

- afficher `Changes: N` ;
- afficher chemins modifies ;
- lancer `git apply --check` ;
- demander `--yes` pour mode non interactif ;
- refuser si aucun patch ;
- refuser si le projet n'est pas sous Git, sauf `--allow-no-git` plus tard ;
- ne pas lancer automatiquement un build long sauf option `--build`.

Critere de reussite :

- fichiers modifies uniquement apres confirmation ;
- `git diff` montre exactement le patch attendu ;
- build optionnel passe.

## Phase SA-4 - Rollback simple

Objectif :

- pouvoir annuler une application recente.

Approche V1 :

- sauvegarder le patch applique dans `benchmarks/runs` ou `.opticcode/runs` ;
- afficher la commande Git manuelle de rollback ;
- si le projet est sous Git, recommander `git diff` puis revert manuel.

Approche plus tard :

- journal local `.opticcode/apply-log.jsonl` ;
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

1. Implementer `apply --dry-run`.
2. Ajouter un test unitaire ou integration leger pour l'application de patch sur dossier temporaire.
3. Ajouter `apply --yes` sur copie temporaire.
4. Valider avec `run-patch-build-quality.ps1`.
5. Seulement ensuite activer application reelle avec confirmation stricte.

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
