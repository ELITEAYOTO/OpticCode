# Documentation projet — OpticCode

## Objectif : créer un assistant code type Qwen Code, optimisé Rust/C++, avec modèle IA déjà entraîné

## 1. Vision du projet

L’objectif n’est pas de réentraîner un modèle comme Qwen depuis zéro. Ce serait beaucoup trop coûteux et inutile.
L’objectif est de créer un **agent de développement local** en **Rust/C++**, capable d’utiliser un modèle déjà entraîné comme **Qwen3-Coder**, tout en remplaçant la couche lourde autour du modèle par une architecture maison plus légère, plus rapide et plus spécialisée.

Le projet peut être résumé comme ça :

```text
OpticCode
= Agent code local en Rust/C++
+ Modèle IA open-weight déjà entraîné
+ RAG spécialisé
+ Tools de lecture/édition/compilation
+ Mémoire projet
+ Spécialisation Java / Minecraft / Rust / C++
```

Le modèle garde sa performance logique, car on ne modifie pas ses poids. On construit autour de lui un système plus intelligent, mieux organisé et plus adapté à tes besoins.

---

## 2. Différence importante : Qwen Code vs Qwen3-Coder

### Qwen3-Coder

**Qwen3-Coder** est le modèle IA spécialisé code. Le dépôt officiel indique qu’il existe en plusieurs tailles, notamment **Qwen3-Coder-480B-A35B-Instruct**, **Qwen3-Coder-30B-A3B-Instruct** et **Qwen3-Coder-Next**. Le dépôt présente aussi Qwen3-Coder comme un modèle orienté tâches de code et tâches agentiques, avec support long contexte jusqu’à **256K tokens**, extensible jusqu’à **1M tokens avec Yarn**.

Donc Qwen3-Coder est le **cerveau IA**.

### Qwen Code

**Qwen Code** est un agent de terminal open source. Il sait utiliser des modèles, lire des fichiers, exécuter des commandes, fonctionner en mode interactif, headless, IDE, daemon, SDK, etc. Le dépôt officiel indique qu’il supporte plusieurs protocoles/API : OpenAI, Anthropic, Gemini, Qwen, et aussi des modèles locaux via Ollama/vLLM.

Donc Qwen Code est le **logiciel autour du modèle**.

### Conclusion

Pour ton projet, il ne faut pas dire :

```text
Je vais refaire Qwen3-Coder.
```

Il faut dire :

```text
Je vais refaire un Qwen Code-like en Rust/C++,
en utilisant Qwen3-Coder comme moteur IA.
```

---

## 3. Recommandation IA pour ton projet

Ta config :

```text
CPU : Ryzen 7 3700X
RAM : 32 Go
GPU : RX 9060 XT 16 Go
Stockage : SSD
OS : Windows 64 bits
```

### Meilleur choix principal

Je te conseille :

```text
Qwen2.5-Coder 14B en GGUF Q4_K_M
```

Pourquoi ?

Ollama indique que `qwen3-coder:30b` possède **30B paramètres au total**, mais seulement **3.3B paramètres activés**, ce qui donne un meilleur compromis performance/coût d’inférence qu’un modèle dense classique de taille équivalente. Ollama indique aussi que cette version vise les tâches agentiques de code réelles et le long contexte.

C’est donc le meilleur choix pour ton objectif :

```text
assistant code local
agent autonome
lecture de projet
génération de code
debug
RAG
édition de fichiers
```

### À éviter sur ta config

```text
Qwen3-Coder 480B
```

Ollama indique que la version 480B locale demande au minimum environ **250 Go de mémoire ou mémoire unifiée**. Avec tes 32 Go RAM, ce n’est pas réaliste en local.

### Cas spécial : Qwen3-Coder-Next

Qwen3-Coder-Next est intéressant, car il est pensé pour les workflows agentiques et le développement local. Mais Ollama affiche la version `qwen3-coder-next:q4_K_M` autour de **52 Go** avec contexte 256K, ce qui est trop lourd pour être confortable sur ta machine actuelle avec 32 Go RAM.

Donc je le mettrais en second choix, à tester plus tard si tu upgrades la RAM.

### Choix recommandé final

```text
Choix n°1 :
Qwen3-Coder 30B quantifié

Choix n°2 :
Qwen3-Coder-Next si upgrade RAM 64/96 Go

Choix n°3 :
Qwen2.5-Coder 14B/32B si Qwen3-Coder 30B est trop lourd

À éviter :
Qwen3-Coder 480B local sur ta config actuelle
```

---

## 4. Objectif technique exact

Le but est de créer un agent qui peut faire ça :

```text
Utilisateur :
"Ajoute un système de jobs mineur compatible Minecraft 1.8.8."

NayoxCode :
1. Lit le projet
2. Trouve les fichiers importants
3. Cherche dans la base RAG
4. Vérifie les contraintes Java 8 / Bukkit 1.8.8
5. Propose un plan
6. Génère un patch
7. Modifie les fichiers
8. Lance Maven/Gradle
9. Lit les erreurs
10. Corrige
11. Affiche le diff final
```

L’objectif n’est pas juste d’avoir un chatbot.
L’objectif est d’avoir un **agent code local capable d’agir sur un projet réel**.

---

## 5. Architecture générale

```text
┌─────────────────────────────┐
│         CLI / Web UI         │
└──────────────┬──────────────┘
               ↓
┌─────────────────────────────┐
│        OpticCode Core        │
│          Rust / Tokio        │
└──────────────┬──────────────┘
               ↓
┌─────────────────────────────┐
│        Agent Planner         │
│  planifie les étapes/tools   │
└──────────────┬──────────────┘
               ↓
┌─────────────────────────────┐
│        Tool Registry         │
│ read_file, search, build...  │
└──────────────┬──────────────┘
               ↓
┌─────────────────────────────┐
│        Context Builder       │
│ RAG + fichiers + mémoire     │
└──────────────┬──────────────┘
               ↓
┌─────────────────────────────┐
│      LLM Runtime local       │
│ Ollama / llama.cpp / LM      │
└──────────────┬──────────────┘
               ↓
┌─────────────────────────────┐
│       Qwen3-Coder 30B        │
└─────────────────────────────┘
```

---

## 6. Stack conseillée

### Langage principal

```text
Rust
```

Rust est idéal pour :

```text
- serveur local
- agent
- tools
- RAG
- gestion de fichiers
- sécurité
- async
- mémoire
- indexation
- performance
```

### C++

C++ est utile pour :

```text
- llama.cpp
- inference bas niveau
- bindings natifs
- audio plus tard
- optimisation très basse couche
```

Mais je ne conseille pas de tout faire en C++.
Le meilleur équilibre :

```text
Rust = cerveau de l’agent
C++ = moteur d’inférence / natif bas niveau
```

### Runtime modèle

Trois options :

```text
1. Ollama
2. llama.cpp
3. LM Studio
```

Pour démarrer vite :

```text
Ollama
```

Pour optimiser plus bas niveau :

```text
llama.cpp
```

Pour tester facilement des modèles :

```text
LM Studio
```

---

## 7. Structure du projet

```text
Optic-code/
├── crates/
│   ├── Optic-cli/
│   │   └── interface terminal
│   │
│   ├── Optic-core/
│   │   ├── agent.rs
│   │   ├── planner.rs
│   │   ├── session.rs
│   │   ├── context.rs
│   │   └── orchestrator.rs
│   │
│   ├── Optic-llm/
│   │   ├── ollama.rs
│   │   ├── llama_cpp.rs
│   │   ├── lmstudio.rs
│   │   └── openai_compatible.rs
│   │
│   ├── Optic-tools/
│   │   ├── read_file.rs
│   │   ├── write_file.rs
│   │   ├── edit_file.rs
│   │   ├── search_files.rs
│   │   ├── run_command.rs
│   │   ├── maven.rs
│   │   ├── gradle.rs
│   │   ├── git_diff.rs
│   │   └── apply_patch.rs
│   │
│   ├── Optic-rag/
│   │   ├── chunker.rs
│   │   ├── indexer.rs
│   │   ├── embeddings.rs
│   │   ├── vector_search.rs
│   │   ├── fulltext_search.rs
│   │   └── reranker.rs
│   │
│   ├── Optic-memory/
│   │   ├── project_memory.rs
│   │   ├── user_memory.rs
│   │   ├── rules.rs
│   │   └── sqlite.rs
│   │
│   └── Optic-config/
│       ├── config.rs
│       └── profiles.rs
│
├── skills/
│   ├── java8/
│   ├── minecraft-1.8.8/
│   ├── spigot/
│   ├── rust/
│   └── volkaria/
│
├── data/
│   ├── memory.sqlite
│   ├── qdrant/
│   ├── tantivy/
│   └── embeddings/
│
├── docs/
│   ├── architecture.md
│   ├── rag.md
│   ├── tools.md
│   ├── skills.md
│   └── roadmap.md
│
└── config.toml
```

---

## 8. Module LLM

Le module LLM doit être indépendant du modèle.

Tu ne dois pas coder :

```text
call_qwen()
```

Tu dois coder :

```text
call_llm()
```

Comme ça, tu peux changer de modèle sans refaire ton agent.

Exemple :

```text
Qwen3-Coder
Devstral
DeepSeek Coder
Codestral
Llama Coder
modèle API distant
```

Interface logique :

```rust
pub trait LlmProvider {
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse>;
}
```

Providers :

```text
OllamaProvider
LlamaCppProvider
LmStudioProvider
OpenAiCompatibleProvider
```

---

## 9. Module Agent

Le module agent doit décider quoi faire.

Exemple :

```text
Demande utilisateur :
"Corrige l'erreur dans mon plugin."

Agent :
1. Lire logs
2. Chercher fichiers liés
3. Chercher dans RAG
4. Demander au modèle une analyse
5. Proposer patch
6. Appliquer patch
7. Compiler
8. Vérifier
```

Il faut éviter que le modèle fasse tout seul n’importe quoi.
Le modèle doit proposer, mais l’agent Rust doit encadrer.

Architecture :

```text
Agent
├── Planner
├── Tool Router
├── Context Builder
├── Safety Guard
├── Patch Manager
└── Verifier
```

---

## 10. Tool Registry

Un vrai agent code a besoin d’outils.

### Tools de base

```text
read_file
write_file
edit_file
list_files
search_files
grep
apply_patch
git_diff
run_command
run_maven
run_gradle
run_tests
rag_search
memory_read
memory_write
```

### Exemple de trait Rust

```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> serde_json::Value;

    async fn run(
        &self,
        input: serde_json::Value,
        ctx: ToolContext,
    ) -> anyhow::Result<serde_json::Value>;
}
```

### Exemple d’outil

```text
Tool : read_file

Input :
{
  "path": "src/main/java/fr/volkaria/jobs/JobManager.java"
}

Output :
{
  "content": "...",
  "lines": 240
}
```

---

## 11. RAG spécialisé code

Le RAG est l’un des points les plus importants du projet.

Tu ne veux pas juste faire :

```text
chercher un texte proche
```

Tu veux faire :

```text
comprendre le projet + retrouver les bons exemples + retrouver les règles compatibles
```

### Données à indexer

Pour ton usage, il faut indexer :

```text
- docs Java 8
- docs Bukkit 1.8.8
- docs Spigot 1.8.8
- docs Paper/PandaSpigot si dispo
- ProtocolLib ancien
- Vault
- WorldGuard
- WorldEdit
- exemples de plugins
- tes anciens plugins
- conventions Volkaria
- mappings legacy Bukkit
- noms d’items 1.8.8
- erreurs Maven/Gradle déjà rencontrées
```

### Exemple de règle importante

```json
{
  "id": "minecraft_1_8_8_gunpowder",
  "type": "compatibility_rule",
  "content": "En Bukkit 1.8.8, gunpowder correspond à Material.SULPHUR, pas Material.GUNPOWDER.",
  "tags": ["minecraft", "bukkit", "1.8.8", "material", "legacy"]
}
```

### Recherche hybride

Pour le code, le vectoriel seul ne suffit pas.

Il faut :

```text
1. Recherche vectorielle
2. Recherche texte exacte
3. Recherche par symboles
4. Analyse AST
5. Reranking
```

Stack recommandée :

```text
Qdrant     = recherche vectorielle
Tantivy    = recherche full-text Rust
SQLite     = metadata + mémoire
Tree-sitter = parsing code
```

---

## 12. Système de skills

Qwen Code propose des fonctionnalités type Auto-Skills, SubAgents, Agent Teams, MCP, hooks, etc.
Pour ton projet, tu peux reprendre l’idée des skills, mais en version plus simple et plus contrôlée.

Structure :

```text
skills/
├── minecraft-1.8.8/
│   ├── skill.md
│   ├── rules.json
│   ├── materials_legacy.json
│   └── examples/
│
├── java8/
│   ├── skill.md
│   └── forbidden_apis.json
│
├── rust/
│   ├── skill.md
│   └── conventions.md
│
└── volkaria/
    ├── architecture.md
    ├── conventions.md
    └── plugin_rules.md
```

Exemple `skill.md` :

```text
# Skill Minecraft 1.8.8

Contraintes :
- Toujours cibler Java 8.
- Ne jamais utiliser les API Bukkit modernes.
- Vérifier les noms legacy des matériaux.
- Préférer les events compatibles 1.8.8.
- Ne pas utiliser Adventure API.
- Ne pas utiliser les composants TextComponent modernes sauf si Bungee API dispo.
```

---

## 13. Mémoire

Il faut plusieurs mémoires.

### Mémoire utilisateur

```text
- style de réponse
- langages préférés
- conventions de code
- niveau technique
- choix Rust/C++
```

### Mémoire projet

```text
- architecture du projet
- fichiers importants
- commandes de build
- version Java
- version serveur
- dépendances
```

### Mémoire erreurs

```text
- erreur rencontrée
- cause
- correction appliquée
- fichier touché
- date
```

### Mémoire règles

```text
- Java 8 obligatoire
- Bukkit 1.8.8
- pas d’API moderne
- PandaSpigot
- performance serveur PvP/Faction
```

Stockage recommandé :

```text
SQLite pour mémoire structurée
Qdrant pour mémoire vectorielle
Tantivy pour recherche texte
```

---

## 14. Prompt système principal

```text
Tu es OpticCode, un assistant de développement local spécialisé en code, architecture logicielle et plugins Minecraft Java 1.8.8 / 1.8.9.

Objectifs :
- Aider à comprendre, modifier, corriger et optimiser du code.
- Utiliser les outils disponibles au lieu d’inventer.
- Lire les fichiers avant de proposer des modifications.
- Générer des patches propres et vérifiables.
- Compiler ou lancer les tests quand c’est possible.
- Respecter les contraintes du projet.

Contraintes générales :
- Ne jamais modifier un fichier sans expliquer pourquoi.
- Préférer les petits patches aux gros blocs flous.
- Toujours vérifier les imports.
- Toujours tenir compte de la version Java et des dépendances.
- Pour Minecraft 1.8.8, ne jamais utiliser d’API moderne incompatible.
- Si une information manque, utiliser les tools de recherche avant de répondre.

Style :
- Réponses claires.
- Pas de blabla inutile.
- Donner un plan avant les grosses modifications.
- Afficher les fichiers touchés.
```

---

## 15. Workflow type

### Exemple : ajout d’une commande

```text
Utilisateur :
"Ajoute une commande /jobs avec un menu GUI."

OpticCode :
1. list_files
2. read_file plugin.yml
3. search_files "CommandExecutor"
4. search_files "InventoryClickEvent"
5. rag_search "Bukkit 1.8.8 inventory gui command"
6. plan
7. generate_patch
8. apply_patch
9. run_maven
10. fix_errors si besoin
11. git_diff
12. résumé final
```

### Réponse finale attendue

```text
Modification terminée.

Fichiers modifiés :
- plugin.yml
- JobsCommand.java
- JobsGuiListener.java
- Main.java

Ajouté :
- commande /jobs
- GUI 27 slots
- listener de clic
- compatibilité Java 8 / Bukkit 1.8.8

Build :
- mvn package OK
```

---

## 16. Optimisation Rust/C++

### Ce que Rust optimise

```text
- démarrage rapide
- mémoire maîtrisée
- serveur local stable
- tools rapides
- indexation efficace
- parallélisme propre
- sécurité filesystem
- gestion async
```

### Ce que C++ optimise

```text
- inference llama.cpp
- backend GPU/CPU
- quantization
- calcul bas niveau
```

### Ce que le modèle garde

Le modèle garde ses capacités parce que :

```text
- les poids Qwen3-Coder ne changent pas
- l’inférence reste faite par llama.cpp/Ollama/LM Studio
- Rust ne remplace pas le modèle
- Rust organise mieux les entrées/sorties
```

Donc tu gagnes en :

```text
- légèreté
- contrôle
- spécialisation
- intégration projet
- vitesse des tools
- stabilité
```

Sans perdre la logique du modèle déjà entraîné.

---

## 17. Modes d’utilisation

### Mode CLI

```bash
Optic "Explique ce projet"
Optic "Corrige l'erreur Maven"
Optic "Ajoute un système de jobs"
```

### Mode interactif

```bash
Optic
> analyse le projet
> ajoute une commande /warp
> build
> corrige les erreurs
```

### Mode headless

```bash
Optic -p "Analyse les logs et propose une correction"
```

Qwen Code propose aussi un mode headless, un mode interactif, des intégrations IDE, un mode daemon, des SDK et des bots IM.
Tu peux reprendre cette logique progressivement.

### Mode serveur

```text
Optic serve
```

Puis interface web :

```text
localhost:3000
```

---

## 18. Sécurité

Très important : un agent code peut faire des dégâts.

Il faut ajouter :

```text
- confirmation avant écriture fichier
- confirmation avant commande dangereuse
- sandbox projet
- blocklist commandes
- whitelist dossiers
- backup automatique
- git diff obligatoire
```

Commandes interdites par défaut :

```text
rm -rf
del /s /q
format
diskpart
shutdown
powershell Invoke-WebRequest suspect
curl | bash sans confirmation
```

Règle :

```text
Le modèle ne doit jamais exécuter directement une commande.
Il propose une action, l’agent Rust valide, puis l’utilisateur confirme si nécessaire.
```

---

## 19. Roadmap conseillée

### V0 — Prototype rapide

Objectif : parler à Qwen3-Coder localement.

```text
- installer Ollama ou LM Studio
- lancer Qwen3-Coder 30B
- créer CLI Rust
- envoyer prompt
- recevoir réponse
```

### V1 — Agent minimal

```text
- CLI Rust
- config.toml
- LLM provider
- read_file
- search_files
- list_files
- prompt système
```

### V2 — Agent code réel

```text
- apply_patch
- git_diff
- run_maven
- run_gradle
- lecture erreurs
- correction automatique
```

### V3 — RAG

```text
- indexation docs
- Qdrant
- Tantivy
- SQLite
- recherche hybride
- injection contexte
```

### V4 — Skills

```text
- skill Java 8
- skill Minecraft 1.8.8
- skill Rust
- skill Volkaria
```

### V5 — Interface web

```text
- dashboard local
- historique conversations
- projets indexés
- fichiers modifiés
- logs
```

### V6 — Agent avancé

```text
- plan mode
- subagents
- reviewer
- debugger
- memory auto
- tests automatiques
```

---

## 20. Choix final recommandé

Pour ton projet, je conseille :

```text
Nom projet :
OpticCode

Langage :
Rust principalement
C++ seulement pour inference/native si nécessaire

Modèle :
Qwen3-Coder 30B en local

Runtime :
Ollama au début
llama.cpp ensuite pour optimisation

RAG :
Qdrant + Tantivy + SQLite

Parsing :
Tree-sitter

Premier domaine :
Java 8 / Minecraft 1.8.8 / PandaSpigot / Bukkit legacy

Objectif :
Créer un agent code local spécialisé, plus léger qu’un outil Python/Node,
mais capable d’utiliser la puissance logique d’un modèle Qwen déjà entraîné.
```

---

## 21. Résumé simple

Le bon projet, ce n’est pas :

```text
Refaire Qwen3-Coder en Rust.
```

Le bon projet, c’est :

```text
Refaire Qwen Code en Rust/C++,
en gardant Qwen3-Coder comme moteur IA.
```

La meilleure architecture :

```text
Rust Agent
+ C++ inference via llama.cpp/Ollama
+ Qwen3-Coder 30B
+ RAG spécialisé
+ Tools code
+ Mémoire projet
+ Skills Minecraft/Java/Rust
```

C’est réaliste, puissant, évolutif, et parfaitement adapté à ton usage.



Choix IA finale "Qwen2.5-Coder 14B en GGUF Q4_K_M" (le plus opti et perf).


Recherches sur quel model use :

Évaluation comparative des LLM open source orientés code pour un agent local sur Ryzen 7 3700X, 32 Go RAM et Radeon RX 9060 XT 16 Go
Résumé exécutif
Pour ta machine actuelle — Ryzen 7 3700X, 32 Go de RAM, Radeon RX 9060 XT 16 Go — le meilleur compromis qualité code / latence / simplicité de déploiement parmi les modèles demandés n’est pas Qwen3-Coder 30B, mais Qwen2.5-Coder 14B en GGUF Q4_K_M pour la base, avec un essai en Q5_K_M puis Q6_K si tu acceptes un peu moins de contexte pour gagner en robustesse de génération. Sur ce GPU précis, des estimateurs publics donnent environ 23,6 à 25,5 tok/s en décodage pour le profil “coding”, avec une TTFT autour de 7,6 à 8,2 s et un contexte “safe” proche de 27K tokens. Surtout, on a un retour utilisateur direct sur RX 9060 XT 16 Go indiquant un mois d’usage sans problème avec Qwen2.5-Coder 14B Q4_K_M, VS Code + Continue + Ollama sous Linux Mint. 

À l’inverse, Qwen3-Coder 30B A3B est très séduisant sur le papier — 30,5B total, 3,3B actifs, 256K de contexte natif, vraie orientation agentic coding, bon support outils — mais il est, en pratique, trop gros pour être un daily driver agréable sur 16 Go de VRAM. Les sources publiques convergent vers un besoin d’environ 21,9 Go en Q4_K_M, avec recommandation matérielle d’au moins 26 Go pour un bon équilibre. Tu peux le lancer en mode fortement quantifié et/ou avec offload CPU/RAM, mais l’expérience devient vite variable, le contexte utile se réduit, et plusieurs retours signalent des soucis de faible utilisation GPU ou d’OOM selon le runtime et le contexte. Sur ta config, ce n’est pas la meilleure expérience utilisateur locale. 

Devstral Small 2 24B est un cas intéressant : il est officiellement très fort pour l’agentic software engineering et annonce 68,0 % sur SWE-Bench Verified, mais sur RX 9060 XT 16 Go, les estimateurs publics le classent en mauvais fit avec seulement 6,8 tok/s sur le workload “coding”, ~20 s de TTFT et 4K de contexte seulement dans ce profil. Autrement dit : qualité potentielle intéressante, mais UX locale trop lente sur ton matériel actuel. 

StarCoder2 15B reste un modèle utile en contexte, surtout pour du code-in / code-out et pour sa couverture de 600+ langages, mais il est désormais clairement derrière Qwen2.5-Coder 14B sur les benchmarks modernes pertinents pour toi, notamment en Java et C++. CodeLlama 13B/34B, de son côté, est aujourd’hui surtout une référence historique : sa famille était très bonne à sa sortie, mais les comparaisons récentes le placent nettement derrière Qwen2.5-Coder sur génération, édition et Java multi-langage. 

Ma recommandation finale pour ce PC, en restant rigoureux et orienté usage réel :

Recommandation mono-modèle : Qwen2.5-Coder 14B, runtime Ollama ou llama.cpp, quant Q4_K_M pour commencer, puis essai Q5_K_M / Q6_K si tes prompts sont surtout courts à moyens et que tu veux un peu plus de fiabilité syntaxique. 
Recommandation multi-modèle : Qwen2.5-Coder 14B comme moteur principal interactif + éventuellement Devstral Small 2 24B ou Qwen3-Coder 30B uniquement pour des tâches ponctuelles plus lourdes, sur un second profil, en acceptant une latence nettement plus élevée. Si ton objectif devient “agent local très autonome”, l’upgrade qui change réellement la donne n’est pas d’abord le CPU, mais le passage à 24 Go de VRAM ou plus. 
Méthode et critères
Cette comparaison a été construite à partir de sources primaires quand elles existent : model cards officielles sur Hugging Face, dépôts/projets officiels, docs de runtime officielles, rapports techniques et annonces des éditeurs. J’ai ensuite triangulé avec des estimateurs matériels et des retours utilisateurs issus de Reddit/GitHub quand les docs officielles ne publient pas les chiffres pratiques de latence sur cartes gaming AMD 16 Go. Les estimateurs type WillItRunAI eux-mêmes précisent que leurs chiffres sont approximatifs, basés sur modèles mathématiques et spécifications publiques ; je les traite donc comme des ordres de grandeur, pas comme des benchs “laboratoire”. 

Les critères retenus sont ceux qui comptent vraiment pour un agent code local : latence perçue (TTFT + tok/s), fit VRAM/RAM, qualité sur tâches de code, aptitude sur grands dépôts et workflows agentiques, pertinence pour Java/Java 8, et enfin expérience utilisateur avec les runtimes disponibles sur AMD. Pour Java / Spigot / Minecraft, il faut noter un point important : il n’existe pas, dans les sources publiques consultées, de benchmark standard dédié à Spigot/Bukkit ou à plugins Java 8 legacy. Il faut donc utiliser des proxys sérieux : Java dans MultiPL-E, code editing, SWE-Bench, tool use, long context, et les retours de développeurs en usage réel. Cette absence de benchmark Spigot natif doit tempérer toute conclusion trop absolue. 

Les sources prioritaires les plus utiles pour ton cas ont été : le technical report Qwen2.5-Coder et la page officielle Qwen2.5-Coder-14B-Instruct ; la page officielle Qwen3-Coder-30B-A3B-Instruct chez Qwen/Hugging Face, plus Ollama et LM Studio pour les aspects de déploiement ; la model card officielle Devstral Small 2 24B et l’annonce Mistral ; les docs Ollama/LM Studio/vLLM/llama.cpp pour AMD et OpenAI-compatible endpoints ; puis des issues/threads GitHub/Reddit pour affiner la réalité de l’UX sur GPU AMD 16 Go. 

Comparatif synthétique
Le tableau suivant sépare ce qui relève de la spécification officielle et ce qui relève du fit pratique sur ta classe de matériel. Quand les sources divergent entre “taille du fichier quantifié” et “VRAM totale en runtime”, j’indique explicitement qu’il s’agit de runtime total ou d’ordre de grandeur. 

Modèle	Architecture	Params	Contexte officiel	VRAM Q4 ou équivalent	VRAM Q3 approx.	Fit sur RX 9060 XT 16 Go	Runtime conseillé	Sources
Qwen2.5-Coder 14B	Dense	14,7B	128K–131K	~8,7 Go de poids ; ~10–11 Go total typique ; sur RX 9060 XT, profil coding ~14,0 Go runtime	~7–9 Go de poids / ~8–9+ Go total	Oui, tight fit ; ~27K contexte “safe”	Ollama ou llama.cpp	
Qwen3-Coder 30B A3B	MoE	30,5B total / 3,3B actifs	256K natif	Officiel/estimateurs publics : ~21,9 Go en Q4_K_M ; LM Studio affiche ~15 Go de modèle mais pas le runtime complet	données publiques divergentes ; ordre de grandeur ~16–18+ Go selon quant/agressivité	Non comme daily driver, offload requis sur 16 Go	llama.cpp ou Ollama si tu insistes ; plutôt sur 24 Go+	
Devstral Small 2 24B	Dense	24B	256K	~18,9–20,0 Go en Q4_K_M selon estimateurs publics	public peu documenté, ordre de grandeur ~14–16+ Go si très agressif	Mauvais fit sur 16 Go pour un usage confortable	vLLM recommandé par Mistral, sinon teste via GGUF	
StarCoder2 15B	Dense	15B	16K	~12,7 Go en Q4_K_M ; ~13,8–14,5 Go en Q5/K_M	~10–11 Go approx.	Oui, tight fit	llama.cpp	
CodeLlama 13B Instruct	Dense	13B	16K	les estimateurs publics récents le placent au-delà de 16 Go dans les profils modernes consultés ; les chiffres publics sont moins cohérents que pour les modèles plus récents	n.d. fiable	À éviter sur cette machine	seulement si besoin de compatibilité historique	

Le point le plus important de ce tableau est simple : sur 16 Go de VRAM, les modèles “vraiment agréables” sont ceux qui rentrent sans gymnastique lourde. Dès que le modèle passe en offload CPU/RAM permanent, la sensation d’agent local “fluide” se dégrade vite, même si le modèle reste théoriquement exécutable. C’est exactement ce que montrent les retours utilisateurs sur AMD 16 Go : les modèles qui fit fully in VRAM “se sentent” dramatiquement meilleurs que ceux qui débordent. 

Le second tableau convertit cela en expérience perçue. Pour Qwen2.5-Coder 14B et Devstral Small 2, les chiffres s’appuient sur le profil RX 9060 XT 16 Go. Pour Qwen3-Coder 30B et CodeLlama 13B, je donne une fourchette prudente, car on manque de benches publics propres sur RX 9060 XT 16 Go et les performances deviennent extrêmement dépendantes du runtime, du context size et du type d’offload ; je l’indique explicitement. 

Modèle	tok/s estimés sur une machine proche de la tienne	TTFT typique	Latence 500 tokens	Latence 1500 tokens	Commentaire UX	Sources
Qwen2.5-Coder 14B	~23,6–25,5 tok/s	~7,6–8,2 s	~28–30 s	~68–72 s	Clairement utilisable pour assistant local interactif	
Qwen3-Coder 30B A3B	ordre de grandeur ~8–15 tok/s sur 16 Go AMD avec offload, selon runtime/context ; plus haut sur 16 Go NVIDIA et bien plus haut sur 24 Go+	ordre de grandeur ~12–25 s	~45–90 s	~2–4 min	acceptable seulement pour tâches ponctuelles, pas pour toutes les requêtes	
Devstral Small 2 24B	~6,8 tok/s en profil coding	~20,4 s	~1 min 34 s	~4 min 1 s	trop lent pour un daily driver sur 16 Go	
StarCoder2 15B	~22 tok/s sur RX 9060 XT 16 Go	TTFT non documentée proprement sur ce GPU ; en pratique, ordre de grandeur quelques secondes	~25–35 s	~70–90 s	réactif, mais moins bon que Qwen2.5 14B sur qualité	
CodeLlama 13B Instruct	non pertinent sur ta machine au quant recommandé ; offload très variable	très variable	> 1 min plausible si forcé	plusieurs minutes plausibles	modèle legacy, peu pertinent en 2026 sur 16 Go	

Voici la visualisation la plus utile pour ton cas : le compromis VRAM ↔ vitesse sur RX 9060 XT 16 Go ou matériel voisin. Les vitesses sont des ordres de grandeur issus d’estimateurs et de retours utilisateurs, pas des benchs “scientifiques” homogènes. 

text
Copier
Compromis pratique vitesse ↔ mémoire sur 16 Go de VRAM AMD

tok/s
30 |  Qwen2.5-Coder 14B
25 |
20 |  StarCoder2 15B
15 |                        Qwen3-Coder 30B A3B offload
10 |            Devstral Small 2 24B
 5 |
    +---------+---------+---------+---------+--------->
      10 Go      14 Go      18 Go      22 Go      26 Go
         runtime approx. requis en usage local
Qualité sur les tâches code et retour utilisateur
Si ton angle principal est Java / Java 8 / gros codebase / plugin Minecraft-Spigot, le modèle le plus solidement documenté dans les sources publiques de ce comparatif est paradoxalement Qwen2.5-Coder 14B, pas Qwen3-Coder 30B. La raison est simple : le technical report de Qwen2.5 publie des chiffres par langage en format instruct sur MultiPL-E. Pour Qwen2.5-Coder-14B-Instruct, on voit notamment 79,7 en Java, 85,1 en C++, 84,2 en C#, 86,8 en TypeScript, 84,5 en JavaScript et 80,1 en PHP, avec une moyenne à 79,6. Dans le même tableau, StarCoder2-15B-Instruct est à 53,8 en Java et 50,9 en C++, tandis que CodeLlama-13B-Instruct n’est qu’à 40,5 en Java et 42,2 en C++. Pour ton usage Java/Spigot, l’écart n’est donc pas marginal : il est massif. 

Le même rapport Qwen2.5 montre aussi que sur la table “code generation” — HumanEval, MBPP, BigCodeBench et LiveCodeBench — Qwen2.5-Coder-14B-Instruct dépasse à la fois StarCoder2-15B-Instruct-v0.1 et CodeLlama-13B-Instruct. En pratique, cela signifie que si tu veux un modèle local qui sache lire un repo Java, modifier plusieurs fichiers, garder une cohérence syntaxique et rester assez rapide, Qwen2.5-Coder 14B est aujourd’hui bien mieux étayé par les preuves publiques que les anciennes alternatives. 

Pour Qwen3-Coder 30B, la documentation publique officielle met surtout en avant l’agentic coding, le tool calling, l’exploration de dépôt, le long contexte et l’intégration avec Qwen Code, CLINE et autres plateformes. C’est un vrai point fort si ton futur agent doit lancer des commandes, lire beaucoup de fichiers, planifier et itérer. Le problème n’est donc pas la proposition de valeur du modèle ; le problème, sur ta machine, c’est le coût de déploiement local. À qualité potentiellement supérieure sur les tâches “agentiques”, il devient moins séduisant dès que la latence réelle explose à cause de l’offload. 

Devstral Small 2 24B mérite une lecture nuancée. Officiellement, Mistral le vend comme un agentic LLM for software engineering tasks, léger, Apache-2.0, capable d’explorer des codebases et d’éditer plusieurs fichiers, avec 68,0 % sur SWE-Bench Verified. C’est très fort sur le papier pour un modèle de cette taille. Mais les retours utilisateurs ne sont pas uniformes. Certains le trouvent lent ou peu convaincant, surtout avec certains quants ou templates ; d’autres expliquent que les problèmes venaient principalement de la chat template ou du runtime, et qu’une fois la stack corrigée, il devient nettement plus utilisable. Dit autrement : Devstral est peut-être plus fragile de stack que Qwen2.5 dans un contexte local hobbyiste. 

StarCoder2 15B reste défendable seulement dans un cas précis : tu veux un modèle simple, orienté code brut, avec bonne couverture de langages, et tu acceptes une compréhension naturelle moins forte. Les docs du modèle disent clairement qu’il n’est pas un instruction model dans sa version de base et que les commandes de type “write a function…” ne marchent pas particulièrement bien. Des retours utilisateurs vont dans le même sens : bon pour du code-in / code-out, plus limité pour la compréhension en langage naturel, moins agentique, plus “copilot-like”. Cela colle mal à un agent local moderne. 

Le résumé UX réel est donc assez net. Qwen2.5-Coder 14B est le meilleur “daily driver local” parmi les modèles comparés. Qwen3-Coder 30B est le plus prometteur si tu donnes priorité au potentiel d’agent et au long contexte, mais pas sur 16 Go comme moteur principal. Devstral 24B peut être intéressant pour tests ciblés d’agent, mais la pénalité de vitesse est trop forte sur ta config. StarCoder2 15B et CodeLlama servent davantage de points de comparaison historiques que de vrais meilleurs choix en 2026. 

Déploiement pratique sur ta machine
Sur AMD 9000 series, le choix du runtime compte presque autant que le choix du modèle. Ollama est le meilleur point d’entrée si tu veux une mise en place rapide, une API compatible OpenAI, un écosystème d’outils large et un catalogue de modèles simples à lancer. Ollama documente le support GPU AMD, et côté Linux il recommande les drivers ROCm v7. AMD indique également qu’Ollama détecte et utilise automatiquement les GPU AMD avec ROCm, ce qui renforce l’intérêt d’Ollama pour un premier setup stable. 

LM Studio est excellent pour le tuning interactif, pour voir ce qui rentre ou non, et pour expérimenter rapidement les quants et l’offload. Surtout, LM Studio 0.3.19 a explicitement ajouté le support des GPU AMD 9000 sur Linux avec ROCm. Si tu es sur Linux, c’est un argument fort. Si tu es sur Windows, les retours consultés montrent davantage de friction sur RDNA4/9000 : certains utilisateurs de 9070 XT ont dû bricoler des bibliothèques ROCm, et d’autres indiquent que Vulkan donne un meilleur comportement que ROCm sur cette génération, au moins dans certains cas. 

llama.cpp reste le runtime le plus intéressant si ton objectif est la performance fine et le contrôle. Son serveur HTTP est OpenAI-compatible, il existe en pur C/C++, il supporte le speculative decoding, et la communauté rapporte qu’en usage AMD avancé il peut parfois faire mieux qu’Ollama grâce à des réglages plus fins. Sur 7900 XTX par exemple, des utilisateurs rapportent de très bons résultats avec Qwen et expliquent qu’Ollama ne supporte pas certains modes de split qui leur donnaient plus de vitesse avec llama.cpp. Pour un projet “agent local durable”, c’est le runtime à connaître, même si Ollama reste plus simple au départ. 

vLLM a sa place, mais je le recommanderais surtout si tu es sur Linux, prêt à gérer Docker/ROCm, et que tu veux tester des modèles comme Devstral, pour lesquels Mistral recommande explicitement vLLM. Les docs vLLM supportent ROCm 6.3+, mais la documentation AMD officielle que j’ai consultée vise surtout des GPU Instinct datacenter et l’installation reste plus lourde qu’un setup GGUF via Ollama/LM Studio/llama.cpp sur une machine de bureau. Pour ton cas précis, vLLM n’est pas le meilleur premier runtime. 

Le paramétrage que je te conseille pour Qwen2.5-Coder 14B est le suivant. Commence avec Q4_K_M si ton objectif est la réactivité et un contexte confortable. Si tes réponses sont plutôt courtes à moyennes et que tu veux un peu plus de qualité sur les refactors et la syntaxe, teste Q5_K_M, puis Q6_K. Le guide matériel Qwen2.5-Coder recommande même, pour le code réel, Q5_K_M au minimum, et préfère Q6_K / Q8_0 pour les flux multi-fichiers ; sur RX 9060 XT 16 Go, les estimateurs indiquent que Q6_K reste envisageable, mais il faudra être plus raisonnable sur le contexte. 

Enfin, garde un point fondamental en tête : sur 16 Go, ta vraie ressource rare n’est pas seulement la VRAM, c’est le couple VRAM + contexte + confort d’interaction. Un modèle légèrement moins “prestigieux” mais pleinement en VRAM te donnera souvent une meilleure productivité qu’un plus gros modèle admirable en benchmark mais éternellement en offload. Les retours utilisateurs sur AMD 16 Go le disent presque mot pour mot. 

Non

Oui

Oui

Non

Tu privilégies quoi ?

Réactivité et confort

Qualité maximale locale

Agentic coding avancé

VRAM 16 Go seulement

Qwen2.5-Coder 14B
Q4_K_M puis Q5_K_M/Q6_K

Tu acceptes l'offload ?

Tester Devstral Small 2 24B
ou Qwen3-Coder 30B ponctuellement

Linux + tuning ROCm/Vulkan + patience

Qwen3-Coder 30B A3B
profil secondaire

Reste sur Qwen2.5-Coder 14B
comme moteur principal

Si le budget évolue vers 24 Go+ VRAM,
Qwen3-Coder devient beaucoup plus logique



Afficher le code
Recommandation finale et checklist d’implémentation
Ma recommandation principale pour ton PC exact est donc : Qwen2.5-Coder 14B comme moteur principal, avec Ollama si tu veux aller vite, ou llama.cpp si tu veux viser le meilleur compromis perf/contrôle à moyen terme. Je ne recommande ni Qwen3-Coder 30B, ni Devstral Small 2 24B comme modèle principal sur 16 Go de VRAM, non pas parce qu’ils seraient “mauvais”, mais parce qu’ils sont matériellement mal assortis à ta cible si tu veux une expérience fluide et quotidienne. 

La recommandation secondaire, si tu veux une architecture un peu plus ambitieuse, est un routing à deux profils. Profil A : Qwen2.5-Coder 14B pour la quasi-totalité des tâches interactives, édition, exploration modérée de dépôt, Java/Spigot, tests, petites refactors. Profil B : un modèle plus lourd comme Qwen3-Coder 30B ou Devstral Small 2 réservé à des tâches de fond plus rares, à condition d’accepter qu’elles soient lentes. Sur ta machine, ce double setup a plus de sens qu’un mono-setup “gros modèle partout”. 

Le checklist le plus rationnel est le suivant. D’abord, installe Ollama ou LM Studio selon ton OS et ton niveau de confort ; si tu es sous Linux AMD 9000, LM Studio 0.3.19+ et Ollama avec ROCm récent sont les deux options propres. Ensuite, charge Qwen2.5-Coder 14B et fixe un contexte court à modéré au départ, typiquement 8K à 16K pour garder une bonne latence, avant d’élargir si nécessaire. Puis benchmarke trois scénarios réels : génération d’un petit plugin Java 8, correction d’un bug sur ton fork Spigot/Paper, et refactor multi-fichier avec docs locales. Si les résultats sont bons mais que tu veux un chouïa plus propre, monte de Q4_K_M à Q5_K_M ou Q6_K. Enfin, seulement après cela, teste un deuxième profil avec Qwen3-Coder 30B ou Devstral pour voir si le gain qualitatif compense réellement la latence. 

Pour garder la réponse directement exploitable, voici la version courte de la checklist :

Étape	Action recommandée	Pourquoi	Sources
Runtime de départ	Ollama ou LM Studio	mise en route la plus simple sur consumer hardware AMD	
Modèle principal	Qwen2.5-Coder 14B	meilleur équilibre preuves/UX/fit sur 16 Go	
Quant de départ	Q4_K_M	vitesse et contexte corrects	
Quant à tester ensuite	Q5_K_M puis Q6_K	meilleure “propreté” code si tu acceptes moins de marge	
Taille de contexte de départ	8K–16K	évite de tuer la latence sur 16 Go	
Bench minimal	Java 8, refactor multi-fichier, debug réel	plus représentatif que les prompts jouets	
Profil secondaire	Qwen3-Coder 30B ou Devstral 24B	seulement si tu acceptes une UX nettement moins fluide	

Les sources les plus prioritaires à lire si tu veux vérifier ou reproduire l’analyse sont : Qwen2.5-Coder-14B-Instruct et son technical report pour les chiffres Java/C++ et code editing ; Qwen3-Coder-30B-A3B-Instruct chez Hugging Face, plus Ollama et LM Studio pour le déploiement local ; Devstral Small 2 24B chez Mistral/Hugging Face pour la partie software-engineering ; puis les docs Ollama GPU, vLLM ROCm, et llama.cpp server/speculative decoding pour la partie runtime. 