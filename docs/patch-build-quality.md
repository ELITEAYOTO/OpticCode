# OpticCode - Qualite patch + build

Derniere mise a jour : 2026-07-06

## Objectif

Ce benchmark verifie une chaine complete sans modifier le mini projet original :

1. copier `benchmarks/mini-bukkit-plugin` dans `benchmarks/runs` ;
2. injecter volontairement des symboles modernes incompatibles Bukkit 1.8.8 ;
3. verifier que le build Maven echoue ;
4. generer un patch deterministe OpticCode ;
5. verifier le patch avec `patch --check` ;
6. appliquer le patch dans la copie temporaire ;
7. relancer le build Maven ;
8. verifier que les symboles legacy attendus sont presents.

## Commande

```powershell
.\scripts\run-patch-build-quality.ps1
```

Le script ecrit ses artefacts dans :

```text
benchmarks/runs/patch-build-quality-<timestamp>/
```

Ce dossier est ignore par Git.

## Cas teste

Symboles modernes injectes :

```text
Material.GUNPOWDER
Material.NETHER_WART
Material.SPAWNER
Material.WOODEN_SHOVEL
Material.SPAWN_EGG
```

Symboles legacy attendus apres patch :

```text
Material.SULPHUR
Material.NETHER_STALK
Material.MOB_SPAWNER
Material.WOOD_SPADE
Material.MONSTER_EGG
```

## Resultat observe

Dernier run valide :

```text
benchmarks/runs/patch-build-quality-20260706-221333/summary.md
```

Synthese :

| Etape | Exit | Attendu |
| --- | ---: | --- |
| build avant patch | 1 | echec |
| patch --check | 0 | succes |
| git apply | 0 | succes |
| build apres patch | 0 | succes |

Resultat :

```text
Succes global : True
Manquants : -
Symboles modernes restants : -
Build apres patch : OK en 2.75 s
```

## Interpretation

- Le patch deterministe sait maintenant corriger plusieurs symboles legacy en une passe.
- Le script prouve que le patch n'est pas seulement textuellement plausible : il restaure un build Maven Java 8.
- Le test reste volontairement local et reproductible ; il ne modifie aucun projet externe.
- L'application automatique sur vrais projets reste a faire dans une commande separee et confirmee explicitement.

## Notes techniques

- Le patch est genere par `cargo run -q -- patch --check`.
- Le script applique le patch uniquement dans la copie temporaire.
- `git apply` utilise `--ignore-space-change --ignore-whitespace` pour neutraliser les differences LF/CRLF entre PowerShell, Git et les fichiers Java.
