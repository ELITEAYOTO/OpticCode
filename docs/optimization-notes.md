# OpticCode - Notes optimisation

Derniere mise a jour : 2026-07-06

## Principe

OpticCode doit etre optimise, mais pas au hasard.

La bonne strategie est :

1. mesurer ;
2. reduire le contexte inutile ;
3. ameliorer les tools ;
4. comparer les runtimes ;
5. seulement ensuite descendre plus bas niveau avec llama.cpp / C++ si le gain est reel.

## Etat actuel des temps

Mesures initiales sur la machine :

| Action | Temps observe | Commentaire |
| --- | --- | --- |
| `inspect` sur mini plugin | < 1 s | outil Rust local rapide |
| `search Material.SULPHUR` | < 1 s | outil Rust local rapide |
| `mvn -q -DskipTests package` | ~5.5 s | compile le mini plugin |
| `plan` via Ollama/Qwen 14B, contexte simple | 25.52 s | cout dominant cote modele |
| `plan --metrics`, contexte enrichi | 26.35 s | 7177 caracteres de prompt, 645 tokens generes |
| `plan --brief --metrics`, contexte enrichi | 6.41 s | 7090 caracteres de prompt, 114 tokens generes |

Conclusion :

Le probleme principal n'est pas encore Rust, Maven ou les tools locaux. Le cout dominant est l'inference LLM.

Detail important :

- prompt eval : 1.28 s ;
- generation : 24.41 s ;
- debit : 26.43 tokens/s.

Le contexte enrichi n'est donc pas encore le probleme principal sur un petit projet. La longueur de reponse pese beaucoup plus.

Comparaison mode court :

- prompt eval : 1.42 s ;
- generation : 4.28 s ;
- debit : 26.65 tokens/s ;
- gain total : environ 4x plus rapide que le plan long.

Conclusion :

Le mode court est une optimisation prioritaire validee. Il conserve le contexte utile tout en reduisant fortement les tokens generes.

## Optimisations prioritaires

### 1. Mesurer les appels LLM

A ajouter dans `opticcode-llm` :

Statut : premier passage ajoute.

Deja disponible avec `--metrics` :

- duree totale cote client ;
- taille du prompt en caracteres ;
- `prompt_eval_count` ;
- `prompt_eval_duration` ;
- `eval_count` ;
- `eval_duration` ;
- debit tokens/s.

Encore a ajouter plus tard :

- estimation tokens avant envoi ;
- export CSV pour benchmarks repetables.

Export JSON :

- disponible avec `--metrics-json` ;
- utile pour comparer prompts, modes et futurs runtimes.

Runner local :

```powershell
.\scripts\run-mini-benchmark.ps1
```

Les sorties sont ecrites dans `benchmarks/runs/`, ignore par Git.

But :

- savoir si une reponse est lente a cause du prompt, du modele, du runtime ou du nombre de tokens generes.

### 2. Reduire le preprompt

Le preprompt actuel est volontairement explicite pour la securite legacy.

Prochaine evolution :

- profil court par defaut ;
- profil Java/Bukkit seulement si projet detecte Java ou Bukkit ;
- regles specifiques comme `Material.SULPHUR` seulement si le sujet concerne les materials/items legacy.

But :

- eviter de faire penser le modele a des sujets non pertinents ;
- reduire latence et hallucinations hors sujet.

### 3. Construire un contexte utile

Actuellement `plan` recoit surtout :

- detection Git/Maven/Gradle ;
- liste des fichiers ;
- extensions ;
- extraits controles des fichiers importants.

Statut : premier context builder ajoute.

Prochaine evolution :

- detecter plus finement classe principale, commandes, listeners ;
- limiter strictement la taille par profil ;
- afficher une estimation de contexte envoye.

But :

- eviter les plans trop generiques ;
- permettre au modele de raisonner sur le vrai code.

### 4. Streaming

### 4. Mode bref pour iteration rapide

Statut : ajoute avec `--brief`.

Usage :

```powershell
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin --brief --metrics
```

Effet :

- limite la forme de la reponse ;
- utilise `num_predict` cote Ollama ;
- reduit nettement le temps de generation.

### 5. Streaming

Actuellement Ollama est utilise avec `stream=false`.

Avantage :

- simple ;
- facile a tester.

Limite :

- l'utilisateur attend sans voir le debut de reponse.

Evolution :

- ajouter streaming plus tard pour rendre l'agent plus confortable ;
- garder `stream=false` pour les benchmarks reproductibles.

### 6. llama.cpp / C++

llama.cpp reste important pour :

- controle fin GGUF ;
- serveur OpenAI-compatible ;
- options Vulkan sur AMD ;
- comparaison avec Ollama ;
- optimisations runtime futures.

Mais il ne faut pas le faire avant d'avoir :

- un provider abstrait ;
- des benchmarks reproductibles ;
- des prompts stabilises ;
- une idee claire du contexte utile.

## Plan optimisation court terme

1. Ajouter metriques LLM dans la sortie de debug.
2. Ajouter une commande `context` ou un context builder interne.
3. Reduire le prompt `plan`.
4. Ajouter un mode reponse courte pour iteration rapide.
5. Comparer deux prompts sur le mini plugin.
6. Ajouter export benchmark CSV ou fichier JSONL.
7. Ensuite seulement tester llama.cpp.
