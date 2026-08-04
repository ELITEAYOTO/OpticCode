# CHAT-EDIT-001 - Editions Chat verifiees

Derniere mise a jour : 2026-08-04

## Statut

`CHAT-EDIT-001` est implemente. OpticCode peut produire une proposition avec le
modele local, la verifier dans un worktree jetable, afficher un diff natif VS
Code, puis appliquer ou rollbacker une transaction apres confirmation native.

Ce jalon n'est pas une boucle autonome. Une demande `/fix` s'arrete a l'etat
`verified`. Le projet original ne change jamais sans la modale VS Code et une
approbation Policy one-shot.

## Architecture

```text
ChatRequest read_only
  -> Policy BuildContext
  -> LlmProvider local
  -> EditPlan JSON non fiable
  -> validation Rust stricte
  -> ProposalStore hors workspace
  -> Policy worktree_edit
  -> worktree Git detache et possede
  -> APPLY-001 dans le worktree
  -> Tree-sitter Java
  -> Maven/Gradle offline via Process Runner
  -> diff Git reel et snapshots de revue
  -> cleanup cible et source inchangee
  -> confirmation modale VS Code
  -> Policy approved_apply one-shot
  -> APPLY-001 sur l'original
  -> reparse et diff exact
  -> rollback exact disponible
```

Les responsabilites sont separees :

- `opticcode-edit` : schemas, validation, store, worktree, diff, apply et rollback ;
- `opticcode-core/chat_edit_runtime.rs` : orchestration du protocole Chat ;
- `opticcode-policy` : autorite deny-by-default et approvals ;
- `opticcode-tools` : Git State Guard, Process Runner, GIT-002 et APPLY-001 ;
- extension TypeScript : modale native, URI virtuelles et rendu uniquement.

## Commandes

### `/fix <tache>`

Genere exactement un `EditPlan` structure. Une seule correction de format est
permise si la premiere reponse n'est pas un objet JSON conforme. Une erreur
fonctionnelle, un mauvais hash ou une operation interdite ne declenche pas une
seconde tentative.

La generation structuree active un schema JSON natif provider-neutre, traduit
par Ollama dans `format`. Le schema contraint la forme, les types, les enums et
les identites constantes. Les bornes de taille et les regles semantiques restent
controlees par Rust, car elles ne doivent pas dependre du support de grammaire du
runtime LLM.

Pour chaque reference fichier resolue, Rust calcule un snapshot fiable contenant
le chemin, le BLAKE3 du fichier complet, la taille, les fins de ligne et des
ancres de lignes avec ranges byte exactes. Qwen copie ces valeurs au lieu de
recalculer un hash ou de compter les octets. Le parseur Serde ferme et toute la
validation Rust restent obligatoires : un JSON conforme au schema n'est ni une
autorisation ni une proposition valide.

Le plan valide est stocke, applique dans un worktree detache, reparse, compile
offline et compare par Git. Un succes publie `verified`, le diff et le bouton
`Apply Verified Changes`. Un echec publie `verification_failed` sans bouton
Apply.

### `/verify [proposal-id]`

Rejoue la verification du plan immuable. Le workspace, le profil, le TTL, le
HEAD, le digest, les fichiers et leurs hashes sont revalides. Aucun contenu du
plan n'est regenere silencieusement.

### `/diff [proposal-id]`

Lit le dernier diff verifie et les snapshots minimaux depuis le store. Cette
commande ne cree ni worktree ni processus. Le bouton `Discard Proposal` utilise
egalement ce chemin read-only avec un controle structure pour changer seulement
l'etat local de la proposition.

### `/apply [proposal-id]`

Une commande tapee, `oui` ou une reponse du modele ne fait qu'afficher
`approval_required`. Le bouton interne ouvre une modale native contenant le
workspace, les statistiques, les creations, le build, les tests et le rollback.

Apres `Apply`, l'extension envoie une confirmation structuree liee a un
`approval_request_id` deterministe du proposal et du hash de diff. Rust cree et
consomme l'approbation Policy one-shot pendant la transaction. Le token Policy
n'est jamais expose au modele ni a TypeScript.

### `/rollback [transaction-id]`

Le rollback cible la transaction APPLY-001 exacte. Proposal et transaction
doivent correspondre. Le runtime refuse une transaction d'un autre workspace,
une derive des fichiers ou du diff, et toute transaction inconnue. Une modale
native et une nouvelle approbation one-shot sont requises. Le rollback est
idempotent et n'utilise jamais `git reset --hard`.

## EditPlan V1

Le schema Serde ferme contient les identites de requete/workspace, le hash de
racine, le profil, provider et modele, le HEAD, le digest Git, les references,
un resume, les operations, les validations, risques, limites et expiration.

Operations autorisees :

- modifier un fichier texte UTF-8 existant avec hash complet, range byte,
  ancien contenu exact et remplacement ;
- creer au plus un fichier texte UTF-8 allowliste et attendu absent.

Extensions : `.java`, `.xml`, `.yml`, `.yaml`, `.json`, `.toml`,
`.properties`, `.gradle`, `.md`, `.txt`.

Refus : suppression, rename, move, binaire, executable, secret, `.git`,
`.opticcode`, wrapper Maven/Gradle, symlink, jonction, reparse point, traversal,
chemin absolu, reseau, telechargement et installation de dependance.

## Limites dures

| Limite | Maximum runtime |
| --- | ---: |
| Fichiers | 5 |
| Fichiers crees | 1 |
| Taille par fichier | 512 Kio |
| Proposition | 2 Mio |
| Diff affichable | 1 Mio |
| Hunks | 64 |
| Lignes ajoutees | 1 500 |
| Lignes supprimees | 1 500 |
| Lignes changees | 2 000 |
| Generation principale | 1 |
| Correction de format | 1 |
| Worktree et build principal | 1 |
| Timeout global | 15 minutes |

Le client et le modele peuvent reduire ces valeurs, jamais les augmenter.
Une limite produit un refus explicite, pas une modification tronquee.

## ProposalStore

Emplacement par defaut :

```text
%LOCALAPPDATA%\OpticCode\proposals-v1\<workspace-hash>\
```

Le store utilise des generations JSON immuables publiees atomiquement, un
verrou fichier borne et une recuperation du dernier record valide. Il ignore un
record tronque et preserve tout repertoire non reconnu. Il ne conserve ni
prompt complet ni chain-of-thought.

Etats : `generated`, `validated`, `worktree_prepared`, `worktree_applied`,
`build_running`, `verified`, `verification_failed`, `approval_pending`,
`applying`, `applied`, `rollback_available`, `rolling_back`, `rolled_back`,
`discarded`, `expired`, `failed`.

TTL : 24 heures pour une proposition non appliquee et 7 jours pour les rapports
de transaction. Les transitions invalides sont refusees.

## Worktree et build

La source doit etre le worktree Git principal et propre au moment de la
proposition et de l'application. GIT-002 cree un worktree detache du HEAD exact,
avec lease liee au workspace et au request ID. Chaque ecriture passe par Policy
et APPLY-001.

Maven utilise `-o`; Gradle utilise `--offline`. Le shell est desactive, le
processus, son arbre Windows, le timeout et les sorties sont bornes. Une
dependance absente du cache fait echouer la verification; OpticCode ne propose
pas d'appliquer quand meme.

Le resultat du build, le cleanup et l'invariance de la source sont separes. Un
build reussi avec cleanup incomplet reste non applicable et demande recovery de
lease.

## Diff VS Code

Rust calcule le patch et les statistiques depuis le diff Git reel. Les
snapshots base/propose sont publies par fichier puis conserves en memoire dans
l'extension. Ils peuvent etre recharges depuis `/diff` apres reload.

Schemes read-only :

```text
opticcode-base:
opticcode-proposed:
```

Ils preservent UTF-8 et LF/CRLF. Un fichier cree a un document base vide. Les
boutons `Show Diff` et `Show All Changes` ouvrent `vscode.diff`; les commandes
internes ne sont pas contribuees a la palette publique.

## Annulation et pannes

L'annulation LLM est cooperative. Pour worktree/build/diff, elle est relayee au
Process Runner. Une annulation pendant la transaction originale ne provoque pas
un abandon sauvage : APPLY-001 finit ou rollbacke, puis le rapport distingue le
resultat.

En cas d'echec apply, OpticCode verifie les snapshots reels avant de revenir a
`verified`. Une restauration incomplete place la proposition en `failed`.
APPLY-001 conserve son journal cible pour inspection et recovery.

## Validation

Tests principaux :

- parsing strict, identites, hashes, ranges, Unicode, CRLF et chemins Windows ;
- store atomique, concurrence, crash, record tronque, TTL et isolation ;
- Policy, lease, worktree, apply, reparse, build offline, diff et cleanup ;
- E2E public Chat : `/fix -> typed apply refused -> native apply -> transaction
  etrangere refusee -> native rollback -> base byte-for-byte` ;
- TypeScript : snapshots, creation, Unicode, CRLF, boutons et isolation ;
- Extension Host : `/fix`, rendu, commandes internes et fournisseur virtuel.

Gate dediee :

```powershell
.\scripts\run-chat-edit-quality.ps1
.\scripts\run-chat-edit-quality.ps1 -WithExtensionHost
.\scripts\run-chat-edit-quality.ps1 -Full
```

Smoke local Qwen optionnel, borne a une fixture temporaire et sans Apply :

```powershell
.\scripts\run-chat-edit-qwen-smoke.ps1
```

Validation du 2026-08-04 avec `qwen2.5-coder:14b` Q4_K_M : une generation,
zero correction de format, 25 evenements, original inchange et zero lease
residuelle. Le passage chaud a pris environ 33 secondes ; la validation finale
froide avec dechargement du modele a pris 91 648 ms. Trois echecs precedents ont
ete refuses sans mutation (texte apres JSON, faux hash, puis range/schema
incorrects) et ont conduit au schema natif et aux ancres byte fiables.

## Test manuel

1. Ouvrir une copie Git propre d'un petit plugin Java 8.
2. Ouvrir un fichier Java et selectionner la methode cible.
3. Lancer `@opticcode /fix <tache precise>`.
4. Verifier plan, build, tests et statistiques.
5. Ouvrir `Show Diff` puis `Show All Changes`.
6. Confirmer que `git status` du projet est reste propre.
7. Cliquer `Apply Verified Changes`, relire la modale puis choisir `Apply`.
8. Compiler ou inspecter le resultat.
9. Cliquer `Rollback Transaction` et confirmer.
10. Verifier le retour au contenu et au Git state de base.

Toujours utiliser une copie ou une fixture pour le premier essai. Aucun push ou
commit Git n'est cree par ce workflow.

## Limites restantes

- une transaction multi-fichiers n'est pas atomique au sens base de donnees ;
- un editeur externe peut modifier un fichier entre deux phases et provoquer un
  refus de derive ;
- le build post-apply reutilise la preuve du build worktree pour les snapshots
  strictement identiques, puis reparse et recalcule le diff original ;
- pas de delete/rename, boucle agent, MCP, Tantivy, embeddings ou llama.cpp dans
  ce jalon ;
- `legacy` reste le contexte par defaut.
