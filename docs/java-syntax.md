# CODE-001 - Tree-sitter Java read-only

Date de validation initiale : 2026-07-13

## Objectif

CODE-001 introduit une comprehension syntaxique Java fiable avant toute
transformation automatique. Cette premiere etape est strictement read-only :
elle extrait symboles, references, zones non-code, diagnostics et positions sans
modifier les sources.

Le parseur textuel historique de `analyze-java` reste actif. La nouvelle commande
`java-syntax` permet de comparer les resultats avant une migration controlee.
Le schema syntaxique est en version 2 depuis l'ajout des usages de types et de
l'arite des appels necessaires a CODE-001B1.

## Dependances

```text
tree-sitter       0.26.11, MSRV Rust 1.77
tree-sitter-java  0.23.5
```

La toolchain locale Rust 1.94 est compatible. La grammaire Java est compilee
avec le build C standard de la crate ; aucun runtime C++ ou executable externe
n'est ajoute.

## Architecture

```text
opticcode-tools/src/java_syntax/
  mod.rs          schema, bornes et scan projet
  parser.rs       initialisation/reutilisation du parser
  symbols.rs      parcours AST, symboles et references
  diagnostics.rs  noeuds ERROR/MISSING structures
```

Le module `worktree.rs` reste independant. Les futurs edits syntaxiques seront
ajoutes sous `java_syntax`, jamais dans GIT-002.

## Commandes

Projet ou dossier :

```powershell
cargo run -q -- java-syntax --path C:\path\to\project --json
```

Fichier unique :

```powershell
cargo run -q -- java-syntax --path C:\path\to\Main.java --json
```

Bornes explicites :

```powershell
cargo run -q -- java-syntax `
  --path C:\path\to\project `
  --limit 500 `
  --max-file-bytes 2097152 `
  --item-limit 2000 `
  --json
```

## Donnees extraites

- package et imports, y compris `static` et wildcard ;
- classes, interfaces, enums, annotations-types et records ;
- methodes, constructeurs, champs et constantes enum ;
- modificateurs, annotations, types, parametres et signatures simples ;
- usages de types, appels de methode, acces de champ/enum, constructions,
  method references et annotations ;
- nombre d'arguments des appels et constructions, utilise ensuite pour les overloads ;
- commentaires ligne/bloc, chaines, caracteres et text blocks comme zones exclues ;
- noeuds syntaxiques `ERROR` et `MISSING` comme diagnostics ;
- hash BLAKE3 du contenu ;
- positions byte, ligne et colonne.

Toutes les positions sont zero-based et les colonnes Tree-sitter sont des
offsets en octets. Les ranges sont semi-ouverts : `start` inclus, `end` exclu.
Des tests avec CRLF et un identifiant UTF-8 place avant la cible verifient que
les ranges decoupent exactement les octets attendus. Ces ranges ne doivent
jamais etre convertis en indices de caracteres Rust.

Les declarations et references situees dans un noeud `ERROR` ne sont pas
emises. Les diagnostics `ERROR` et `MISSING` restent conserves. Un futur edit
devra refuser tout fichier dont `syntax_valid` est faux, meme si des portions
valides du fichier ont pu etre analysees.

## Anti-faux-positifs

Pour ce fichier :

```java
// Material.GUNPOWDER
String text = "Material.GUNPOWDER";
Object value = Material.GUNPOWDER;
```

le rapport contient :

- deux zones non-code ;
- une seule reference `field_access` qualifiee par `Material` ;
- aucune proposition d'edition.

Cela fournit la base necessaire pour remplacer plus tard les `String::replace`
globaux par des edits sur ranges AST verifies.

## Bornes

| Limite | Defaut | Maximum |
| --- | ---: | ---: |
| Fichiers Java | 500 | 5 000 |
| Taille par fichier | 2 Mio | 16 Mio |
| Items conserves par type et fichier | 2 000 | 20 000 |
| Avertissements projet | 100 | 100 |

Le JSON expose les limites effectives et distingue :

- `file_selection_truncated` pour la limite de fichiers ;
- `retained_items_truncated` pour les listes bornees dans au moins un fichier ;
- `warnings_truncated` pour la liste d'avertissements ;
- `truncated`, vrai si l'une de ces trois limites a ete atteinte.

`analysis_complete` n'est vrai que si tous les fichiers decouverts ont ete
parses sans skip, erreur de parcours/lecture, reparse point ou troncature.
`syntax_valid()` exige en plus l'absence de diagnostics syntaxiques. Cette
distinction evite d'annoncer qu'un projet est valide alors qu'un fichier n'a pas
pu etre lu.

Le scan :

- trie les chemins pour des rapports deterministes ;
- refuse une racine qui est elle-meme un symlink ou reparse point ;
- ne suit pas les symlinks, jonctions ou reparse points rencontres dans l'arbre ;
- ignore `.git`, `.idea`, `.gradle`, `.opticcode`, `target`, `build`, `out`,
  `bin`, `classes` et `node_modules` ;
- signale les fichiers trop gros, non UTF-8 et les erreurs de lecture ;
- relit chaque fichier avec une borne dure de `max_file_bytes + 1`, meme si sa
  taille change apres la lecture des metadonnees ;
- distingue le nombre de fichiers decouverts, selectionnes et parses ;
- accepte un chemin arbitraire explicitement fourni : la commande est read-only
  et n'impose pas que l'entree appartienne au depot OpticCode.

L'ordre des chemins et des items AST est deterministe. Les champs de duree sont
par nature variables et sont exclus des comparaisons de determinisme.

## Mesures locales

Build release, Windows 10, Ryzen 7 3700X :

| Source read-only | Decouverts | Parses | Erreurs | Symboles | References | Temps |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| mini Bukkit | 3 | 3 | 0 | 9 | 71 | 2,8 ms |
| Kspawners | 35 | 35 | 0 | 799 | 6 374 | 196 ms |
| PandaSpigot borne | 8 933 | 500 | 0 | 6 541 | 16 261 | 976 ms |

Ces chiffres incluent le scan, les lectures, le parse, l'extraction et la
construction du rapport Rust. Ils ne mesurent pas la serialisation JSON ni le
temps de chargement Cargo.

La mesure PandaSpigot porte sur les 500 premiers chemins tries parmi 8 933
fichiers decouverts. Elle valide un echantillon borne, pas encore un index
integral du fork.

Le binaire release mesure 6 068 736 octets, soit environ 5,79 Mio. Tree-sitter
ajoute approximativement 1,5 Mio par rapport a la baseline d'audit ; ce cout est
acceptable pour le gain syntaxique et reste a surveiller lors des futurs langages.

## Tests

- extraction package/imports/classes/methodes/champs/enums ;
- annotations, appels, constructions et positions ;
- commentaires, chaines et caracteres exclus des references code ;
- text blocks classes comme zones exclues par inspection de leurs octets ;
- ranges exactes avec UTF-8 et CRLF ;
- Java incomplet retourne un arbre et des diagnostics structures ;
- noeuds `ERROR` et `MISSING` testes separement ;
- classes imbriquees, overloads et classe anonyme sans crash ;
- jonction Windows ignoree dans l'arbre et refusee comme racine ;
- fichier non UTF-8 signale explicitement ;
- limites et ordre JSON deterministe hors champs de duree ;
- commande CLI JSON read-only ;
- limite de fichiers deterministe ;
- mini Bukkit, Kspawners et PandaSpigot analyses sans ecriture.

Validation cible apres ce sprint :

```text
cargo fmt --all -- --check                       OK
cargo clippy --workspace ... -D warnings         OK
cargo test --workspace                          120 tests OK
cargo build --workspace --release                OK
```

## Limites assumees

- Tree-sitter fournit une structure syntaxique, pas une resolution semantique.
- CODE-001B1 resout maintenant les imports et certains overloads de facon
  conservatrice, mais pas l'heritage complet ni l'inference generique.
- Les references sont indexees entre fichiers en memoire, sans cache persistant.
- Le parser accepte aussi des syntaxes Java modernes ; le profil Java 8 devra
  continuer a signaler ce qui est interdit pour Minecraft 1.8.8.
- Aucun cache incremental par hash n'est encore persiste.
- Aucun edit ni patch n'est produit par ce module.

## Suite

La suite est volontairement separee en deux sous-sprints :

1. `CODE-001B1` : termine ; index symbolique inter-fichiers strictement
   read-only, identites stables, overloads et imports.
2. `CODE-001B2` : termine ; editions legacy read-only avec hash, octets
   attendus, refus des overlaps, application simulee et reparse.
3. `CODE-001B3` : termine ; revalidation et apply dans un worktree, puis build borne.

`CONTEXT-001` utilisera ensuite l'index B1 pour selectionner le contexte selon
la tache. Voir [`java-index.md`](java-index.md) et
[`java-edits.md`](java-edits.md) et
[`java-edit-worktree.md`](java-edit-worktree.md).
