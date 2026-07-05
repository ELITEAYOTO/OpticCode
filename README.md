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

Le projet est en phase de cadrage.

- Phase 0 : audit environnement Windows 10 termine.
- Phase 1 : documentation de cadrage en cours.
- Phase 1.5 : initialisation projet local en cours.
- Phase 2 : benchmark modele local a faire.

## Documentation

- [Etat environnement](docs/environment-audit.md)
- [Roadmap](docs/roadmap.md)
- [Architecture cible](docs/architecture.md)
- [Strategie d'etude des depots](docs/repository-research.md)
- [Decisions techniques](docs/decisions.md)

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

## Prochaine etape

Tester Qwen2.5-Coder 14B localement via Ollama ou LM Studio avant de coder l'agent.

