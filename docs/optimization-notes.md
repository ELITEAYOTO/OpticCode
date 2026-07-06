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
| `plan --brief --metrics-json`, modele froid | 77.82 s | modele non charge au depart, generation reelle ~3.38 s |
| `plan --brief --metrics-json`, modele chaud | 5.86 s | `keep_alive=15m`, chargement mesure ~0.23 s |
| `run-mini-benchmark.ps1`, froid, profil+memoire, 80 tokens | 76.65 s | prompt 10 919 caracteres, load ~70.42 s |
| `run-mini-benchmark.ps1`, chaud, profil sans memoire, 80 tokens | 5.23 s | prompt 9 285 caracteres |
| `run-mini-benchmark.ps1`, chaud, profil+memoire, 80 tokens | 4.03 s | prompt 10 919 caracteres |
| `plan`, chaud, profil+memoire+RAG, 80 tokens | 3.70 s | prompt 11 299 caracteres |
| `plan`, chaud, profil+memoire sans RAG, 80 tokens | 4.05 s | prompt 10 969 caracteres |

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

Mesure importante du 2026-07-06 :

- le debit chaud reste stable autour de 26.5 tokens/s ;
- le premier appel apres dechargement peut etre beaucoup plus lent a cause du chargement du modele ;
- sur un run froid observe, le total etait ~77.8 s alors que la generation ne prenait que ~3.4 s ;
- OpticCode envoie maintenant `keep_alive=15m` par defaut pour eviter de recharger Qwen a chaque appel ;
- Ollama charge actuellement le modele avec un contexte actif de 4096 tokens, suffisant pour le mini projet.
- le premier branchement RAG ajoute peu de contexte sur une requete ciblee, mais la qualite doit etre mesuree sur plusieurs prompts.

## Optimisations prioritaires

### 1. Mesurer les appels LLM

A ajouter dans `opticcode-llm` :

Statut : premier passage ajoute.

Deja disponible avec `--metrics` :

- duree totale cote client ;
- taille du prompt en caracteres ;
- `load_duration` Ollama ;
- `keep_alive` demande ;
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

Sorties produites :

- reponse Markdown ;
- metriques texte ;
- append JSONL dans `benchmarks/runs/mini-bukkit-runs.jsonl`.

Exemple :

```powershell
.\scripts\run-mini-benchmark.ps1 -Prompt "Verifier rapidement le mini plugin Bukkit 1.8.8" -MaxTokens 80
.\scripts\run-mini-benchmark.ps1 -Prompt "Verifier rapidement le mini plugin Bukkit 1.8.8" -MaxTokens 80 -NoMemory
.\scripts\run-mini-benchmark.ps1 -Prompt "Verifier rapidement le mini plugin Bukkit 1.8.8" -MaxTokens 80 -NoRag
```

But :

- savoir si une reponse est lente a cause du prompt, du modele, du runtime ou du nombre de tokens generes.

### 2. Garder le modele chaud

Statut : ajoute.

Par defaut, les commandes `ask` et `plan` envoient :

```text
keep_alive=15m
```

Usage explicite :

```powershell
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8" --path benchmarks/mini-bukkit-plugin --brief --metrics-json --keep-alive 15m
```

Pour forcer Ollama a decharger apres l'appel :

```powershell
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8" --path benchmarks/mini-bukkit-plugin --brief --metrics-json --keep-alive 0
```

Pour ne pas envoyer le parametre :

```powershell
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8" --path benchmarks/mini-bukkit-plugin --brief --metrics-json --keep-alive none
```

Effet attendu :

- reduit fortement le temps percu entre deux appels ;
- consomme de la VRAM/RAM pendant la duree de maintien ;
- ne change pas le debit tokens/s du modele, mais evite le cout de chargement.

### 3. Reduire le preprompt

Le preprompt actuel est volontairement explicite pour la securite legacy.

Prochaine evolution :

- profil court par defaut ;
- profil Java/Bukkit seulement si projet detecte Java ou Bukkit ;
- regles specifiques comme `Material.SULPHUR` seulement si le sujet concerne les materials/items legacy.

But :

- eviter de faire penser le modele a des sujets non pertinents ;
- reduire latence et hallucinations hors sujet.

### 4. Construire un contexte utile

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

### 5. Mode bref pour iteration rapide

Statut : ajoute avec `--brief`.

Usage :

```powershell
cargo run -q -- plan "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation" --path benchmarks/mini-bukkit-plugin --brief --metrics
```

Effet :

- limite la forme de la reponse ;
- utilise `num_predict` cote Ollama ;
- reduit nettement le temps de generation.

### 6. Streaming

Actuellement Ollama est utilise avec `stream=false`.

Avantage :

- simple ;
- facile a tester.

Limite :

- l'utilisateur attend sans voir le debut de reponse.

Evolution :

- ajouter streaming plus tard pour rendre l'agent plus confortable ;
- garder `stream=false` pour les benchmarks reproductibles.

### 7. llama.cpp / C++

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

### 8. Q5_K_M

Qwen2.5-Coder 14B Q5_K_M est une piste qualite, pas une optimisation automatique.

Hypothese :

- reponses potentiellement plus precises ;
- moins d'erreurs sur certaines generations de code.

Risques :

- modele plus lourd ;
- chargement plus long ;
- debit plus faible ;
- aucun gain garanti sur Bukkit 1.8.8 sans RAG.

Decision :

- rester sur Q4_K_M pour le MVP ;
- tester Q5_K_M plus tard avec les memes prompts, memes projets et memes metriques ;
- comparer qualite, temps froid, temps chaud, tokens/s, build pass/fail.

### 9. Reglages Ollama experimentaux

Pistes a tester plus tard :

```text
OLLAMA_FLASH_ATTENTION=1
OLLAMA_KV_CACHE_TYPE=q8_0
OLLAMA_MAX_LOADED_MODELS=1
OLLAMA_NUM_PARALLEL=1
```

Ces reglages peuvent aider, surtout avec de grands contextes, mais ils doivent etre mesures.

Regle :

- un changement de runtime ou d'environnement doit produire un fichier de benchmark comparable ;
- si le gain n'est pas visible sur OpticCode, on ne le garde pas comme recommandation.

## Plan optimisation court terme

1. Ajouter metriques LLM dans la sortie de debug. Fait.
2. Ajouter une commande `context` ou un context builder interne. Fait.
3. Ajouter un mode reponse courte pour iteration rapide. Fait.
4. Ajouter `keep_alive` et `load_duration`. Fait.
5. Reduire encore le prompt `plan` avec des profils.
6. Ajouter export benchmark CSV ou fichier JSONL. Fait pour JSONL.
7. Tester streaming pour confort utilisateur.
8. Comparer Ollama avec llama.cpp/Vulkan seulement apres stabilisation des prompts.
9. Comparer Q4_K_M et Q5_K_M seulement apres mise en place du benchmark reproductible.
