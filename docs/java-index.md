# CODE-001B1 - Index symbolique Java inter-fichiers

Date de validation : 2026-07-13

## Objectif

CODE-001B1 transforme les rapports Tree-sitter read-only de CODE-001 en un
index deterministe de declarations et de references Java. Il relie les fichiers
d'un projet sans modifier les sources et sans pretendre remplacer `javac`.

Cette brique sert a :

- identifier une classe, une methode surchargee, un champ ou une constante enum ;
- expliquer comment une reference a ete resolue ;
- conserver explicitement les cas ambigus ou non resolus ;
- preparer la selection de contexte et les futurs edits syntaxiques cibles.

## Architecture

```text
java_syntax/
  parser.rs       parse Tree-sitter reutilise
  symbols.rs      declarations, references, ranges et arites

java_index/
  mod.rs          orchestration, limites et metriques
  declarations.rs identifiants, signatures et visibilite
  imports.rs      contexte package/imports par fichier
  resolver.rs     resolution conservatrice et bornee
  schema.rs       contrat JSON versionne
```

Chaque fichier n'est parse qu'une fois pendant une execution. L'index reste en
memoire dans B1 : SQLite, Tantivy et le cache persistant sont volontairement
reportes jusqu'a l'obtention d'une baseline correcte.

## Commandes

```powershell
cargo run -q -- java-index --path C:\path\to\project
cargo run -q -- java-index --path C:\path\to\project --json
```

Bornes explicites :

```powershell
cargo run -q -- java-index `
  --path C:\path\to\project `
  --limit 500 `
  --max-file-bytes 2097152 `
  --item-limit 2000 `
  --symbol-limit 100000 `
  --reference-limit 200000 `
  --candidate-limit 16 `
  --json
```

Script de qualite :

```powershell
.\scripts\run-java-index-quality.ps1
.\scripts\run-java-index-quality.ps1 -Full
```

## Schema

Le schema `java_index` est en version 1 et reference le schema syntaxique en
version 2. Le rapport contient notamment :

- racine, entree et hashes BLAKE3 des sources ;
- packages, fichiers et imports explicites/wildcard/statiques ;
- declarations, identifiants stables, ranges d'octets et visibilite ;
- references, conteneur, qualificateur et nombre d'arguments lorsqu'il existe ;
- statut, cible eventuelle, candidats, origine et raison de resolution ;
- compteurs, limites, troncatures, avertissements et metriques par phase.

Exemples d'identifiants :

```text
dev.opticcode.Plugin
dev.opticcode.Plugin.Inner
dev.opticcode.Plugin#onEnable()
dev.opticcode.Plugin#createSpawner(Player,Location)
dev.opticcode.Plugin#<init>()
org.bukkit.Material#GUNPOWDER
```

Les overloads sont distingues par leur signature syntaxique. Une signature
incomplete conserve `?` et `signature_complete=false` au lieu d'inventer un
type.

## Regles de resolution

La resolution s'arrete au premier niveau de priorite qui produit des candidats :

1. type courant, types englobants et types imbriques ;
2. meme package ;
3. import explicite ;
4. nom pleinement qualifie ;
5. imports wildcard et allowlist `java.lang` ;
6. declarations globales portant le meme nom simple.

Pour les membres, l'index utilise aussi le proprietaire qualifie, l'arite des
appels et les imports statiques explicites ou wildcard. L'arite syntaxique
elimine les overloads dont le nombre de parametres est incompatible, sans
pretendre resoudre leurs types. Un import statique ne
peut cibler qu'un membre `static`; les constantes enum sont traitees comme
statiques meme si aucun modificateur n'apparait dans leur declaration.

Les statuts sont :

| Statut | Signification |
| --- | --- |
| `exact` | chemin prouve syntaxiquement ou declaration unique a un niveau exact |
| `unique_candidate` | un seul candidat plausible, mais informations semantiques incompletes |
| `ambiguous` | plusieurs candidats restent possibles ; aucune cible n'est choisie |
| `unresolved` | aucun candidat suffisamment justifie |
| `invalid_syntax_context` | le fichier contient un diagnostic ERROR/MISSING |

Une classe externe explicitement importee peut produire un chemin exact, par
exemple `org.bukkit.Material#GUNPOWDER`. Une methode externe surchargee reste
`unique_candidate`, car son overload n'est pas connu sans indexer le JAR.

Les candidats sont tries et bornes. Le resolver examine au maximum
`candidate-limit + 1` candidats par chemin : cela suffit pour conserver la
decision unique/ambigue et savoir que la liste d'affichage est tronquee, sans
allouer une liste proportionnelle a tout PandaSpigot.

## Bornes et completude

| Limite | Defaut | Maximum |
| --- | ---: | ---: |
| fichiers Java | 500 | 5 000 |
| taille par fichier | 2 Mio | 16 Mio |
| items par type et fichier | 2 000 | 20 000 |
| declarations | 100 000 | 1 000 000 |
| references | 200 000 | 2 000 000 |
| candidats affiches par reference | 16 | 256 |

`analysis_complete` exige une lecture source complete, zero fichier syntaxiquement
invalide et aucune troncature de fichiers, declarations ou references. Une liste
de candidats tronquee ne rend pas l'analyse fausse : le resolver a deja prouve
qu'il existe plusieurs candidats. Le rapport conserve toutefois
`truncation.candidates=true` et le nombre de listes concernees.

## Robustesse

- aucune ecriture dans les projets analyses ;
- lecture bornee a `max_file_bytes + 1` ;
- refus d'une racine symlink, jonction ou reparse point ;
- liens rencontres pendant le parcours ignores et signales ;
- offsets en octets verifies avec UTF-8 et CRLF ;
- commentaires, chaines, caracteres et text blocks exclus ;
- references d'un fichier syntaxiquement invalide marquees fail-closed ;
- ordre JSON deterministe hors champs de duree ;
- fichiers non UTF-8 et erreurs de lecture visibles dans le rapport.

## Mesures locales

Build release, Windows 10, Ryzen 7 3700X, index read-only :

| Source | Fichiers parses | Declarations | References | Resolution | Temps total |
| --- | ---: | ---: | ---: | --- | ---: |
| corpus B1 | 10/10 | 30 | 27 | 19 exactes, 5 uniques, 2 ambigues, 1 non resolue | 3,7 ms |
| mini Bukkit | 3/3 | 9 | 71 | 42 exactes, 6 uniques, 23 non resolues | 3,6 ms |
| Kspawners | 35/35 | 799 | 6 374 | 2 448 exactes, 2 010 uniques, 180 ambigues, 1 736 non resolues | 207 ms |
| PandaSpigot borne | 500/8 933 | 6 541 | 16 261 | 10 232 exactes, 2 700 uniques, 2 176 ambigues, 1 153 non resolues | 808 ms |
| PandaSpigot etendu | 5 000/8 933 | 72 525 | 409 180 | 68 679 exactes, 40 786 uniques, 279 508 ambigues, 20 207 non resolues | 9,49 s |

Le chemin PandaSpigot 5 000 fichiers, lance avec `--reference-limit 500000`,
prenait environ 53,9 s avant le bornage des candidats et prend 9,49 s apres,
soit environ 5,7 fois plus rapide. La grande
quantite d'ambiguites est attendue : B1 ne connait ni les types des variables,
ni l'heritage complet, ni le classpath compile.

Le depot complet compte 8 933 fichiers et depasse la borne dure actuelle de
5 000. Augmenter arbitrairement cette borne n'est pas retenu : l'etape suivante
pour cette echelle est un index incremental/pagine par hash, pas une allocation
monolithique plus grande.

Les temps varient selon le cache disque et l'activite de la machine. Ils servent
de baseline, pas de garantie contractuelle.

## Tests couverts

- meme package, imports explicites, wildcard et statiques ;
- noms pleinement qualifies et types `java.lang` ;
- classes imbriquees, constructeurs et methodes surchargees ;
- doublons de nom simple, ambiguite et absence de resolution ;
- constantes enum et chemins Bukkit externes ;
- commentaires et chaines sans faux positif ;
- ERROR/MISSING en mode fail-closed ;
- UTF-8, CRLF et chemin contenant des espaces ;
- limites de symboles, references et candidats ;
- stress de nombreux imports wildcard ;
- JSON deterministe et commande CLI reelle ;
- racine jonction Windows refusee ;
- verification que les sources restent identiques.

## Limites assumees

B1 ne resout pas completement :

- l'inference generique et les conversions Java ;
- le type runtime d'une variable et le dispatch dynamique ;
- l'heritage, les interfaces et les membres herites ;
- les dependances JAR non indexees ;
- Lombok, annotation processors et code genere ;
- les regles de visibilite entre modules ;
- toutes les subtilites d'overload de `javac`.

`ambiguous` et `unresolved` sont donc des resultats normaux. B2 devra rester
read-only pour ces statuts et ne produire un edit que sur une cible suffisamment
prouvee, avec hash source, octets attendus, reparse et verification en worktree.

## Suite

`CODE-001B2` fournit maintenant des propositions d'edits sur ranges AST avec
hash, octets attendus, garde anti-shadow et reparse, sans ecriture directe. Voir
[`java-edits.md`](java-edits.md). `CODE-001B3` les verifiera dans un worktree,
puis `CONTEXT-001` utilisera les symboles et resolutions de B1 pour selectionner
moins de fichiers et envoyer moins de tokens au modele local.
