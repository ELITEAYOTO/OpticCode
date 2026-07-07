# OpticCode - Test copie reelle Kspawners

Derniere mise a jour : 2026-07-07

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

## Prochaine action recommandee

Ajouter un garde-fou de build avant projets originaux :

- detecter les fichiers modifies par le build ;
- separer clairement les modifications OpticCode des modifications Maven ;
- idealement lancer le build avec une strategie qui evite de modifier `dependency-reduced-pom.xml`.

Tant que ce point n'est pas traite, continuer les essais sur copies Git uniquement.
