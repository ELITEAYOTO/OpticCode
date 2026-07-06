# OpticCode - Benchmark mini projet Bukkit

Derniere mise a jour : 2026-07-06

## Objectif

Creer un petit projet Bukkit Java 8 pour tester OpticCode sur une cible plus proche du besoin reel.

Ce benchmark sert aussi a mesurer progressivement :

- taille du contexte envoye au modele ;
- temps de reponse ;
- qualite du preprompt ;
- qualite des tools ;
- pertinence future d'un runtime plus optimise comme llama.cpp / C++.

## Projet de test

Chemin :

```text
benchmarks/mini-bukkit-plugin
```

Contenu :

- `pom.xml`
- `src/main/resources/plugin.yml`
- `MiniBenchmarkPlugin.java`
- `CoinsCommand.java`
- `JoinListener.java`

Points legacy volontairement presents :

- Java 8 ;
- Spigot API 1.8.8 en scope `provided` ;
- pas d'`api-version` dans `plugin.yml` ;
- `Material.SULPHUR` dans `JoinListener`.

## Commandes OpticCode a tester

```powershell
cargo run -q -- inspect --path benchmarks/mini-bukkit-plugin
cargo run -q -- analyze-java --path benchmarks/mini-bukkit-plugin
cargo run -q -- search Material.SULPHUR --path benchmarks/mini-bukkit-plugin --limit 10
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin
.\scripts\run-mini-benchmark.ps1
```

## Resultats initiaux

### Inspection OpticCode

Commande :

```powershell
cargo run -q -- inspect --path benchmarks/mini-bukkit-plugin
```

Resultat :

- 6 fichiers detectes ;
- Maven detecte ;
- Gradle non detecte ;
- extensions detectees : Java, XML, YML, Markdown ;
- temps observe : moins d'une seconde apres compilation Rust.

### Recherche legacy

Commande :

```powershell
cargo run -q -- search Material.SULPHUR --path benchmarks/mini-bukkit-plugin --limit 10
```

Resultat :

- retrouve `Material.SULPHUR` dans `JoinListener.java` ;
- retrouve la note correspondante dans le README du benchmark ;
- temps observe : moins d'une seconde apres compilation Rust.

### Analyse Java/Bukkit

Commande :

```powershell
cargo run -q -- analyze-java --path benchmarks/mini-bukkit-plugin
```

Resultat :

- Maven detecte ;
- Java source/target 1.8 ;
- dependance `spigot-api:1.8.8-R0.1-SNAPSHOT` en `provided` ;
- `plugin.yml` detecte ;
- main class detectee ;
- commande `/coins` detectee ;
- `CoinsCommand.java` detecte comme `CommandExecutor` ;
- `JoinListener.java` detecte comme listener ;
- aucun risque detecte.

### Compilation Maven

Commande :

```powershell
mvn -q -DskipTests package
```

Resultat :

- compilation OK ;
- dependency `spigot-api:1.8.8-R0.1-SNAPSHOT` resolue ;
- temps observe : environ 5.5 secondes sur ce premier passage.

### Plan OpticCode

Commande :

```powershell
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin
```

Resultat :

- plan exploitable ;
- fichiers Java et `plugin.yml` correctement identifies ;
- rappelle Java 8, Bukkit 1.8.8, absence d'`api-version` ;
- temps observe : 25.52 secondes.

Limite constatee :

- le modele ne lit pas encore le contenu des fichiers, seulement le contexte d'inspection ;
- il produit donc un plan general, mais ne peut pas encore verifier finement le code ;
- prochaine amelioration : ajouter une commande/construction de contexte qui inclut des extraits controles des fichiers importants.

### Contexte enrichi

Commande :

```powershell
cargo run -q -- context --path benchmarks/mini-bukkit-plugin
```

Resultat :

- inclut `pom.xml` ;
- inclut `plugin.yml` ;
- inclut `MiniBenchmarkPlugin.java` ;
- inclut `CoinsCommand.java` ;
- inclut `JoinListener.java` ;
- total snippets : 5152 caracteres.

### Plan avec contexte enrichi et metriques

Commande :

```powershell
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin --metrics
```

Resultat :

- plan plus precis ;
- reconnait que `pom.xml` utilise Java 8 ;
- reconnait que Spigot est en scope `provided` ;
- reconnait que `plugin.yml` ne doit pas contenir `api-version` ;
- reconnait `Material.SULPHUR`.

Metriques :

| Mesure | Valeur |
| --- | --- |
| Temps client | 26.35 s |
| Taille prompt | 7177 caracteres |
| Temps Ollama total | 26.02 s |
| Prompt eval count | 1792 |
| Prompt eval duration | 1.28 s |
| Eval count | 645 |
| Eval duration | 24.41 s |
| Debit generation | 26.43 tokens/s |

Conclusion :

- le contexte enrichi ameliore la qualite ;
- le cout du prompt reste faible sur ce mini projet ;
- le cout dominant est la generation de sortie ;
- pour iterer vite, il faudra aussi limiter la longueur attendue des reponses.

### Plan bref avec contexte enrichi

Commande :

```powershell
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin --brief --metrics
```

Metriques :

| Mesure | Valeur |
| --- | --- |
| Temps client | 6.41 s |
| Taille prompt | 7090 caracteres |
| Temps Ollama total | 6.09 s |
| Prompt eval count | 1761 |
| Prompt eval duration | 1.42 s |
| Eval count | 114 |
| Eval duration | 4.28 s |
| Debit generation | 26.65 tokens/s |

Conclusion :

- le mode bref garde le contexte enrichi ;
- la generation passe de 645 a 114 tokens ;
- le temps total passe d'environ 26 s a environ 6.4 s ;
- c'est l'optimisation la plus rentable a ce stade.

## Notes optimisation

Ce benchmark doit rester petit pour isoler les couts :

- temps CLI Rust pur ;
- temps inspection/recherche ;
- temps provider Ollama ;
- taille du prompt ;
- qualite du plan.

Avant de passer a llama.cpp/C++, il faut d'abord avoir des mesures stables avec Ollama.

Constat actuel :

- outils Rust locaux : rapides ;
- Maven : acceptable sur mini projet ;
- LLM local : cout dominant ;
- optimisation prioritaire : prompt, contexte envoye, streaming, metriques, puis seulement runtime C++.
