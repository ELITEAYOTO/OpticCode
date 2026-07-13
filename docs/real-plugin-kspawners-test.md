# OpticCode - Test copie reelle Kspawners

Derniere mise a jour : 2026-07-11

## Objectif

Valider OpticCode sur une copie d'un vrai plugin personnel, sans modifier l'original.

Source originale non modifiee :

```text
C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kspawners
```

Copie de test :

```text
benchmarks/runs/real-plugin-kspawners-20260707-015407/Kspawners-copy
```

## Preparation

La copie exclut :

- `target/`
- `.git/`
- `.opticcode/`
- `libs/Factions-extracted/`

Raison :

- `libs/Factions-extracted/` contient des chemins trop longs pour Git sous Windows ;
- les `.jar` utiles dans `libs/` sont conserves ;
- le projet de test est initialise en repo Git local.

## Analyse

Commandes :

```powershell
cargo run -q -- inspect --path benchmarks/runs/real-plugin-kspawners-20260707-015407/Kspawners-copy
cargo run -q -- analyze-java --path benchmarks/runs/real-plugin-kspawners-20260707-015407/Kspawners-copy
```

Resultat :

- Git detecte ;
- Maven detecte ;
- 33 fichiers Java ;
- 11 jars dans `libs/` ;
- Java source/target 1.8 ;
- commandes detectees : `spawners`, `kspawners` ;
- listeners detectes ;
- risque detecte : `plugin.yml` contient `api-version`.

## Build

Commande :

```powershell
cargo run -q -- build --path benchmarks/runs/real-plugin-kspawners-20260707-015407/Kspawners-copy
```

Resultat :

```text
Status: OK
Duration: 9.70s
```

Le plugin compile sur la copie avec Maven.

Validation supplementaire apres integration de `PROC-001` :

```text
Status: success
Duration: 3.06s
Timeout: 120.00s
Output truncated: false
Tree strategy: windows_job_object
```

Le build borne a conserve le code de sortie 0. Le guard Git a observe 61
entrees deja presentes dans cette ancienne copie et zero changement
supplementaire pendant le run. Aucun test n'a ete lance sur l'original.

## Patch plugin.yml

OpticCode a ete etendu pour proposer une correction deterministe :

```diff
-api-version: 1.8
+# api-version disabled for Bukkit 1.8.8 compatibility
```

Raison :

- Bukkit/Spigot 1.8.8 ne s'attend pas a `api-version` ;
- remplacer par un commentaire garde un patch ligne-a-ligne stable et reversible.

Validation :

- `patch --check` : OK ;
- `apply --yes` sur copie : OK ;
- build apres apply : OK, 7.58s ;
- `apply --undo <run-id> --yes` : OK apres alignement whitespace/CRLF ;
- build apres undo : OK, 5.19s.

## Point important traite

Le test a d'abord revele un bruit de fin de ligne sur `plugin.yml` apres undo :

```text
-api-version: 1.8
+api-version: 1.8
```

Correction ajoutee :

- OpticCode detecte le style dominant LF/CRLF avant apply ;
- apres apply ou undo, OpticCode restaure ce style sur les fichiers touches ;
- un test unitaire couvre un `plugin.yml` CRLF avec apply puis undo.

Validation apres correction :

- apply puis undo ne laisse plus de diff sur `plugin.yml` ;
- apres un build Maven, le seul fichier suivi modifie observe est `dependency-reduced-pom.xml`.

Conclusion :

- le safe apply fonctionne sur copie reelle ;
- le build reste OK ;
- l'undo restaure le contenu attendu ;
- la preservation LF/CRLF est traitee pour les fichiers touches par apply/undo ;
- avant d'appliquer sur un original, il faut encore gerer le bruit de build Maven.

## Validation Build Git State Guard

Commande executee uniquement sur la copie :

```powershell
cargo run -q -- build `
  --path benchmarks/runs/real-plugin-kspawners-20260707-015407/Kspawners-copy `
  --fail-on-worktree-change `
  --json
```

Resultat :

- build Maven : succes en 4.362s ;
- succes global OpticCode : oui ;
- changements avant/apres : 61 / 61 ;
- changements preexistants inchanges : 60 ;
- un fichier non suivi regenere : `target/original-KSpawner-1.0.0.jar` ;
- taille du JAR : 169342 -> 170340 octets ;
- aucun fichier suivi propre modifie ;
- mode strict : succes ;
- original non modifie.

`dependency-reduced-pom.xml` etait deja modifie avant ce build et son empreinte
n'a pas evolue. Le guard l'a donc conserve en `pre_existing` au lieu de
l'attribuer a tort au nouveau build.

Un second run final a termine en 3.034s avec 61 changements preexistants
inchanges, zero changement pendant le build et une politique stricte validee.

Documentation detaillee :

- [`build-git-state-guard.md`](build-git-state-guard.md)

## Prochaine action recommandee

APPLY-001 et GIT-002 sont maintenant valides sur fixtures temporaires, sans
nouvelle ecriture sur Kspawners. Continuer les essais sur copies ou worktrees :
la prochaine etape est Tree-sitter Java avant les originaux.
