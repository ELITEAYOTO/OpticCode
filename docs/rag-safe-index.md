# OpticCode - RAG-SAFE-001

Statut : implemente et valide localement le 2026-08-03.

## Objectif

RAG-SAFE-001 remplace l'index JSONL permissif par une ingestion locale
fail-closed. Un fichier inconnu, ambigu, lie, trop gros, modifie pendant sa
lecture ou susceptible de contenir un secret est exclu. Aucun futur agent ne
dispose d'un override automatique pour ces regles.

Le sprint ne contient ni SQLite, ni Tantivy, ni embeddings, ni Qdrant. La
recherche reste lexicale afin d'isoler le changement de securite et de format.

## Risques reproduits avant correction

Le binaire V1 a ete execute sur une fixture locale sans vraie valeur sensible.
Il considerait indexables les cinq fichiers testes : `.env`, `credentials`,
un `README` sans extension, un YAML avec password et un fichier Java sain. Il
ecrivait directement les deux JSONL actifs, sans manifeste ni publication
atomique. Un dossier legacy incomplet pouvait egalement etre lu comme un index
valide. Le document pentest local etait indexable avec cette ancienne politique;
il a ete deplace dans les notes ignorees `Idees-Vrac` avec hash identique, puis
explicitement exclu du nouveau corpus.

## Pipeline

```text
racines explicitement autorisees
  -> canonicalisation et controle reparse
  -> parcours determine sans suivre les liens
  -> denylist globale
  -> allowlist d'extensions
  -> limite de taille
  -> lecture coherente et hash BLAKE3
  -> detection bornee des secrets
  -> documents/chunks dans une generation temporaire
  -> validation complete
  -> generation immuable
  -> remplacement atomique de CURRENT
```

Une racine doit etre fournie explicitement a `rag-scan` ou `rag-index`. Les
racines dupliquees ou imbriquees sont refusees. Chaque chemin canonique doit
rester sous sa racine et chaque composant doit etre un vrai fichier ou dossier,
pas un symlink, une jonction ou un reparse point.

## Politique fail-closed

Extensions autorisees :

```text
gradle, groovy, java, json, kt, kts, lang, mcmeta, md, patch,
properties, rs, toml, txt, xml, yaml, yml
```

Un fichier sans extension est refuse. Les dossiers de depot, build,
dependances, caches, modeles, donnees generees et notes privees sont ignores,
notamment `.git`, `.opticcode`, `.gradle`, `.idea`, `.vscode`, `target`,
`build`, `node_modules`, `.m2`, `.npm`, `.ssh`, `models`, `data` et
`Idees-Vrac`/`Idées-Vrac`. Les sorties generees sous `benchmarks/runs` sont
egalement exclues sans ignorer les fixtures versionnees de `benchmarks/`.

Les noms suivants sont bloques avant lecture :

- `.env` et toutes ses variantes ;
- `credentials`, `secrets`, `tokens`, comptes de service ;
- `.npmrc`, `.pypirc`, `.netrc`, `_netrc`, `auth.json` ;
- cles SSH usuelles ;
- `.pem`, `.key`, `.p12`, `.pfx`, `.jks`, `.keystore`, `.kdbx`.

## Detection sensible

Les contenus sont limites a 512 Kio et examines sans journaliser la valeur.
Les regles bloquantes couvrent :

| Rule ID | Categorie |
| --- | --- |
| `secret.private_key` | marqueur de cle privee PEM/SSH |
| `secret.github_token` | token GitHub structurel |
| `secret.aws_access_key` | identifiant de cle AWS |
| `secret.openai_token` | token OpenAI structurel |
| `secret.huggingface_token` | token Hugging Face structurel |
| `secret.gitlab_token` | token GitLab structurel |
| `secret.uri_credentials` | URI avec utilisateur et mot de passe |
| `secret.credential_assignment` | affectation password/secret/token/API key |

Une exclusion ne contient que collection, source, chemin relatif, type
d'entree, rule ID, categorie, decision et eventuelle position ligne/colonne.
Les references explicites comme `${DB_PASSWORD}` et `<injected-at-runtime>`
restent autorisees. Le simple mot `password` dans une phrase ne suffit pas a
bloquer un document.

## Format v2

```text
data/index/
  CURRENT
  generations/
    g-<id>/
      manifest.json
      documents.jsonl
      chunks.jsonl
```

`CURRENT` contient uniquement l'identifiant valide de la generation active.
Le manifeste `schema_version: 2` contient : generation, date Unix, version
OpticCode, hash de configuration, politique, collections, sources logiques,
comptes, exclusions, catalogue de regles, metriques et hashes des deux JSONL.

Chaque document et chunk contient collection, profil, source logique,
`source_kind`, chemin relatif portable, type, BLAKE3, regle d'autorisation,
raison d'inclusion et date source lorsqu'elle est disponible. Aucun chemin
absolu de racine n'est stocke dans l'index portable.

## Publication et recovery

Les JSONL sont ecrits et synchronises dans `.staging-<generation>`. OpticCode
reparse chaque record, controle les IDs, relations document/chunk, ordre des
chunks, hashes BLAKE3, provenance, politique et absence de secret. Le manifeste
complet est ensuite ecrit et relu.

La generation valide est renommee sous `generations/`, puis un fichier pointeur
temporaire est synchronise. Sous Windows, `MoveFileExW` avec remplacement et
`WRITE_THROUGH` publie `CURRENT`. Une panne avant ce remplacement laisse donc
l'ancienne generation active. Au lancement suivant, les staging tronques et
temporaires de pointeur strictement reconnus sont supprimes; les generations
finalisees non actives sont conservees pour ne pas detruire de donnees ambiguës.

Les anciens `documents.jsonl`/`chunks.jsonl` a la racine ne sont jamais lus
comme du v2. La recherche demande explicitement une reconstruction. Une
reconstruction dans le meme dossier conserve les fichiers legacy mais publie
une nouvelle generation v2 via `CURRENT`.

## CLI

```powershell
cargo run -q -- rag-scan --path . --limit 20 --json
cargo run -q -- rag-index --path . --output data/index --chunk-chars 4000 --json
cargo run -q -- rag-search "nether wart" --index data/index --limit 5 --json
cargo run -q -- rag-debug "legacy spawner" --index data/index --limit 3 --json
```

Les options `--json` sont facultatives et les sorties humaines historiques
restent disponibles.

Les recherches elargies de `rag-debug`, `ask` et `plan` utilisent une seule
lecture securisee de l'index pour toutes les variantes de requete. Hash,
provenance et detection sensible ne sont donc pas recalcules quatorze fois sur
le meme fichier.

## Tests et mesure

```powershell
.\scripts\run-rag-safe-quality.ps1 -SearchIterations 10
```

Le script genere uniquement des secrets factices invalides dans
`benchmarks/runs`, execute les tests unitaires et CLI, construit deux index,
compare les JSONL, recherche, teste `rag-debug` et refuse un index legacy.

Dernier resultat fixture du 2026-08-03 : 3 documents, 3 chunks, 3 exclusions,
construction 70,292 ms, recherche moyenne 12,572 ms sur dix executions et
`rag-debug` batche 14,661 ms. La detection sensible representait 2 063 us,
soit 19,282 % du temps interne de scan sur cette fixture minuscule; ce ratio
n'est pas extrapolable a un gros corpus.

Mesure du corpus local historique a six sources :

| Mesure | V1 permissive | V2 fail-closed |
| --- | ---: | ---: |
| Documents actifs | 2 651 | 1 141 |
| Chunks | 5 063 | 3 762 |
| Octets indexes | non mesure ici | 12 444 900 |
| Exclusions explicites | aucune trace complete | 3 887 |
| Reconstruction murale | 25,9 s sur la passe d'audit | 2,299 s |
| Recherche simple mediane | 63,75 ms | 317,508 ms |
| Debug legacy median | 687,3 ms | 403,064 ms |

La comparaison de reconstruction n'est pas un microbenchmark strict : la V2
refuse davantage de bruit et indexe donc moins de donnees. Son scan sensible a
pris 128,848 ms, soit 5,60 % des 2,299 s murales. La recherche simple paie
environ 254 ms pour parser, verifier, hasher et rescanner 12,4 Mo avant de
retourner un resultat. En revanche, le batch multi-requetes a fait passer le
premier prototype securise de `rag-debug` de 4 326,391 ms a 403,064 ms medians
(-90,68 %, 10,73x), et le rend 41,35 % plus rapide que le debug V1 mesure.

## Limites restantes

- Les generations anciennes ne sont pas encore soumises a une retention.
- La detection vise les secrets manifestes, pas toute chaine a forte entropie.
- Le contenu de l'index local n'est pas chiffre au repos.
- L'indexation reste complete et une commande de recherche reparcourt le JSONL.
- Une recherche simple est plus lente que la V1 car elle revalide l'index; un
  cache de generation dans le futur daemon devra supprimer ce cout repete sans
  affaiblir le fail-closed.
- Les liens avec l'index symbolique Java restent a faire dans RAG-002.
- CONTEXT-002 devra mesurer ce qui est reellement transmis au modele.
