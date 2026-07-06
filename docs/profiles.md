# OpticCode - Profils

Derniere mise a jour : 2026-07-06

## Objectif

Les profils permettent de sortir les regles metier du code Rust et de les rendre reutilisables.

Un profil definit :

- domaine cible ;
- regles de compatibilite ;
- risques a surveiller ;
- conventions de reponse ;
- futures sources RAG et commandes de verification.

## Profil actuel

```text
skills/profiles/minecraft-java-1.8/profile.md
```

Ce profil couvre :

- Java 8 strict ;
- Bukkit / Spigot / PandaSpigot 1.8.8 et 1.8.9 ;
- `Material.SULPHUR` pour la gunpowder ;
- absence d'`api-version` dans `plugin.yml` ;
- commandes/listeners Bukkit ;
- risques de performance serveur.

## Commande de verification

```powershell
cargo run -q -- profile --path benchmarks/mini-bukkit-plugin --profile minecraft-java-1.8
```

## Utilisation dans les appels LLM

Les commandes `ask` et `plan` utilisent par defaut :

```text
--profile minecraft-java-1.8
```

Pour desactiver le profil :

```powershell
cargo run -q -- plan "Question generique" --path . --profile none
```

## Resolution du fichier

OpticCode cherche le profil dans :

1. le workspace analyse ;
2. le dossier courant du repo OpticCode.

Cela permet d'analyser un sous-projet benchmark tout en gardant les profils au niveau du depot principal.

## Prochaines etapes

1. Ajouter des profils `rust-cli` et `cpp-perf` quand le besoin apparait.
2. Ajouter une configuration YAML plus structuree si Markdown devient insuffisant.
3. Relier les profils aux futurs packs RAG.
4. Mesurer l'impact du profil sur le nombre de tokens et la qualite.
