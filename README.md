# OpticCode

OpticCode est un projet d'agent code local specialise pour le developpement Java Minecraft 1.8.8 / 1.8.9, Bukkit/Spigot legacy, PandaSpigot, plugins et documentation personnelle.

Le projet ne vise pas a entrainer un modele IA depuis zero. Il construit une couche agentique locale autour d'un modele deja entraine, d'abord Qwen2.5-Coder 14B Instruct en GGUF Q4_K_M.

## Objectifs

- Utiliser un modele local open-weight.
- Lire et comprendre une codebase locale.
- Rechercher dans les fichiers, docs et conventions.
- Proposer des patches propres.
- Compiler avec Maven/Gradle quand c'est possible.
- Lire les erreurs et corriger progressivement.
- Garder une memoire projet.
- Respecter Java 8 et les contraintes Bukkit/Spigot 1.8.8.

## Etat actuel

Le projet a maintenant un premier squelette Rust fonctionnel.

- Phase 0 : audit environnement Windows 10 termine.
- Phase 1 : documentation de cadrage en cours.
- Phase 1.5 : initialisation projet local terminee.
- Phase 2 : benchmark Ollama / Qwen2.5-Coder 14B termine.
- Phase 3 : recherche depots externes et analyse Qwen Code terminees.
- Phase 4 : squelette Rust MVP demarre.

## Documentation

- [Etat environnement](docs/environment-audit.md)
- [Roadmap](docs/roadmap.md)
- [Architecture cible](docs/architecture.md)
- [Strategie d'etude des depots](docs/repository-research.md)
- [Decisions techniques](docs/decisions.md)
- [Resultats benchmark modele](docs/model-benchmark-results.md)
- [Analyse Qwen Code](docs/qwen-code-analysis.md)
- [Phase 4 MVP Rust](docs/phase-4-mvp.md)
- [Benchmark mini Bukkit](docs/mini-bukkit-benchmark.md)
- [Notes optimisation](docs/optimization-notes.md)
- [Tri des idees de recherche](docs/ideas-triage.md)
- [Patch preview](docs/patch-preview.md)
- [Profils](docs/profiles.md)
- [Regles Minecraft 1.8 legacy](docs/minecraft-legacy-rules.md)
- [Memoire simple](docs/memory.md)

## Arborescence prevue

```text
crates/      code Rust du futur workspace
docs/        documentation projet
skills/      regles et profils metier
data/        donnees locales, indexes, memoire
models/      references locales vers modeles, sans stocker de gros fichiers dans Git
benchmarks/  tests et scenarios realistes
scripts/     scripts de maintenance et verification
```

## Benchmark local

Un mini projet Bukkit Java 8 est disponible ici :

```text
benchmarks/mini-bukkit-plugin
```

Il sert a tester OpticCode sur une structure proche d'un plugin legacy.

## Commandes MVP actuelles

```powershell
cargo run -q -- inspect --path .
cargo run -q -- context --path benchmarks/mini-bukkit-plugin
cargo run -q -- analyze-java --path benchmarks/mini-bukkit-plugin
cargo run -q -- build --path benchmarks/mini-bukkit-plugin
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin --check
cargo run -q -- profile --path benchmarks/mini-bukkit-plugin --profile minecraft-java-1.8
cargo run -q -- memory --path benchmarks/mini-bukkit-plugin --profile minecraft-java-1.8
cargo run -q -- search Material.SULPHUR --path . --limit 5
cargo run -q -- ask "Reponds en une phrase : quelle regle Bukkit 1.8.8 dois-tu respecter pour gunpowder ?" --path .
cargo run -q -- plan "Ajouter une commande /coins dans un plugin Bukkit 1.8.8" --path . --metrics
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin --brief --metrics
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin --brief --metrics-json
cargo run -q -- inspect --path benchmarks/mini-bukkit-plugin
```

## Prochaine etape

Comparer les commandes declarees dans `plugin.yml` avec les appels `getCommand(...)`, puis relier le profil `minecraft-java-1.8` aux futures regles RAG.
