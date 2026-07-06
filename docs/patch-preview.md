# OpticCode - Patch preview

Derniere mise a jour : 2026-07-06

## Objectif

OpticCode doit proposer des corrections sous forme de patch texte avant toute modification de fichier.

Cette phase est volontairement prudente :

- pas d'application automatique ;
- pas d'appel LLM pour cette premiere regle ;
- patch lisible par l'utilisateur ;
- base pour un futur `safe apply`.

## Commande

```powershell
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin
```

## Etat actuel

La commande `patch` propose uniquement des corrections deterministes Java legacy.

Regle supportee :

- remplacer `Material.GUNPOWDER` par `Material.SULPHUR` pour Bukkit 1.8.8.

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

## Prochaines etapes

1. Ajouter `git apply --check` sur les patchs generes.
2. Ajouter une commande `apply` avec confirmation explicite.
3. Ajouter d'autres regles legacy sures.
4. Brancher plus tard la generation LLM de patchs sur ce meme format.
