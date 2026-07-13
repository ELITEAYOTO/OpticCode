# Verification des edits Java en worktree

## Statut

`CODE-001B3` est implemente.

La commande `java-edits-verify` relie maintenant les propositions AST de
CODE-001B2, l'apply transactionnel APPLY-001 et le worktree jetable GIT-002.
Elle ne transfere jamais le resultat vers le projet original.

```powershell
cargo run -q -- java-edits-verify `
  --path C:\path\to\clean-git-project `
  --timeout-seconds 600 `
  --git-timeout-seconds 180 `
  --output-limit-bytes 1048576 `
  --json
```

Le chemin doit etre un projet Git propre contenant un projet Maven ou Gradle
compilable. Un resultat sans proposition retourne `no_changes` sans creer de
worktree et sans lancer de build.

## Pipeline reel

```text
projet source read-only
  -> analyse B2 complete et non tronquee
  -> empreinte du contrat de propositions
  -> source Git propre + HEAD exact
  -> worktree Git detache sous %TEMP%
  -> nouvelle analyse B2 depuis le worktree
  -> comparaison exacte des deux contrats
  -> relecture hash/ranges/octets/overlaps
  -> application virtuelle en ordre inverse
  -> reparse Tree-sitter en memoire
  -> FileMutation attendues avant/apres
  -> transaction APPLY-001 dans le worktree
  -> verification byte-identique apres ecriture
  -> reparse Tree-sitter apres ecriture
  -> build Maven/Gradle borne
  -> Git State Guard strict pendant le build
  -> validation des hashes dans le snapshot Git final
  -> diff final borne
  -> cleanup cible via lease
  -> seconde verification Git de la source
```

## Contrat revalide

L'empreinte BLAKE3 source/worktree exclut les chemins absolus et les timings,
mais couvre les donnees qui changent la decision d'edition :

- versions des schemas et jeu de regles ;
- limites et etat de troncature ;
- resume d'analyse et compteurs de l'index ;
- resolutions et propositions completes ;
- hashes, ranges, noeuds et octets attendus ;
- validations par fichier ;
- rejets structures.

Le worktree doit recalculer exactement la meme empreinte. Une source modifiee
ou un autre `HEAD` entre les deux analyses provoque `revalidation_failed`.

Une seconde materialisation relit chaque fichier juste avant APPLY-001. Elle
controle a nouveau :

- chemin relatif strict, sans symlink, jonction ou reparse point ;
- hash BLAKE3 du fichier ;
- hash porte par chaque proposition ;
- contenu du noeud AST et de la range cible ;
- alignement UTF-8 et inclusion de la range dans le noeud ;
- absence de ranges chevauchantes ;
- application de la range la plus haute vers la plus basse ;
- hash et taille du resultat ;
- syntaxe Tree-sitter du resultat complet.

APPLY-001 compare ensuite `expected_before` aux octets presents au moment de la
transaction. Cette verification ferme la fenetre entre materialisation et
ecriture.

## Verification apres ecriture et build

Une transaction `committed` ne suffit pas a annoncer le succes. B3 relit les
fichiers ecrits, compare les octets et hashes attendus, puis les reparse.

Le build est ensuite execute avec :

- timeout ;
- annulation Ctrl+C ;
- sortie bornee ;
- terminaison de l'arbre de processus sous Windows ;
- Git State Guard strict avant/apres build.

Enfin, le snapshot Git pris apres le build doit contenir chaque fichier attendu
avec son hash `proposed_hash`. Tout autre changement hors metadonnees
`.opticcode` est refuse. Cette derniere verification detecte aussi une derive
survenue apres le reparse mais avant la capture finale.

## Rapport JSON

Le schema `java_edits_verify` version 1 separe explicitement :

- `source_analysis` : completude, bornes, propositions et empreinte source ;
- `revalidation` : propositions recues, valides, refusees et empreinte worktree ;
- `materialization` : validations et hashes avant/apres par fichier ;
- `post_write_validation` : octets, syntaxe et diagnostics apres transaction ;
- `final_git_validation` : hashes finaux et changements inattendus ;
- `worktree.apply` : transaction APPLY-001 et patch ;
- `worktree.build` : processus borne et Git State Guard ;
- `worktree.diff` : diff Git final ;
- `worktree.cleanup` : suppression ou recovery requise ;
- `worktree.source` : commit, refs et etat Git de la source avant/apres.

Les champs globaux ne confondent pas verification et nettoyage :

```json
{
  "verification_success": true,
  "cleanup_success": false,
  "lease_recovery_required": true,
  "operation_success": false
}
```

Un cleanup en echec ne transforme pas un build reussi en build echoue, mais
l'operation reste incomplete jusqu'a la recovery de la lease.

## Sorties bornees

Le patch complet est toujours fourni a APPLY-001, mais il n'est inclus dans le
JSON que si sa taille respecte `--output-limit-bytes`. Sinon :

- `patch` est vide ;
- `patch_complete` vaut `false` ;
- `patch_bytes` et `patch_hash` restent disponibles ;
- le diff Git final est lui aussi borne ;
- `worktree_after` reste le resume autoritaire chemins/statuts/hashes.

Le rapport ne transporte jamais les fichiers source complets.

## Statuts et codes de sortie

Statuts metier principaux :

- `passed` : revalidation, transaction, reparse, build, Git final et source OK ;
- `no_changes` : analyse complete, aucune proposition, aucun worktree ;
- `source_analysis_failed` : analyse incomplete, unsafe ou tronquee ;
- `revalidation_failed` : contrat source/worktree different ;
- `materialization_failed` : hash, range, octets, overlap ou reparse refuse ;
- `apply_failed` : transaction non commitee ;
- `post_write_validation_failed` : octets ou syntaxe differents apres APPLY ;
- `build_failed` : build en erreur ou timeout ;
- `final_git_validation_failed` : hash absent/different ou changement inattendu ;
- `source_changed` : commit, refs ou etat source modifies ;
- `cancelled`, `setup_failed`, `verification_failed`.

Codes CLI :

- `0` : succes ou aucun changement ;
- `6` : verification/apply/build en echec ;
- `7` : cleanup incomplet, recovery requise ;
- `8` : precondition invalide, notamment source Git sale.

## Tests couverts

La suite B3 verifie notamment :

- deux edits exacts dans un meme fichier et application en ordre inverse ;
- conservation de `CreatureSpawnEvent.SpawnReason.SPAWNER` ;
- contrats identiques dans deux racines absolues differentes ;
- changement de contenu detecte par l'empreinte ;
- derive du fichier entre proposition et materialisation ;
- transaction commitee et reparse post-ecriture ;
- hash Git final exact ;
- build reussi, build echoue et timeout avec terminaison ;
- mutation d'un fichier prepare pendant le build refusee par le guard et le hash final ;
- source Git sale refusee avant creation ;
- source Git sale refusee meme quand aucun edit n'est necessaire ;
- limite de propositions fail-closed ;
- zero edit sans worktree ni build ;
- patch et diff trop grands omis proprement avec tailles et hashes conserves ;
- projet source byte-identique, refs inchangees et worktree nettoye ;
- non-regression de l'ancien `worktree-verify`.

La gate complete du sprint passe avec 145 tests workspace, Clippy strict sans
avertissement et un build release optimise.

Commande de qualite :

```powershell
.\scripts\run-java-edit-worktree-quality.ps1
.\scripts\run-java-edit-worktree-quality.ps1 -Full
```

## Limites assumees

- la resolution reste volontairement plus conservatrice que `javac` ;
- seules les regles Bukkit 1.8 prouvees par B2 sont executees ;
- un build qui depend du reseau peut toujours echouer, mais reste borne ;
- aucune transaction multi-fichiers n'est atomique comme une base de donnees ;
- un programme externe peut modifier des fichiers, mais les controles successifs
  reduisent les fenetres TOCTOU et le snapshot final ferme le pipeline ;
- aucune promotion vers le projet source n'existe dans ce sprint.

## Suite

Le prochain travail peut maintenant se faire sans ajouter de logique a
`worktree.rs` :

1. etendre progressivement les regles legacy avec corpus positifs/negatifs ;
2. construire `CONTEXT-001` sur l'index de symboles ;
3. mesurer precision, latence, RAM et taille de contexte sur projets reels ;
4. introduire une premiere boucle agent bornee ;
5. concevoir la promotion controlee comme un sprint distinct avec approbation.
