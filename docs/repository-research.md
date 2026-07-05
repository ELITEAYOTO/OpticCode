# OpticCode - Strategie d'etude des depots externes

Derniere mise a jour : 2026-07-06

## Principe

On ne clone pas tout par reflexe. On etudie d'abord ce qui peut nous apprendre quelque chose, puis on clone seulement si une inspection locale du code est utile.

## Ordre recommande

### 1. Qwen Code

But :

- comprendre l'architecture d'un agent terminal moderne ;
- etudier tools, sessions, memoire, skills, modes, permissions ;
- identifier ce qu'il faut reprendre conceptuellement sans copier toute la complexite.

Decision probable :

- cloner pour lecture locale apres la Phase 2.

### 2. llama.cpp

But :

- comprendre le runtime GGUF ;
- tester `llama-server` OpenAI-compatible ;
- evaluer Vulkan sur AMD Windows ;
- garder une option d'optimisation face a Ollama/LM Studio.

Decision probable :

- cloner seulement au moment ou on veut compiler/tester localement.

### 3. Tree-sitter

But :

- comprendre l'usage Rust ;
- parser Java ;
- extraire classes, methodes, imports et symboles.

Decision probable :

- pas besoin de cloner le depot principal au debut ;
- utiliser les crates Rust et la grammaire Java.

### 4. Tantivy

But :

- recherche full-text locale ;
- indexation incremental ;
- scoring BM25 pour docs/code.

Decision probable :

- pas besoin de cloner au debut ;
- utiliser comme dependance Rust.

### 5. Qdrant

But :

- RAG vectoriel local ;
- recherche hybride plus avancee.

Decision probable :

- attendre la preuve de besoin ;
- installation plus tard, possiblement via binaire ou Docker.

### 6. PandaSpigot fork

But :

- cible metier principale ;
- comprendre build, patches, conventions, erreurs courantes ;
- construire les premiers benchmarks realistes.

Decision probable :

- a cloner/analyser seulement quand le benchmark modele et les regles Java 8 sont valides.

## Depots a ne pas cloner maintenant

- Qdrant : trop tot.
- Tree-sitter complet : inutile pour le MVP.
- Tantivy complet : inutile pour le MVP.
- Plusieurs model repos Hugging Face : pas necessaire si Ollama/LM Studio gere le modele.

## Premiere recherche locale utile

Avant tout clonage massif :

1. tester le modele local ;
2. choisir runtime principal ;
3. creer le squelette Rust ;
4. seulement ensuite analyser Qwen Code et PandaSpigot.

## Politique de clonage

Aucun depot externe ne doit etre clone avant d'avoir :

- un depot local Git fonctionnel ;
- un benchmark minimal du modele local ;
- une raison claire pour chaque clone.

