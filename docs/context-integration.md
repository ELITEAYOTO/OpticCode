# CONTEXT-002 - Contexte symbolique dans ask et plan

Derniere mise a jour : 2026-08-03

## Objectif

CONTEXT-002 branche le selecteur CONTEXT-001 dans les commandes LLM sans changer
silencieusement leur comportement historique. `legacy` reste le mode par defaut.
Le sprint mesure le cout et la qualite avant toute promotion de `symbol`.

## Modes CLI

- `--context-mode legacy` : ancien selecteur borne par priorite de fichiers ;
- `--context-mode symbol` : contexte Java guide par les symboles ;
- `--context-mode compare` : construit les deux variantes sans appeler le modele ;
- `--compare-generate` : autorise explicitement deux generations en mode compare ;
- `--strict-context` : refuse un contexte symbolique non fiable au lieu de revenir a legacy.

Sans `--strict-context`, un rejet symbolique produit un fallback **visible** vers
legacy. Le JSON conserve le mode demande, le mode utilise, les raisons, le warning
et `analysis_complete`.

## Refus fail-closed

Le contexte symbolique n'est pas envoye au modele si :

- l'analyse ou la selection est incomplete ;
- une limite critique est atteinte ;
- le symbole principal est ambigu ;
- un fichier change entre indexation et materialisation ;
- aucun projet Java ou symbole pertinent n'est disponible.

Le RAG est lui aussi strict : `ask` et `plan` acceptent seulement une generation
schema v2 publiee par `CURRENT`. Un index legacy, absent, incomplet ou incoherent
est une erreur lorsque le RAG est active. `--no-rag` desactive explicitement cette
source.

## Prompt stable

Le prompt `opticcode-assistant-prompt-v2` suit cet ordre deterministe :

1. systeme versionne ;
2. politique stable Java 8 / Bukkit 1.8 ;
3. outils read-only disponibles ;
4. profil ;
5. historique et memoire ;
6. demande courante ;
7. contexte projet puis RAG dynamique en fin de prompt.

Les chemins des fichiers de profil et de memoire ne sont plus injectes. Aucun
RepoMap n'a ete ajoute dans ce sprint.

## Ollama et JSON

Les options reproductibles sont `--model`, `--temperature`, `--seed`,
`--max-tokens`, `--http-timeout-ms` et `--keep-alive`. L'URL est refusee sauf si
elle cible `localhost` ou une adresse IP loopback en HTTP(S). Le modele est
verifie via `/api/tags` avant l'envoi du prompt.

`--json` emet une seule enveloppe sur stdout avec : configuration, modes,
fallback, fichiers/snippets sans contenu, scores, hashes, tokens estimes et
reels, temps de contexte/load/prompt/generation, debit, reponses et erreurs
structurees. Les anciennes sorties humaines, `--metrics` et `--metrics-json`
restent compatibles.

## Evaluation Qwen reelle

`eval --with-llm` utilise les cas EVAL-001 et enrichit uniquement les strategies
`legacy` et `symbol`. Les fixtures sont fingerprintes avant et apres. Une analyse
symbolique incomplete devient `generation.status=skipped`, jamais une fausse
reponse. `--case` choisit un sous-ensemble stable du corpus.

Configuration des runs initiaux :

- Ollama 0.32.5 local ;
- `qwen2.5-coder:14b`, GGUF Q4_K_M, 14.8B ;
- temperature 0, seed 42, sans RAG ;
- warmup explicite pour les runs chauds ;
- 64 puis 128 tokens maximum.

Sur trois cas comparables chauds (impact, configuration, legacy), les vrais
prompts moyens sont passes de 1 902 tokens en legacy a 1 188 tokens en symbol,
soit environ 37,5 % de reduction. Le retrieval symbolique a obtenu Recall@k 1,0
contre 0,833 pour legacy. La latence client totale p50 est passee de 5,931 s a
5,114 s et la p95 de 6,309 s a 5,236 s sur ce run unique par cas. Le score
qualite deterministe est toutefois passe de 0,556 a 0,333. Quatre des six
sorties ont atteint la limite de 128 tokens, ce qui rend l'echantillon sensible
a la troncature ; une metrique lexicale ne remplace pas une revue humaine.

Le premier appel sans warmup a confirme le cout froid : 77,083 s cote client,
dont 72,764 s de chargement. Les runs chauds utilisent un warmup explicite et
un `keep_alive` de 15 minutes.

Conclusion : `symbol` est prometteur pour le retrieval et le prefill, mais
`legacy` reste le defaut. Il faut elargir les repetitions et accepter/rejeter des
reponses avant de changer ce choix.

## Reproduire

Comparaison sans generation :

```powershell
.\target\release\opticcode.exe ask `
  "Locate dev.opticcode.util.Helpers#ping()." `
  --path benchmarks/java-index-mini `
  --profile none --no-memory --no-rag `
  --context-mode compare --json
```

A/B Qwen borne :

```powershell
.\target\release\opticcode.exe eval `
  --strategy legacy,symbol `
  --case impact-ping-static-import,config-java-index-permission,legacy-material-gunpowder `
  --with-llm --no-rag `
  --temperature 0 --seed 42 `
  --max-generated-tokens 128 --warmup-runs 1
```

Gate sans modele, ou avec le modele local :

```powershell
.\scripts\run-context-integration-quality.ps1
.\scripts\run-context-integration-quality.ps1 -WithLlm
```
