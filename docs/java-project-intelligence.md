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
Risks: none detected
Build command: mvn -q -DskipTests package
```

## Risques detectes pour l'instant

- absence de Maven/Gradle ;
- absence de dependance Bukkit/Spigot ;
- Java source/target non Java 8 ;
- `api-version` present dans `plugin.yml` ;
- `plugin.yml` sans main class ;
- `Material.GUNPOWDER` dans du code cible 1.8.8 ;
- `record` ou `var` ;
- imports Adventure API ;
- imports `org.bukkit.persistence`.

## Limites connues

- l'analyse Java est encore textuelle ;
- pas encore de Tree-sitter ;
- ne valide pas encore les correspondances entre `plugin.yml` et les appels `getCommand(...)` ;
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
- premiere regle : `Material.GUNPOWDER` vers `Material.SULPHUR`.

Test negatif effectue :

- remplacement temporaire de `Material.SULPHUR` par `Material.GUNPOWDER` ;
- `patch` propose un unified diff correct ;
- `patch --check` valide le diff ;
- `build` echoue avant correction comme attendu ;
- fichier restaure ;
- `build` repasse OK.

## Prochaines etapes

1. Comparer les commandes declarees dans `plugin.yml` avec `getCommand(...)`.
2. Ajouter un premier profil `minecraft-java-1.8` minimal.
3. Ajouter Tree-sitter Java plus tard pour une extraction plus robuste.
