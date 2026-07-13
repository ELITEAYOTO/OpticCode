# GIT-002 - Verification dans un worktree jetable

Date de validation : 2026-07-13

## Objectif

GIT-002 verifie un patch et son build sans appliquer la moindre modification au
worktree source. Le flux actuel utilise le generateur de patch legacy
deterministe ; le cycle de vie du worktree pourra ensuite recevoir les patchs de
l'agent.

```text
worktree source propre
  -> resolution du commit HEAD exact
  -> creation d'un worktree detache temporaire
  -> apply transactionnel dans le worktree temporaire
  -> build strict avec timeout et capture bornee
  -> capture Git et diff final
  -> preuve que la source est inchangee
  -> suppression Git controlee du worktree temporaire
```

Aucun transfert automatique vers le projet source n'est effectue.

## Commandes

Verification :

```powershell
cargo run -q -- worktree-verify --path C:\path\to\clean-git-project --json
```

Limites explicites :

```powershell
cargo run -q -- worktree-verify `
  --path C:\path\to\clean-git-project `
  --timeout-seconds 600 `
  --git-timeout-seconds 180 `
  --output-limit-bytes 1048576 `
  --json
```

Lister les leases laissees par une interruption :

```powershell
cargo run -q -- worktrees --json
```

Nettoyer explicitement une lease :

```powershell
cargo run -q -- worktrees --cleanup <run-id> --yes --json
```

## Invariants de securite

- Le projet source doit etre un worktree Git propre.
- Le commit verifie est resolu avec `HEAD^{commit}` avant creation.
- Le worktree est detache de toute branche utilisateur.
- Le commit reel et l'etat detached HEAD sont verifies apres creation.
- Les refs Git utilisateur sont empreintees avant/apres et doivent rester identiques.
- Le chemin temporaire est un enfant direct d'un stockage controle sous `%TEMP%`.
- Les symlinks, jonctions et autres reparse points sont refuses dans ce stockage.
- Chaque commande Git longue passe par le process runner borne.
- Le build utilise le Git State Guard strict.
- L'apply reste gere par APPLY-001 avec journal, backups et rollback.
- La suppression utilise uniquement `git worktree remove --force` sur le chemin
  enregistre et revalide.
- Un dossier temporaire non enregistre n'est supprime que s'il est vide.
- Aucun `git reset`, `git clean` ou `remove_dir_all` n'est utilise en production.
- Le nettoyage ignore le token d'annulation afin qu'un `Ctrl+C` ne laisse pas
  volontairement le worktree actif.
- Le commit et l'etat Git de la source sont captures une seconde fois apres le
  nettoyage.
- La comparaison source utilise le Git State Guard strict, en plus du commit et
  de l'empreinte des refs.

## Stockage et recovery

Le stockage par defaut est :

```text
%TEMP%\opticcode-worktrees\
  leases\<run-id>.json
  runs\<run-id>\
```

La lease est publiee avant la reservation du dossier de travail. Si OpticCode
plante, `worktrees` peut donc retrouver le chemin, le depot source et le commit.

Le cleanup manuel reste fail-closed :

- un worktree enregistre peut etre retire par Git ;
- une reservation non enregistree et vide peut etre supprimee ;
- un dossier non enregistre mais non vide est conserve et signale ;
- une lease invalide ou un chemin qui sort du stockage est refuse.
- relancer le cleanup d'un run deja nettoye est idempotent.

## Rapport JSON

Le schema V1 expose notamment :

- `status`, `verification_success`, `cleanup_success`,
  `lease_recovery_required` et `operation_success` ;
- le commit source avant/apres et `source.unchanged` ;
- le rapport borne de `git worktree add` ;
- le plan apply et son resultat transactionnel ;
- le build, son status process, son Git guard et ses sorties bornees ;
- l'etat Git final du worktree ;
- le diff final retenu et son indicateur `complete` ;
- le resultat du cleanup et la suppression de la lease ;
- les erreurs et avertissements.

Statuts possibles :

| Statut | Sens |
| --- | --- |
| `passed` | apply, build, preuve source et cleanup valides |
| `setup_failed` | creation ou etat initial du worktree invalide |
| `apply_failed` | apply transactionnel non valide |
| `build_failed` | build, timeout ou Git guard strict en echec |
| `cancelled` | annulation utilisateur avant la fin |
| `verification_failed` | capture finale Git/diff incomplete |
| `source_changed` | commit ou etat Git source different apres le run |

Un cleanup echoue ne remplace pas le statut de verification. Par exemple, un
build valide suivi d'un fichier Windows verrouille produit :

```json
{
  "status": "passed",
  "verification_success": true,
  "cleanup_success": false,
  "lease_recovery_required": true,
  "operation_success": false
}
```

Le code de sortie reste 7 afin de signaler l'action de recovery necessaire.

Le contenu textuel/binaire du diff est borne. Une troncature d'affichage ne fait
pas echouer la verification : `worktree_after.changes` conserve chemins, statuts,
renommages, empreintes BLAKE3 et tailles. Un echec de la commande Git, lui, fait
echouer la verification.

Codes de sortie CLI :

| Code | Sens |
| ---: | --- |
| 0 | verification ou cleanup reussi |
| 6 | verification echouee |
| 7 | cleanup echoue |
| 8 | precondition refusee |
| 9 | run id invalide |

## Tests valides

- verification reelle sur un depot temporaire avec espaces dans le chemin ;
- remplacement `GUNPOWDER -> SULPHUR` et `WOODEN_SHOVEL -> WOOD_SPADE` ;
- apply transactionnel commite uniquement dans le worktree temporaire ;
- build Maven simule reussi ;
- build Maven simule en echec avec cleanup conserve ;
- build bloque termine par timeout et Job Object Windows ;
- source sale refusee avant creation ;
- source originale identique et Git propre apres chaque scenario ;
- detached HEAD et refs source inchangees verifies ;
- chemin temporaire absent et metadata Git desenregistree apres cleanup ;
- traversal du run id refuse ;
- recovery d'une reservation vide non enregistree ;
- cleanup repete idempotent et echec cleanup distinct du resultat de verification ;
- limites process hors bornes refusees avant creation.

Validation globale au 2026-07-13 :

```text
cargo fmt --all -- --check                       OK
cargo clippy ... --all-targets -- -D warnings   OK
cargo test --workspace                          105 tests OK
```

## Limites assumees

- Le workflow public applique encore uniquement les corrections legacy
  deterministes ; il ne consomme pas encore un patch arbitraire du LLM.
- Le diff Git ne contient pas le contenu des fichiers non suivis. Leur presence
  reste visible dans le snapshot, et le patch propose est conserve dans le
  rapport apply.
- Un autre programme peut modifier le projet source pendant la verification.
  OpticCode le detecte apres coup, mais ne verrouille pas Git, VS Code ou Maven.
- Une coupure brutale peut laisser un worktree et une lease ; la recovery est
  manuelle et fail-closed.
- Aucune promotion vers le projet original n'est implementee. Elle devra etre
  une operation separee, confirmee et transactionnelle.

## Suite

La prochaine cible est `CODE-001` : Tree-sitter Java. Elle doit remplacer les
remplacements textuels globaux par des edits bases sur des noeuds et positions
syntaxiques avant d'autoriser des patchs generaux issus du modele.
