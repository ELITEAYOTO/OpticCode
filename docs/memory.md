# OpticCode - Memoire simple

Derniere mise a jour : 2026-07-06

## Objectif

La memoire simple permet a OpticCode de charger des notes Markdown courtes dans les prompts.

Elle sert a retenir :

- preferences utilisateur ;
- regles globales du projet ;
- regles par profil ;
- chemins de ressources utiles ;
- lecons apprises avant la future base SQLite/RAG.

## Fichiers actuels

```text
skills/memory/global.md
skills/memory/profiles/minecraft-java-1.8.md
```

## Commande

```powershell
cargo run -q -- memory --path benchmarks/mini-bukkit-plugin --profile minecraft-java-1.8
```

## Utilisation dans les prompts

Les commandes `ask` et `plan` chargent la memoire par defaut.

Pour desactiver la memoire :

```powershell
cargo run -q -- plan "Question" --path . --no-memory
```

## Resolution

OpticCode cherche actuellement :

1. `skills/memory/global.md`
2. `skills/memory/profiles/<profile>.md`
3. `.opticcode/memory.md`

La recherche se fait depuis le workspace analyse puis depuis le dossier courant.

## Limites

- La memoire est volontairement petite.
- Les fichiers sont tronques avant injection si necessaire.
- Pas encore de feedback `accepted/rejected`.
- Pas encore de SQLite.
- La memoire augmente le prompt, donc son impact doit etre surveille avec `run-mini-benchmark.ps1`.

## Prochaines etapes

1. Ajouter une commande `remember` plus tard.
2. Ajouter `.opticcode/memory.md` dans les vrais projets si besoin.
3. Migrer vers SQLite quand les donnees deviennent nombreuses.
4. Relier les entrees memoire aux benchmarks et aux patchs acceptes/refuses.
