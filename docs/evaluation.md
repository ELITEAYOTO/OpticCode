# EVAL-001 - Evaluation reproductible

Derniere mise a jour : 2026-08-03

## But

EVAL-001 mesure le contexte et la recherche avant de choisir un nouveau moteur,
un nouveau mode par defaut ou une integration IDE. Le runner est read-only : il
fingerprinte chaque fixture avant et apres le run et refuse de publier un rapport
si son contenu a change.

Les strategies disponibles sont :

- `legacy` : selecteur historique par priorite de fichiers ;
- `symbol` : CONTEXT-001, Tree-sitter et index inter-fichiers ;
- `exact` : recherche textuelle exacte existante ;
- `rag` : index lexical securise v2 publie par `CURRENT`.

EVAL-001 n'ajoute aucun moteur de recherche. La combinaison symboles + RAG n'est
pas inventee tant qu'elle n'existe pas dans le produit.

## Corpus v1

Le fichier `benchmarks/eval/context-retrieval-v1.json` contient 45 cas, soit neuf
cas dans chacune des cinq categories :

1. symbole exact ;
2. architecture inter-fichiers ;
3. impact et appelants ;
4. Maven, Gradle, `plugin.yml` et Bukkit ;
5. regles legacy et cas negatifs.

Les fixtures versionnees sont petites et artificielles. PandaSpigot et
Kspawners sont seulement des fixtures externes optionnelles. Leur absence donne
un resultat `skipped`, sans erreur et sans chemin personnel dans le corpus.

## Metriques

Pour le retrieval, le fichier pertinent est l'unite canonique lorsqu'il est
fourni. Un symbole devient l'unite seulement pour un cas sans fichier attendu.
Cela evite de compter deux fois un meme snippet comme fichier et declaration.

- Hit@1, Hit@3 et Hit@5 : au moins un resultat pertinent dans la fenetre ;
- Recall@k : unites pertinentes distinctes retrouvees / unites attendues ;
- MRR : inverse du rang du premier resultat pertinent ;
- NDCG@5 : DCG binaire normalise, sans credit supplementaire pour un doublon ;
- doublons : couples chemin/symbole repetes ;
- diversite : fichiers uniques / resultats uniques ;
- hors perimetre : resultats non pertinents lorsque des attentes existent ;
- p50/p95 : nearest-rank sur la latence mesuree.

Les metriques de contexte distinguent fichiers, snippets, caracteres, octets,
troncatures, budgets et temps de decouverte/ranking/materialisation.
`ceil_unicode_chars_div_4` est toujours etiquete comme **estimation**. Les vrais
`prompt_eval_count` et `eval_count` Ollama restent des champs separes et ne sont
jamais deduits de cette estimation.

## Rapports

Chaque run contient notamment : version de schema, version OpticCode, commit Git,
suite et version, configuration exacte et son hash BLAKE3, generation RAG et hash
du manifeste, timestamp, duree et environnement OS/architecture sans chemin
personnel. Les sorties sont :

- rapport JSON complet ;
- resume Markdown avec tableau par strategie ;
- comparaison optionnelle a une baseline ;
- regressions et ameliorations au-dela des tolerances.

Les rapports sont places sous `benchmarks/runs/eval/`, ignore par Git.

## Commandes

Suite deterministe sans modele :

```powershell
.\target\release\opticcode.exe eval `
  --suite benchmarks/eval/context-retrieval-v1.json `
  --strategy legacy,symbol,exact `
  --no-rag
```

Inclure le RAG v2 actif :

```powershell
.\target\release\opticcode.exe eval `
  --suite benchmarks/eval/context-retrieval-v1.json `
  --strategy legacy,symbol,exact,rag `
  --rag-index data/index
```

Fixtures externes, toujours read-only :

```powershell
.\target\release\opticcode.exe eval `
  --external "pandaspigot=C:\path\to\PandaSpigot" `
  --external "kspawners=C:\path\to\Kspawners" `
  --strategy symbol,exact `
  --no-rag
```

Comparer deux rapports existants :

```powershell
.\target\release\opticcode.exe eval `
  --compare benchmarks/runs/eval/eval-baseline.json `
  --candidate benchmarks/runs/eval/eval-candidate.json `
  --json
```

Gate reproductible :

```powershell
.\scripts\run-eval-quality.ps1
.\scripts\run-eval-quality.ps1 -IncludeRag
```

## Limites honnetes

- Les temps sans modele dependent du materiel et du profil debug/release.
- Le RAG lexical v2 rescane encore son fichier de chunks. EVAL execute chaque cas
  separement afin que p50/p95 representent une vraie latence de requete, pas le
  debit artificiel d'un lot de prompts independants.
- Les faits et affirmations interdites sont evalues seulement lorsqu'une reponse
  existe. Qwen ne sera jamais son unique juge.
- `--with-llm` enregistre deja la configuration, mais la generation reelle est
  branchee dans CONTEXT-002 afin de garder les deux commits separes.
