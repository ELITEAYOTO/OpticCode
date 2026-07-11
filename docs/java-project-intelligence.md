# OpticCode - V1 Java/Bukkit Project Intelligence

Derniere mise a jour : 2026-07-06

## Objectif

OpticCode doit comprendre un projet Java/Bukkit avant de proposer des modifications.

Cette phase est volontairement deterministe :

- pas d'appel LLM ;
- analyse rapide ;
- sortie stable ;
- base pour le futur `build`, `patch` et RAG leger.

## Commande

```powershell
cargo run -q -- analyze-java --path benchmarks/mini-bukkit-plugin
```

## Ce que la commande detecte

- outil de build : Maven / Gradle / inconnu ;
- `pom.xml` ;
- `groupId`, `artifactId`, `version` ;
- version Java source/target ;
- dependances Maven ;
- `plugin.yml` ;
- main class ;
- commandes declarees ;
- commandes enregistrees via `getCommand(...)` ;
- permissions declarees ;
- classes Java ;
- classes `CommandExecutor` ;
- classes `Listener` avec `@EventHandler` ;
- risques simples Java 8 / Bukkit legacy.

## Resultat actuel sur mini plugin

```text
Build tool: Maven
Java source: 1.8
Java target: 1.8
Dependency: org.spigotmc:spigot-api:1.8.8-R0.1-SNAPSHOT provided
Plugin main: dev.opticcode.benchmark.MiniBenchmarkPlugin
Commands: coins
CommandExecutor: CoinsCommand.java
Listener: JoinListener.java
Registered command: coins in MiniBenchmarkPlugin.java
Risks: none detected
Build command: mvn -q -DskipTests package
```

## Risques detectes pour l'instant

- absence de Maven/Gradle ;
- absence de dependance Bukkit/Spigot ;
- Java source/target non Java 8 ;
- `api-version` present dans `plugin.yml` ;
- `plugin.yml` sans main class ;
- commande declaree dans `plugin.yml` sans `getCommand(...)` detecte ;
- commande enregistree en Java sans declaration dans `plugin.yml` ;
- `Material.GUNPOWDER` dans du code cible 1.8.8 ;
- pelles modernes `*_SHOVEL` au lieu de `*_SPADE` ;
- `Material.NETHER_WART` au lieu de `Material.NETHER_STALK` ;
- `Material.SPAWNER` au lieu de `Material.MOB_SPAWNER` ;
- `Material.SPAWN_EGG` au lieu de `Material.MONSTER_EGG` ;
- `EntityType.ZOMBIFIED_PIGLIN`, `MOOSHROOM`, `SNOW_GOLEM` au lieu des noms legacy ;
- `record` ou `var` ;
- imports Adventure API ;
- imports `org.bukkit.persistence`.

## Limites connues

- l'analyse Java est encore textuelle ;
- pas encore de Tree-sitter ;
- n'applique pas encore les patchs automatiquement.

## Prochaines etapes

## Build controle

Commande :

```powershell
cargo run -q -- build --path benchmarks/mini-bukkit-plugin
```

Etat actuel :

- detecte Maven via `pom.xml` ;
- lance `mvn -q -DskipTests package` ;
- affiche le statut, le code de sortie et la duree ;
- resume les erreurs utiles au lieu de noyer l'utilisateur dans tout le log.

Test negatif effectue :

- remplacement temporaire de `Material.SULPHUR` par `Material.GUNPOWDER` ;
- build Maven en echec comme attendu ;
- erreur detectee : `cannot find symbol`, fichier Java, ligne, symbole `GUNPOWDER` ;
- suggestion ajoutee : utiliser `Material.SULPHUR` pour Bukkit 1.8.8 ;
- fichier restaure apres le test.

## Patch preview

Commande :

```powershell
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin --check
```

Etat actuel :

- propose un patch texte sans modifier les fichiers ;
- verifie le patch avec `git apply --check -` quand `--check` est active ;
- cible les corrections deterministes Java legacy ;
- regles actuelles : gunpowder, nether wart, spawner, spawn egg, pelles/spades et quelques `EntityType` legacy.

Test negatif effectue :

- remplacement temporaire de `Material.SULPHUR` par `Material.GUNPOWDER` ;
- `patch` propose un unified diff correct ;
- `patch --check` valide le diff ;
- `build` echoue avant correction comme attendu ;
- fichier restaure ;
- `build` repasse OK.

Benchmark reproductible :

```powershell
.\scripts\run-patch-build-quality.ps1
```

Dernier resultat valide :

```text
build avant patch : echec
patch --check : succes
git apply : succes
build apres patch : succes
```

## Prochaines etapes

1. Etendre l'apply transactionnel actuel aux futurs patches AST avec confirmation explicite.
2. Relier l'analyse Java au profil `minecraft-java-1.8`.
3. Ajouter Tree-sitter Java plus tard pour une extraction plus robuste.
