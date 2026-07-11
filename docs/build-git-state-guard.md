# OpticCode - Build Git State Guard

Derniere mise a jour : 2026-07-11

Statut : implemente et valide.

## Revue post-implementation

| Point souleve | Verification / action | Verdict |
| --- | --- | --- |
| taille du module | responsabilites encore cohesives ; seuil de decoupage documente dans le backlog | pas de refactorisation cosmetique |
| FNV | remplace par BLAKE3 1.8.5, streaming 64 Kio | corrige |
| fichiers ignores | test Git reel : un `target/` ignore est absent et zero octet est lu | confirme |
| `build_generated` strict | assertion directe et test CLI : le fichier suivi propre fait echouer strict | confirme |
| vrai test CLI | le binaire est lance par `cargo test`, code/JSON/raisons verifies | ajoute |
| cout PandaSpigot | cinq snapshots read-only, 166,312 ms moyen | acceptable |

Le sprint est donc valide sur les points techniques de la revue. Les risques
restants sont documentes plus bas et ne sont pas masques par la classification.

## Objectif

Le Build Git State Guard capture l'etat d'un worktree Git avant et apres chaque
commande `build`. Il permet de distinguer :

- les changements deja presents avant le build ;
- les changements apparus ou ayant evolue pendant le build ;
- les sorties attendues de Maven/Gradle ;
- les fichiers suivis modifies alors qu'ils etaient propres ;
- les fichiers non suivis generes ;
- les cas dont l'origine reste incertaine.

Cette brique est destinee au futur agent. Un build vert ne suffit pas :
OpticCode doit aussi savoir si le build a modifie le projet.

## Architecture

Le code Git est isole dans :

```text
crates/opticcode-tools/src/git_state.rs
```

Le module separe :

1. l'execution de Git ;
2. le parsing porcelain NUL ;
3. la capture et les empreintes de contenu ;
4. la comparaison avant/apres ;
5. la classification ;
6. la politique stricte ;
7. l'affichage humain.

Flux :

```text
analyze-java
  -> capture Git avant
  -> Maven/Gradle
  -> capture Git apres, meme si le build echoue
  -> comparaison et classification
  -> rapport humain ou JSON
  -> succes global = build OK et politique stricte OK
```

`build_java_project` reste disponible avec les options par defaut.
`build_java_project_with_options` recoit maintenant `BuildOptions`.

## Commande Git stable

La capture utilise :

```text
git -c core.quotepath=false \
    -c status.relativePaths=false \
    status --porcelain=v1 -z --untracked-files=all
```

Proprietes importantes :

- `--porcelain=v1` fournit un format stable ;
- `-z` separe les champs avec NUL ;
- les chemins ne sont jamais decoupes sur les espaces ;
- un renommage lit le chemin destination puis le chemin source ;
- `status.relativePaths=false` force des chemins relatifs a la racine Git ;
- `core.quotepath=false` garde les chemins Unicode lisibles ;
- les fins de ligne PowerShell/CMD ne participent pas au parsing.

La racine est obtenue avec :

```text
git rev-parse --show-toplevel
```

## Modele de donnees

Structures serialisables principales :

- `GitStateSnapshot` ;
- `GitChange` ;
- `GitChangeKind` ;
- `GitChangeOrigin` ;
- `ClassifiedGitChange` ;
- `GitStateDiff` ;
- `GitStateDiffCounts` ;
- `GitStrictPolicy` ;
- `BuildGitReport`.

Types de changements reconnus :

| Type | Signification |
| --- | --- |
| `modified` | fichier suivi modifie |
| `added` | fichier ajoute a l'index |
| `deleted` | fichier suivi supprime |
| `renamed` | fichier renomme avec source et destination |
| `copied` | copie reconnue par Git |
| `untracked` | fichier non suivi |
| `type_changed` | changement de type Git |
| `unmerged` | conflit/non fusionne |
| `ignored` | support du statut, non demande dans la capture actuelle |
| `unknown` | statut non classe |

Chaque fichier sale capture possede aussi une empreinte :

```text
blake3:<taille>:<hash>
```

Cette empreinte permet de detecter qu'un fichier deja sale avant le build a
encore change, meme si son statut Git reste ` M` avant et apres.

Le contenu est lu en flux avec un tampon fixe de 64 Kio ; un gros JAR n'est pas
charge entierement en memoire.

BLAKE3 remplace l'empreinte FNV initiale afin de rendre une collision
accidentelle negligeable sans sacrifier la lecture en flux. La taille lue reste
incluse dans la valeur. Cette empreinte sert a comparer des snapshots locaux ;
elle ne constitue pas une signature d'authenticite.

Chaque snapshot expose aussi :

- `duration_us` ;
- `status_entries` ;
- `fingerprinted_files` ;
- `fingerprinted_bytes`.

## Classification

Origines supportees :

| Origine | Regle |
| --- | --- |
| `pre_existing` | meme statut, chemin et empreinte avant/apres |
| `build_generated` | changement suivi sur un chemin de build connu |
| `tracked_changed` | fichier suivi apparu ou ayant evolue pendant le build |
| `untracked_generated` | fichier non suivi apparu ou ayant evolue pendant le build |
| `unknown` | conflit, statut inconnu ou attribution incertaine |

La provenance ne suffit pas a elle seule pour les cas ambigus. Chaque entree
contient donc trois indicateurs orthogonaux :

- `existed_before` ;
- `changed_during_build` ;
- `tracked_was_clean_before`.

Exemple : un JAR deja non suivi dans `target/`, puis regenere par Maven, est :

```text
origin = untracked_generated
existed_before = true
changed_during_build = true
tracked_was_clean_before = false
```

Cette representation conserve a la fois le bruit preexistant et l'activite du
build, ce qu'une categorie unique ne pourrait pas exprimer.

## Chemins de build connus

La classification reconnait notamment :

- `dependency-reduced-pom.xml` ;
- `.flattened-pom.xml` ;
- backups Maven release/versions ;
- `release.properties` ;
- `buildNumber.properties` ;
- dossiers `target`, `build`, `out`, `.gradle` ;
- dossiers de sources generees, rapports et resultats de tests.

Cette liste est une heuristique explicable. Elle ne rend jamais un changement
suivi acceptable en mode strict : un fichier suivi propre modifie reste une
violation, meme s'il s'appelle `dependency-reduced-pom.xml`.

## CLI

Snapshot Git read-only, sans build :

```powershell
cargo run -q -- git-state --path <projet>
cargo run -q -- git-state --path <projet> --json
```

Rapport humain :

```powershell
cargo run -q -- build --path benchmarks/mini-bukkit-plugin
```

Rapport JSON stable :

```powershell
cargo run -q -- build --path benchmarks/mini-bukkit-plugin --json
```

Politique stricte :

```powershell
cargo run -q -- build `
  --path benchmarks/mini-bukkit-plugin `
  --fail-on-worktree-change
```

Mode strict et JSON pour agent/CI :

```powershell
cargo run -q -- build `
  --path benchmarks/mini-bukkit-plugin `
  --fail-on-worktree-change `
  --json
```

Le rapport humain resume les changements preexistants inchanges et affiche en
detail les changements detectes pendant le build. `--json` conserve tous les
snapshots et toutes les entrees.

## Semantique du succes

Le JSON separe :

- `build_success` : code de sortie Maven/Gradle ;
- `overall_success` : build et politique stricte ;
- `exit_code` : code du processus Maven/Gradle ;
- `git_guard.strict_policy` : resultat et raisons du guard.

En mode normal :

- une capture Git indisponible est expliquee ;
- elle ne transforme pas un build Maven reussi en echec.

En mode `--fail-on-worktree-change` :

- tout fichier suivi propre avant le build puis sale apres est une violation ;
- les fichiers suivis de build connus sont aussi des violations ;
- les changements preexistants inchanges ne sont pas des violations ;
- un changement supplementaire d'un fichier deja sale est rapporte, mais le
  fichier n'etait pas propre avant et ne declenche pas cette politique precise ;
- les fichiers non suivis generes sont rapportes mais ne font pas echouer le
  mode strict ;
- une capture Git indisponible fait echouer le mode strict ;
- le code de sortie OpticCode est non nul quand la politique echoue.

## Schema JSON

Le rapport racine et le rapport Git utilisent `schema_version: 1`.

Forme simplifiee :

```json
{
  "schema_version": 1,
  "project": "C:/project",
  "command": "mvn -q -DskipTests package",
  "build_success": true,
  "overall_success": false,
  "exit_code": 0,
  "duration_ms": 19,
  "summary": [],
  "stdout_tail": "",
  "stderr_tail": "",
  "git_guard": {
    "schema_version": 1,
    "status": "captured",
    "before": {},
    "after": {},
    "diff": {},
    "strict_policy": {
      "enabled": true,
      "passed": false,
      "reasons": []
    }
  }
}
```

## Tests unitaires

Le parser couvre :

- fichier modifie ;
- fichier ajoute ;
- fichier supprime ;
- fichier non suivi ;
- renommage ;
- espaces ;
- Unicode ;
- separateurs Windows ;
- plusieurs entrees NUL ;
- entree invalide ;
- entree tronquee ;
- source de renommage manquante.

La comparaison couvre :

- changement preexistant inchange ;
- sortie Maven connue ;
- fichier suivi modifie ;
- fichier non suivi genere ;
- candidat strict ;
- fichier preexistant dont l'empreinte evolue.

## Tests d'integration

`crates/opticcode-tools/tests/git_state_integration.rs` cree uniquement des
depots temporaires sous le dossier temporaire Windows.

Le test principal verifie :

1. depot initial propre ;
2. modification utilisateur preexistante ;
3. modification simulee d'un fichier suivi ;
4. reecriture simulee de `dependency-reduced-pom.xml` ;
5. creation d'un fichier non suivi ;
6. comparaison des snapshots ;
7. quatre classifications ;
8. echec strict ;
9. serialisation JSON.

Un second test effectue un vrai renommage Git avec espaces et caracteres
Unicode. Git confirme l'ordre destination/source attendu par le parser `-z`.

Un troisieme test cree un `target/` ignore. Le fichier n'apparait pas dans le
snapshot et aucun octet n'est empreinte, conformement au comportement Git.

Les commits de fixture utilisent une identite locale forcee, des hooks ignores
et `commit.gpgsign=false`. Ils ne dependent pas de la configuration Git de
l'utilisateur.

Un test d'integration dans `opticcode-cli` lance egalement le vrai binaire. Il
verifie `git-state --json`, le prefixe BLAKE3, le code de sortie strict, les
champs `build_success`/`overall_success`, les raisons et les quatre origines.

## Benchmark CLI reproductible

Commande :

```powershell
.\scripts\run-build-git-guard-quality.ps1
```

Le script cree un depot Git imbrique sous `benchmarks/runs/` et place un faux
`mvn.cmd` uniquement en tete du `PATH` du processus de test. Ce faux build :

- reussit avec le code `0` ;
- modifie `src/Main.java` ;
- reecrit `dependency-reduced-pom.xml` ;
- cree `target/generated.txt` ;
- laisse une modification README preexistante intacte.

Resultat du 2026-07-11 :

| Mesure | Resultat |
| --- | --- |
| Processus Maven simule | succes, code 0 |
| Code de sortie OpticCode strict | 1 |
| `build_success` | `true` |
| `overall_success` | `false` |
| `pre_existing` | 1 |
| `build_generated` | 1 |
| `tracked_changed` | 1 |
| `untracked_generated` | 1 |
| Candidats stricts | 2 |

Artefact local ignore par Git :

```text
benchmarks/runs/build-git-guard-20260711-052442/
```

## Validation sur copie Kspawners

Projet utilise :

```text
benchmarks/runs/real-plugin-kspawners-20260707-015407/Kspawners-copy
```

L'original `C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kspawners` n'a pas
ete modifie.

Resultat strict du 2026-07-11 :

| Mesure | Resultat |
| --- | --- |
| Build Maven | succes |
| Duree Maven | 4,362 s |
| Changements avant/apres | 61 / 61 |
| Preexistants inchanges | 60 |
| Changements pendant build | 1 |
| Fichier regenere | `target/original-KSpawner-1.0.0.jar` |
| Origine | `untracked_generated` |
| Fichier suivi propre modifie | 0 |
| Politique stricte | succes |

L'empreinte du JAR est passee de 169 342 a 170 340 octets. Le guard a donc
detecte une regeneration qu'un simple diff de statuts `?? target/` aurait
manquee.

Cette copie Kspawners ne possede aucun `.gitignore`. `git check-ignore` retourne
donc que `target/original-KSpawner-1.0.0.jar` n'est pas ignore, et porcelain le
liste bien comme `??`. Le guard n'effectue aucun scan cache de `target/` et ne
voit pas les sorties que Git ignore reellement.

`dependency-reduced-pom.xml` etait deja modifie avant le build et n'a pas
change d'empreinte pendant ce run. Il est reste `pre_existing`.

Un second build release de validation a dure 3,034 s. Les 61 changements sont
restes strictement identiques, aucun nouveau changement n'a ete attribue au
build et la politique stricte est restee verte.

## Cout des snapshots

Benchmark reproductible :

```powershell
.\scripts\run-git-snapshot-benchmark.ps1 -Iterations 5
```

Resultats du 2026-07-11 :

| Source | Moyenne | Min | Max | Entrees | Fichiers hashes | Octets lus |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| petite fixture | 49,668 ms | 47,969 ms | 52,222 ms | 2 | 2 | 20 |
| Kspawners | 63,462 ms | 57,478 ms | 70,587 ms | 61 | 61 | 896 222 |
| PandaSpigot | 166,312 ms | 152,899 ms | 173,964 ms | 10 | 10 | 186 384 |

PandaSpigot a ete lu uniquement via `git-state` ; aucun build ni aucune ecriture
n'a ete lance sur le fork original. Le cout dominant est le parcours Git du
grand worktree, pas BLAKE3.

## Limites connues

- Un autre processus qui modifie le repo pendant le build sera attribue a la
  fenetre temporelle du build ; il n'existe pas encore de verrou exclusif.
- Un fichier propre modifie puis restaure exactement avant le second snapshot
  ne laisse aucun changement net observable.
- Les fichiers ignores par Git ne sont ni listes ni empreintes ; seuls les
  changements exposes par `git status` sont attribues. Le JAR Kspawners observe
  n'etait pas ignore dans cette copie precise.
- `--untracked-files=all` peut enumerer beaucoup de fichiers dans un repo mal
  ignore. Il faudra mesurer PandaSpigot avant d'ajouter un cache.
- Les empreintes lisent le contenu des fichiers sales. Les fichiers illisibles
  restent sans empreinte et sont compares avec moins de precision.
- La liste des chemins `build_generated` est heuristique et devra devenir
  configurable par profil/projet.
- Le mode strict actuel vise exactement les fichiers suivis propres avant le
  build. Une politique future pourra aussi interdire les sorties non suivies.
- Le guard observe et explique ; il ne restaure ni ne supprime aucun fichier.
- Les builds n'ont toujours ni timeout ni cancellation.

## Prochaine etape

Le verrou principal sur le bruit Maven est maintenant traite. La prochaine
brique courte est un process runner borne avec timeout/cancellation. Elle sera
suivie par l'apply transactionnel :

1. ecrire le patch et un journal provisoire avant modification ;
2. appliquer ;
3. finaliser le journal atomiquement ;
4. tenter un rollback automatique si la finalisation echoue ;
5. journaliser aussi les undo ;
6. garder tous les tests sur copies jusqu'a validation complete.
