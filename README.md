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

Le projet a maintenant un MVP Rust fonctionnel : inspection, analyse Java/Bukkit,
Ollama/Qwen, RAG JSONL, patch legacy, apply transactionnel, rollback/recovery,
controle Git avant/apres build, verification dans un worktree jetable et analyse
syntaxique Java read-only avec Tree-sitter, puis index symbolique inter-fichiers
avec resolution conservatrice. Les builds passent aussi par un
process runner borne avec timeout, sortie limitee et terminaison de l'arbre Windows.

- Phase 0 : audit environnement Windows 10 termine.
- Phase 1 : documentation de cadrage terminee.
- Phase 1.5 : initialisation projet local terminee.
- Phase 2 : benchmark Ollama / Qwen2.5-Coder 14B termine.
- Phase 3 : recherche depots externes et analyse Qwen Code terminees.
- Phase 4 : MVP Rust fonctionnel.
- Phase 5 : tools Java en cours ; apply/worktree, Tree-sitter read-only et index Java B1 termines.
- Phase 6 : prototype RAG JSONL fonctionnel, index scalable a faire.
- Phase 7 : agent iteratif non commence.

## Documentation

- [OpticCode en bref](docs/opticcode-overview.md)
- [Audit complet du projet au 2026-07-11](docs/project-audit-2026-07-11.md)
- [Build Git State Guard](docs/build-git-state-guard.md)
- [Process runner borne](docs/process-runner.md)
- [Apply transactionnel et recovery](docs/apply-transaction.md)
- [Verification dans un worktree jetable](docs/worktree-verification.md)
- [Analyse Java Tree-sitter](docs/java-syntax.md)
- [Index symbolique Java inter-fichiers](docs/java-index.md)
- [Backlog canonique d'optimisation](docs/optimization-backlog.md)
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
- [Scan resource packs](docs/resource-pack-scan.md)
- [Inventaire sources RAG](docs/rag-source-inventory.md)
- [Index RAG JSONL](docs/rag-index.md)

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
cargo run -q -- git-state --path . --json
cargo run -q -- context --path benchmarks/mini-bukkit-plugin
cargo run -q -- analyze-java --path benchmarks/mini-bukkit-plugin
cargo run -q -- java-syntax --path benchmarks/mini-bukkit-plugin --json
cargo run -q -- java-index --path benchmarks/java-index-mini --json
cargo run -q -- build --path benchmarks/mini-bukkit-plugin
cargo run -q -- build --path benchmarks/mini-bukkit-plugin --fail-on-worktree-change
cargo run -q -- build --path benchmarks/mini-bukkit-plugin --json
cargo run -q -- build --path benchmarks/mini-bukkit-plugin --timeout-seconds 600 --output-limit-bytes 1048576 --json
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin
cargo run -q -- patch --path benchmarks/mini-bukkit-plugin --check
cargo run -q -- apply --path benchmarks/mini-bukkit-plugin --dry-run --json
cargo run -q -- apply --path C:\path\to\git-project --allow-external --yes --json
cargo run -q -- apply --path C:\path\to\git-project --undo <transaction-id> --allow-external --yes --json
cargo run -q -- transactions --path C:\path\to\git-project --json
cargo run -q -- transactions --path C:\path\to\git-project --inspect <transaction-id> --json
cargo run -q -- transactions --path C:\path\to\git-project --recover <transaction-id> --allow-external --yes --json
cargo run -q -- worktree-verify --path C:\path\to\clean-git-project --json
cargo run -q -- worktrees --json
cargo run -q -- worktrees --cleanup <run-id> --yes --json
cargo run -q -- profile --path benchmarks/mini-bukkit-plugin --profile minecraft-java-1.8
cargo run -q -- memory --path benchmarks/mini-bukkit-plugin --profile minecraft-java-1.8
cargo run -q -- pack-scan --path "C:\Users\timot\Desktop\RAG-1.8-Minecraft\1.8-JavaDoc\resource-pack-1.8\LegacyPack" --limit 25
cargo run -q -- pack-scan --path "C:\Users\timot\Desktop\minecraft\Volkaria\Pack-Volkaria" --limit 25
cargo run -q -- rag-scan --limit 8 --path "C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kspawners" --path "C:\Users\timot\Desktop\KhopeSpigot\PandaSpigot-Fork\PandaSpigot"
cargo run -q -- rag-index --output data/index --path . --path "C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kspawners"
cargo run -q -- rag-search "nether wart" --index data/index --limit 5
cargo run -q -- rag-debug "Quels risques legacy verifier pour des pelles et spawners ?" --index data/index --limit 3
cargo run -q -- search Material.SULPHUR --path . --limit 5
cargo run -q -- ask "Reponds en une phrase : quelle regle Bukkit 1.8.8 dois-tu respecter pour gunpowder ?" --path .
cargo run -q -- plan "Ajouter une commande /coins dans un plugin Bukkit 1.8.8" --path . --metrics --rag-limit 4
cargo run -q -- plan "Ajouter une commande /coins dans un plugin Bukkit 1.8.8" --path . --metrics --rag-debug
cargo run -q -- plan "Ajouter une commande /coins dans un plugin Bukkit 1.8.8" --path . --metrics --no-rag
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin --brief --metrics
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin --brief --metrics-json
cargo run -q -- inspect --path benchmarks/mini-bukkit-plugin
.\scripts\run-rag-comparison.ps1
.\scripts\run-build-git-guard-quality.ps1
.\scripts\run-git-snapshot-benchmark.ps1 -Iterations 5
.\scripts\run-apply-transaction-quality.ps1
.\scripts\run-worktree-quality.ps1
.\scripts\run-worktree-quality.ps1 -Full
.\scripts\run-java-syntax-quality.ps1
.\scripts\run-java-syntax-quality.ps1 -Full
.\scripts\run-java-index-quality.ps1
.\scripts\run-java-index-quality.ps1 -Full
```

## Prochaine etape

Continuer avec `CODE-001B2` : produire les premieres propositions d'edits legacy
sur des ranges AST verifies, sans ecriture directe. Les resolutions `ambiguous`
et `unresolved` restent read-only. Toute verification passe par un worktree et
aucune promotion automatique vers le projet source n'est autorisee.
