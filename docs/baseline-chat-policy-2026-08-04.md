# Baseline Chat, Policy et Edits - 2026-08-04

Cette baseline a ete capturee avant toute modification de VSCODE-CHAT-001,
POLICY-001 ou CHAT-EDIT-001.

## Git et hygiene

- HEAD initial : `ad133c487d29995d9991f90773825349acbdada5`.
- `master` propre, cinq commits devant `origin/master`.
- Remote : `https://github.com/ELITEAYOTO/OpticCode.git`.
- Une seule worktree normale, zero lease OpticCode.
- Aucun processus `opticcode.exe` ou serveur Node de test residuel.
- `git fsck --full` ne signale aucune corruption ; seuls des objets pendants
  issus des tests de worktree sont presents.
- Le document pentest reste dans le dossier prive d'idees, ignore par Git, non suivi et
  absent de l'index RAG actif.
- Aucun push n'a ete effectue.

## Runtime Rust

- `cargo fmt --all -- --check` : OK.
- Clippy workspace, toutes targets/features, `-D warnings` : OK.
- `cargo test --workspace` : 236 tests reussis, zero ignore, zero echec.
- Build release workspace : OK.
- `cargo audit` 0.22.2 : 194 dependances analysees, aucune vulnerabilite.

Protocoles machine presents :

- `opticcode.discovery` schema 1 ;
- `opticcode.assistant` schema 1 ;
- `opticcode.llm` schema 1 ;
- `version --json`, `capabilities --json` et `doctor --json` valides ;
- aides `ask` et `plan` valides sans stack overflow.

Les smokes Ask et Plan avec le MockProvider passent. Un unique smoke Ollama reel,
borne a 30 secondes et 8 tokens, a termine en timeout explicite avant generation.
Le modele et le provider restent disponibles selon `doctor`; aucun second essai
n'a ete lance pendant la baseline.

## RAG et environnement

- Ollama 0.32.5 joignable localement.
- Modele `qwen2.5-coder:14b` present, sans telechargement ni mise a jour.
- Index RAG schema 2 actif : generation
  `g-18c868667d95d404-4504-0`, 1 141 documents et 3 762 chunks.
- Java Temurin 8 et Maven 3.9.9 disponibles.
- Gradle global absent mais optionnel.

## Extension VS Code 0.1.0

- Extension installee : `opticcode-local.opticcode@0.1.0`.
- VS Code Extension Host teste avec VS Code 1.131.0.
- `npm ci` : 403 paquets audites, zero vulnerabilite.
- Compilation TypeScript stricte et ESLint : OK.
- 20 tests unitaires TypeScript : OK.
- 3 tests d'integration avec le vrai `opticcode.exe` : OK.
- Extension Host : OK.
- VSIX reconstructible et package :
  `artifacts/opticcode-vscode-0.1.0.vsix`.
- SHA-256 initial :
  `5FB3F035C6016EE43BBAAAAA435B13BF8DA1D493E29EDA94C7391AD473A07DBB`.

Cette baseline ne contient aucune approbation, proposition d'edit ou transaction
Chat persistante. L'application automatique sur le projet original reste absente.
