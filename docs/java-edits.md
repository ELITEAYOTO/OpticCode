# CODE-001B2 - Propositions d'edits Java ciblees

Date de validation : 2026-07-13

## Resultat

CODE-001B2 transforme une resolution Java B1 suffisamment prouvee en une
proposition d'edition read-only. Il ne modifie aucun fichier, ne lance aucun
build et ne transfere rien vers le projet source.

La commande publique est :

```powershell
cargo run -q -- java-edits --path C:\path\to\project
cargo run -q -- java-edits --path C:\path\to\project --json
```

Le premier jeu de regles cible les noms Bukkit 1.8.8 documentes dans
[`minecraft-legacy-rules.md`](minecraft-legacy-rules.md) : `Material.SULPHUR`,
`NETHER_STALK`, `MOB_SPAWNER`, `MONSTER_EGG`, les pelles `*_SPADE` et trois
anciens noms de `EntityType`.

## Architecture

```text
java_edits/
  mod.rs         orchestration et politique fail-closed
  legacy.rs      26 regles versionnees, preuves et identites Bukkit canoniques
  schema.rs      contrat JSON, compteurs, rejets et validations
  validation.rs  relecture sure, garde anti-shadow, ranges et reparse
```

La table de regles est partagee avec le generateur legacy historique. Il n'y a
donc plus deux listes independantes susceptibles de diverger.

## Pipeline

```text
sources Java read-only
-> Tree-sitter et index inter-fichiers B1
-> references ressemblant a une regle legacy
-> resolution exacte vers l'identite Bukkit attendue
-> preuve du qualificateur/import
-> relecture bornee et comparaison BLAKE3
-> verification du noeud et des octets attendus
-> detection des shadows et ranges chevauchantes
-> simulation des remplacements de la fin vers le debut
-> reparse Tree-sitter en memoire
-> propositions JSON compactes
```

Chaque fichier candidat n'est relu qu'une fois. Seuls les fichiers contenant
une cible potentielle sont reparses apres simulation.

## Conditions d'eligibilite

Une edition n'est proposee que si toutes les conditions suivantes sont vraies :

1. la reference est un acces de champ ou constante enum Tree-sitter ;
2. son membre correspond exactement a une regle versionnee ;
3. B1 retourne `exact`, jamais `unique_candidate`, `ambiguous`, `unresolved` ou
   `invalid_syntax_context` ;
4. `target_id` correspond a l'identite complete, par exemple
   `org.bukkit.Material#GUNPOWDER` ;
5. le qualificateur est pleinement qualifie ou vient d'un import explicite ;
6. aucune variable, parametre, type parameter ou import statique connu ne
   masque la racine du qualificateur ;
7. le fichier relu possede toujours le hash BLAKE3 produit par l'index ;
8. le range du membre est UTF-8, non vide, dans le noeud et contient les octets
   attendus ;
9. aucun range ne chevauche une autre edition ;
10. le resultat simule est toujours syntaxiquement valide apres reparse.

Cette politique a deja evite une vraie erreur sur Kspawners :
`CreatureSpawnEvent.SpawnReason.SPAWNER` ressemble a la regle moderne
`Material.SPAWNER`, mais son `target_id` est different. Le moteur le classe
`wrong_target` et ne propose rien.

## Contrat d'une proposition

Le schema `java_edit_proposals` est en version 2. Une proposition contient :

- un identifiant stable derive de la regle, du chemin, du hash et du range ;
- `rule_id`, fichier relatif, hash source et reference B1 ;
- `target_id` et raison de resolution ;
- type de reference, range du noeud et range exact a remplacer ;
- contenu complet attendu du noeud et octets attendus du membre ;
- remplacement, raison metier et confiance `syntax_exact`.

Le rapport ne stocke ni source complete ni gros unified diff. Pour chaque
fichier modifie en memoire, il conserve les hashes avant/apres, tailles, IDs
d'edits et resultat du reparse. Cela borne la sortie JSON.

## Etats et rejets

`analysis_complete=true` signifie que l'index est complet et qu'aucun probleme
structurel n'a ete rencontre. `safe_to_apply=true` signifie seulement que les
propositions peuvent etre transmises a un futur adaptateur qui reverifiera
toutes les preconditions. La commande reste strictement read-only.

Rejets attendus qui ne bloquent pas les autres propositions :

- resolution non exacte ;
- cible homonyme mais differente ;
- qualificateur non prouve ;
- qualificateur masque dans le fichier.

Rejets fail-closed qui rendent le rapport non applicable :

- source illisible ou modifiee apres indexation ;
- range invalide ou chevauchant ;
- syntaxe invalide apres simulation ;
- limite de propositions atteinte.

Les details de rejet sont bornes a 256, tandis que les compteurs restent
complets. Une troncature de fichiers, symboles ou references dans B1 interdit
l'application. Une simple troncature de liste de candidats deja classee
ambigue reste visible sans inventer de cible.

## Securite Windows et concurrence

- racine symlink, jonction ou reparse point refusee par la couche Java ;
- chaque composant du chemin relu est reinspecte ;
- chemin relatif strict et cible canonique exigee sous la racine ;
- lecture bornee a `max_file_bytes + 1` ;
- hash et octets attendus obligatoires ;
- aucune ouverture en ecriture.

Le hash detecte une derive entre l'index et la proposition. Le futur adaptateur
APPLY devra refaire cette verification juste avant l'ecriture : CODE-001B2 ne
pretend pas supprimer toute fenetre TOCTOU a lui seul.

## Qualite mesuree

Corpus versionne `benchmarks/java-edits-legacy` :

| Mesure | Resultat |
| --- | ---: |
| fichiers Java | 13 |
| references examinees | 113 |
| noms ressemblant aux regles | 39 |
| cibles Bukkit exactes | 30 |
| propositions valides | 28 dans 3 fichiers |
| rejets attendus | 11 |
| faux positifs | 0 |
| regles couvertes | 26/26 |

Le corpus contient plus de 70 occurrences positives ou negatives : imports
explicites, noms pleinement qualifies, types custom, imports ambigus, shadows,
commentaires, chaines, symboles deja legacy, imports statiques et proprietaires
homonymes. Dans ce corpus controle, precision et rappel des 28 edits attendus
sont de 100 %. Ce chiffre ne vaut pas encore evaluation generale de Java.

Mesures release locales, Ryzen 7 3700X, Windows 10 :

| Source | References | Candidats | Propositions | Resultat | Temps |
| --- | ---: | ---: | ---: | --- | ---: |
| corpus LEGACY-002 | 113 | 39 | 28 | complet, safe | 8,7 a 9,6 ms chaud |
| Kspawners | 6 374 | 1 | 0 | cible `SpawnReason`, correctement refusee | 199 ms chaud |
| PandaSpigot borne | 16 261 | 0 | 0 | 500/8 933, fail-closed | 1,02 s |

Les temps dependent du cache disque et servent de baseline. PandaSpigot complet
reste bloque par la limite B1 de 5 000 fichiers ; la bonne reponse sera un index
incremental, pas une allocation monolithique plus grande.

## Tests

Le script dedie execute formatage, Clippy strict et tests cibles :

```powershell
.\scripts\run-java-edits-quality.ps1
.\scripts\run-java-edits-quality.ps1 -Full
```

La couverture inclut :

- 26 regles et IDs uniques, avec sources et niveaux de preuve ;
- exact, ambigu, mauvaise cible et shadow lexical ;
- commentaires/chaines sans faux positif ;
- hash, contenu attendu, UTF-8 et ranges imbriques ;
- overlap, application inverse et reparse ;
- limite fail-closed et sortie deterministe ;
- vraie CLI humaine/JSON ;
- corpus byte-identique avant/apres ;
- jonction Windows refusee ;
- Java invalide non applicable.

## Limites assumees

- Tree-sitter et B1 ne remplacent pas `javac`, le classpath ou l'heritage ;
- un membre herite portant le nom du qualificateur ne peut pas toujours etre
  prouve sans analyse semantique ;
- les imports wildcard et constantes statiques non qualifiees ne sont pas
  edites dans cette version ;
- seules les 26 regles deterministes Bukkit 1.8 sont supportees ;
- metadata/data values/NBT des blocs et spawn eggs restent hors scope ;
- `patch`, `apply` et `worktree-verify` conservent leur adaptateur legacy
  historique pour compatibilite ; `java-edits-verify` utilise le contrat AST ;
- aucune promotion vers un projet source n'existe.
- le pic memoire n'est pas encore mesure par un benchmark robuste ; il reste une
  metrique obligatoire avant l'index incremental grande echelle.

## Suite

`CODE-001B3` est maintenant disponible via `java-edits-verify`. Il recalcule le
contrat dans un worktree GIT-002, rematerialise hash/ranges/octets, utilise
APPLY-001, reparse, compile puis verifie les hashes Git finaux. La source reste
inchangee et aucune promotion automatique n'est ajoutee. Voir
[`java-edit-worktree.md`](java-edit-worktree.md).

`CONTEXT-001` utilise maintenant cet index symbolique pour reduire les fichiers
et tokens envoyes au modele ; voir [`java-context.md`](java-context.md). La
prochaine etape est son integration A/B dans `ask` et `plan` (`CONTEXT-002`).
