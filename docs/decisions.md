# OpticCode - Decisions techniques

Derniere mise a jour : 2026-07-06

## Decisions validees

### D-001 - Ne pas entrainer un modele depuis zero

Statut : valide.

OpticCode utilisera un modele deja entraine. Le projet porte sur l'agent, les tools, le RAG, la memoire et la specialisation metier.

### D-002 - Rust comme langage principal

Statut : valide provisoirement.

Rust est retenu pour le core, le CLI, les tools, la configuration, la memoire et le RAG.

### D-003 - C++ limite au runtime bas niveau

Statut : valide provisoirement.

C++ ne doit pas devenir le langage principal du projet. Il intervient surtout via llama.cpp ou des dependances natives.

### D-004 - Java 8 comme cible Minecraft legacy

Statut : valide.

Java 8 est obligatoire pour PandaSpigot / Bukkit / Spigot 1.8.8.

### D-005 - Maven doit utiliser JDK 8

Statut : valide et corrige.

Maven utilise maintenant Temurin JDK 8.

## Decisions a prendre bientot

### D-006 - Runtime principal du MVP

Options :

- Ollama ;
- LM Studio OpenAI-compatible ;
- llama.cpp direct.

Recommandation actuelle :

- commencer par Ollama ou LM Studio ;
- garder llama.cpp pour l'optimisation.

### D-007 - Mode d'edition des fichiers

Options :

- proposer seulement des patches ;
- appliquer apres confirmation ;
- appliquer automatiquement dans un dossier autorise.

Recommandation actuelle :

- MVP : proposer des patches, puis confirmation explicite.

### D-008 - Premier projet de benchmark

Options :

- mini plugin Bukkit de test ;
- plugin existant ;
- fork PandaSpigot.

Recommandation actuelle :

- commencer par un mini plugin Bukkit Java 8 ;
- passer ensuite a un plugin reel ;
- garder PandaSpigot pour un benchmark plus lourd.

### D-009 - RAG vectoriel

Options :

- Qdrant des le debut ;
- SQLite + Tantivy + Tree-sitter d'abord ;
- embeddings plus tard.

Recommandation actuelle :

- ne pas commencer par Qdrant ;
- construire d'abord recherche texte + symboles + metadata.

### D-010 - Initialisation Git locale

Statut : action manuelle requise.

Un dossier `.git` vide existait deja mais contenait une regle Windows `DENY` qui bloque l'ecriture depuis Codex.

Decision :

- ne pas contourner la protection sandbox ;
- documenter la correction dans `docs/git-setup.md` ;
- laisser l'utilisateur initialiser Git localement.
