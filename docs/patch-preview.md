# OpticCode - Patch preview

Derniere mise a jour : 2026-07-07

## Objectif

OpticCode doit proposer des corrections sous forme de patch texte avant toute modification de fichier.

Cette phase est volontairement prudente :

- pas d'application automatique ;
- pas d'appel LLM pour cette premiere regle ;
- patch lisible par l'utilisateur ;
- verification possible avec `git apply --check` ;
- base pour un futur `safe apply`.

## Commande

```powershell
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin --check
```

## Etat actuel

La commande `patch` propose uniquement des corrections deterministes Java legacy.

Regle supportee :

- remplacer plusieurs symboles modernes par leurs noms Bukkit 1.8.8 ;
- exemples : `Material.GUNPOWDER` -> `Material.SULPHUR`, `Material.WOODEN_SHOVEL` -> `Material.WOOD_SPADE`, `Material.NETHER_WART` -> `Material.NETHER_STALK`, `Material.SPAWN_EGG` -> `Material.MONSTER_EGG`.

## Resultat sur projet sain

```text
Changes: 0
No deterministic Java legacy patch is currently needed.
```

## Test negatif effectue

Modification temporaire :

```java
Material.SULPHUR -> Material.GUNPOWDER
```

Resultat :

- `patch` propose un unified diff ;
- `patch --check` valide le diff avec `git apply --check -` ;
- le fichier n'est pas modifie ;
- `build` echoue comme attendu avant correction ;
- l'erreur Maven est resumee ;
- le fichier est restaure apres test ;
- `build` repasse OK.

Extrait de patch attendu :

```diff
-        player.getInventory().addItem(new ItemStack(Material.GUNPOWDER, 1));
+        player.getInventory().addItem(new ItemStack(Material.SULPHUR, 1));
```

## Benchmark patch + build

Commande :

```powershell
.\scripts\run-patch-build-quality.ps1
```

Resultat observe :

```text
build avant patch : echec
patch --check : succes
git apply : succes
build apres patch : succes
```

Run valide :

```text
benchmarks/runs/patch-build-quality-20260706-221333/summary.md
```

Ce test travaille sur une copie temporaire du mini plugin et confirme que le patch restaure un build Maven OK.

## Prochaines etapes

1. Ajouter une commande separee `apply --dry-run`. Fait.
2. Ajouter l'application sur copie temporaire. Fait.
3. Ajouter l'application reelle avec confirmation explicite dans le workspace courant. Fait.
4. Ajouter rollback/log simple. Fait.
5. Brancher plus tard la generation LLM de patchs sur ce meme format.

Commande dry-run :

```powershell
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --dry-run
```

Sans `--dry-run`, la commande refuse encore de modifier les fichiers.

Commande copie temporaire :

```powershell
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --copy-to benchmarks/runs/apply-test --yes
```

Ce mode copie le projet, applique uniquement dans la copie, et refuse de tourner sans `--yes`.

Commande application reelle limitee au workspace courant :

```powershell
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --yes
```

Ce mode refuse les chemins externes pour l'instant. PandaSpigot et les plugins personnels restent en lecture seule ou en mode copie tant que les conditions d'elargissement hors workspace courant ne sont pas decidees.

Journal rollback :

Apres une application reussie, OpticCode cree :

```text
.opticcode/apply-log.jsonl
.opticcode/runs/<run-id>/patch.diff
```

La sortie affiche aussi :

```powershell
git apply -R ".opticcode\runs\<run-id>\patch.diff"
```

Ce rollback manuel a ete valide sur une copie temporaire.

Commande undo :

```powershell
cargo run -q -- apply --path benchmarks/runs/apply-test --undo <run-id> --yes
```

`apply --undo` verifie d'abord le reverse patch avec `git apply --check -R`, puis applique `git apply -R`.

Roadmap detaillee :

```text
docs/safe-apply-roadmap.md
```
