# CONTEXT-001 - Contexte Java guide par les symboles

## Statut

CONTEXT-001 est implemente et valide comme outil read-only independant. Il
n'est pas encore injecte dans `ask` ou `plan` : le benchmark actuel mesure la
selection et la taille du prompt, pas la qualite des reponses de Qwen.

Commande principale :

```powershell
cargo run -q -- java-context "dev.opticcode.util.Helpers#create(String)" `
  --path benchmarks/java-index-mini --compare-baseline --json
```

Gate reproductible :

```powershell
powershell -ExecutionPolicy Bypass -File scripts/run-java-context-quality.ps1
```

## Pipeline

```text
demande utilisateur
  -> normalisation et termes bornes
  -> index Java Tree-sitter CODE-001B1
  -> exclusion des fichiers/contextes syntaxiques invalides
  -> score explicable des declarations
  -> choix des symboles principaux
  -> relations exactes a un niveau (declarations, appelants, references)
  -> snippets sur ranges AST avec cache de lecture par fichier
  -> ajout conditionnel de pom.xml ou plugin.yml
  -> enforcement octets/caracteres/tokens
  -> rapport humain ou JSON plat et deterministe
```

Le moteur ne modifie aucun fichier. Les chemins passent par les controles de la
baseline Java : racine liee refusee, entrees symlink/jonction/reparse ignorees,
fichier relu sous la racine canonique, taille bornee et hash BLAKE3 compare a
l'index avant de materialiser un snippet.

## Selection

Les scores privilegient, dans cet ordre, l'identifiant symbolique complet, le
nom qualifie, la signature, le nom simple, puis les termes et references. Une
correspondance exacte est bornee comme un token symbolique : une classe n'est
donc plus favorisee simplement parce que son nom est le prefixe d'une methode.

Une demande exacte comme `Helpers#create(String)` distingue l'overload de
`create()`. Un nom simple present dans plusieurs packages, comme `Duplicate`,
retourne plusieurs `primary_symbols` et `primary_ambiguous=true`. Une reference
non resolue peut selectionner sa declaration englobante utile, mais ne cree
jamais une declaration fictive.

Les relations utilisent uniquement les resolutions `exact` de l'index. V1 suit
un niveau avec un ensemble de symboles deja visites. Si une relation connue
existe au niveau suivant, `truncation.relation_depth` le rend visible.

`pom.xml` n'est selectionne que pour une demande build/Maven/Gradle/dependance.
`plugin.yml` n'est selectionne que pour une demande Bukkit/plugin/commande ou
permission. Ils ne sont plus ajoutes comme bruit a une demande source precise.

## Budgets par defaut

| Ressource | Limite |
|---|---:|
| Fichiers Java candidats | 500 |
| Taille d'un fichier Java | 2 Mio |
| Diagnostics par type et fichier | 2 000 |
| Avertissements exposes | 128 |
| Symboles visites | 100 000 |
| Symboles principaux | 8 |
| Candidats exposes | 64 |
| Score minimal d'un candidat | 300, sauf relation explicite |
| Raisons de score par candidat | 16 |
| Relations suivies | 256 |
| Profondeur des relations | 1 |
| Appelants par symbole | 4 |
| Symboles relies | 8 |
| Snippets | 12 |
| Raisons de selection par snippet | 4 |
| Taille d'un snippet | 6 Kio |
| Contexte rendu | 24 Kio |
| Caracteres rendus | 24 576 |
| Tokens estimes | 6 144 |

Le JSON expose les limites, compteurs, elements ignores et champs de
troncature separes. Aucun budget n'est depasse silencieusement. Les snippets
contiennent leur score, raisons, symbole, fichier, range AST, range retenue,
hash, octets, caracteres, tokens estimes et etat de troncature.

Deux notions sont volontairement distinctes :

- `analysis_complete` signifie que source, index, requete et graphe demande ne
  contiennent aucune omission connue susceptible de changer l'analyse ;
- `selection_complete` exige en plus qu'aucune limite d'affichage ou de prompt
  n'ait retire de candidat, snippet ou avertissement.

Ainsi un resultat peut rester utile tout en annoncant honnetement une relation
de second niveau non exploree.

## Benchmark

Le corpus `benchmarks/java-context/tasks.json` couvre cinq demandes : overload
et appelant, nom ambigu, descriptor Bukkit, manifeste Maven et symbole non
resolu. La gate exige tous les symboles/roles attendus et zero chemin ou symbole
interdit.

Mesure du 2026-07-13 avec le binaire release local :

| Demande | Fichiers | Snippets | Tokens contexte | Tokens baseline |
|---|---:|---:|---:|---:|
| Overload et appelant | 2 | 4 | 248 | 828 |
| Nom ambigu | 4 | 4 | 228 | 828 |
| `plugin.yml` | 1 | 1 | 84 | 828 |
| `pom.xml` | 1 | 1 | 183 | 828 |
| Symbole non resolu | 6 | 7 | 463 | 828 |
| **Total** | - | - | **1 206** | **4 140** |

Reduction mesuree : **70,87 % de tokens estimes** face a
`legacy_file_priority_v1`. L'estimation est `ceil(caracteres Unicode / 4)` :
elle est stable pour comparer les prompts, mais ne remplace pas le tokenizer de
Qwen. Cette mesure ne prouve pas encore une hausse de qualite LLM.

## Incident de reprise

La coupure electrique est survenue pendant le diagnostic d'un stack overflow.
L'audit a confirme que Git, le commit LEGACY-002 et les sources CONTEXT-001
etaient intacts.

Le crash ne venait pas du graphe, de Serde ou des snippets. Le vrai binaire
debug debordait dans `Cli::parse()` avant toute execution de commande, tandis
que les tests de bibliotheque et le binaire release fonctionnaient. L'ajout de
18 options au derive Clap monolithique avait fait depasser la pile principale
Windows en build debug.

`java-context` conserve son nom public, mais ses options sont parsees par un
sous-parseur isole avant le grand enum. Aucun agrandissement de pile n'est
utilise. Un test d'integration lance le vrai binaire debug sur `--help` et sur
la serialisation JSON ; un second test analyse 400 fichiers.

## Limites restantes

- resolution inferieure a `javac` sans classpath complet, heritage ni generiques ;
- graphe volontairement limite a un niveau dans CONTEXT-001 ;
- index reconstruit en memoire, sans cache incremental ;
- estimation de tokens generique, pas tokenizer Qwen exact ;
- pas encore de benchmark de qualite de reponse avec le modele local ;
- pas encore d'integration dans `ask`, `plan` ou la boucle agent.

La suite recommandee est CONTEXT-002 : integration optionnelle et mesurable
dans `ask`/`plan`, comparaison A/B sur les memes prompts, puis activation par
defaut seulement si pertinence, latence et qualite LLM sont validees.
