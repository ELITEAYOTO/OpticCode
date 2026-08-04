# POLICY-001 - Politique d'actions deny-by-default

Derniere mise a jour : 2026-08-04

## Statut

`POLICY-001` fournit l'autorite de securite centrale d'OpticCode. Le nouveau
crate `opticcode-policy` est independant du modele, du Chat, de VS Code et des
prompts. Une action qui n'est pas representee, comprise et autorisee par ce
moteur ne peut pas devenir implicitement executable.

CHAT-EDIT-001 consomme maintenant cette autorite pour `/fix`, `/verify`,
`/diff`, `/apply` et `/rollback`. Le client demande toujours `read_only` ; Rust
seul ouvre un scope `worktree_edit` ou `approved_apply` pour une action exacte.

## Architecture

```text
demande structuree
  -> validation schema/protocole/bornes
  -> canonicalisation workspace et frontieres Git
  -> refus secrets, liens et sorties de racine
  -> validation du mode et de l'action typee
  -> premiere regle deterministe applicable
  -> Allow | RequireApproval | Deny
  -> audit hors workspace
  -> revalidation juste avant execution
```

Le crate est decoupe par responsabilite :

```text
crates/opticcode-policy/
  src/model.rs       actions, modes, decisions et rapports publics
  src/paths.rs       canonicalisation, confinement, empreintes et TOCTOU
  src/engine.rs      ordre des regles et evaluation deny-by-default
  src/approval.rs    confirmations natives, grants opaques et one-shot
  src/audit.rs       journal atomique, borne et namespace par workspace
  src/lib.rs         surface publique minimale
  tests/policy.rs    cas nominaux, derives et attaques
```

## Contrat machine

Le protocole est `opticcode.policy`, schema 1. Une `PolicyRequest` lie :

- `request_id` et `action_id` bornes ;
- origine, profil et client ;
- `workspace_id` et racine canonique ;
- mode de securite ;
- frontiere Git observee ;
- proprete et digest du working tree lorsque necessaires ;
- worktree actif et lease eventuelle ;
- action typee ;
- identifiant d'approbation optionnel.

Les champs critiques inconnus sont refuses par Serde. Une variante d'action
future sans payload devient `Unknown` et recoit `Deny`; un payload inconnu ou un
mode inconnu echoue pendant le decodage. Aucune chaine shell opaque ne remplace
le couple `program` + `arguments[]`.

## Actions

Le modele ferme couvre :

| Famille | Actions |
| --- | --- |
| Lecture | `ReadFile`, `ReadDirectory`, `Search`, `BuildContext`, `UseRag` |
| Fichiers | `WriteFile`, `CreateFile`, `DeleteFile`, `ApplyPatch` |
| Processus | `RunProcess` |
| Worktrees | `CreateWorktree`, `CleanupWorktree` |
| Git | `GitRead`, `GitWrite`, `GitCommit`, `GitPush` |
| Externe | `NetworkAccess`, `PackageInstall`, `Publish` |
| Transactions | `RecoverTransaction`, `RollbackTransaction` |
| Autorite | `ModifyPolicy`, `ElevatePrivileges`, `Unknown` |

Chaque rapport contient schema, version de politique, hashes action/workspace,
decision, `rule_id`, risque, raisons bornees, conditions, recommandation,
caractere retriable et eventuel event d'audit.

## Modes

### `read_only`

Autorise les lectures sures, analyses, contexte, RAG v2 et operations Git de
lecture. Toute mutation est refusee. Ajouter un approval ID ne peut pas elargir
ce mode.

### `worktree_edit`

Une mutation exige un worktree detache sous le stockage temporaire controle :

- source et `HEAD` observes ;
- source propre et digest present ;
- run ID valide ;
- proprietaire `workspace_id` et `request_id` exact ;
- lease reguliere correspondante ;
- fichier `.git` et `commondir` reels ;
- gitdir distinct mais contenu dans le common dir ;
- cible contenue dans le worktree ;
- revalidation avant execution.

Une lease historique qui ne contient pas les champs de proprietaire ne suffit
pas a autoriser une action Chat. `CHAT-EDIT-001` cree la lease enrichie via son
adaptateur Rust de confiance et la lie au workspace et a la requete exacts.

### `approved_apply`

L'original n'est modifiable que par un `ApplyPatch` verifie. Le moteur exige :

- frontiere Git complete ;
- depot observe propre ;
- `HEAD` exact ;
- digest du working tree ;
- hash du diff et du jeu de fichiers ;
- chemins uniques, tries et bornes ;
- transaction ID ;
- approbation native one-shot correspondant a tous ces champs.

`WriteFile`, `CreateFile` et `DeleteFile` ne contournent pas ce chemin. Les
deletions originales doivent elles aussi etre representees dans un patch
transactionnel verifie.

## Ordre des regles

L'ordre est volontairement stable :

1. schema, protocole, IDs, origine et tailles ;
2. racine workspace et stockage Policy disjoints ;
3. frontiere Git observee ;
4. action inconnue, elevation ou modification Policy ;
5. canonicalisation et chemins sensibles ;
6. mode effectif ;
7. frontiere workspace/worktree et lease ;
8. preconditions specifiques a l'action ;
9. allowlist processus/reseau ;
10. approval eventuel ;
11. audit ;
12. revalidation immediate par l'executor.

La premiere regle bloquante gagne. Un echec d'inspection devient un refus, pas
une permission de secours.

## Chemins et Git

Pour chaque cible, le moteur :

- resolve la racine reelle ;
- refuse une racine liee ou reparse ;
- transforme la cible en chemin relatif structurel ;
- refuse `..`, prefixes Windows et sorties de racine ;
- inspecte chaque composant sans suivre symlink, jonction ou reparse point ;
- refuse `.git`, `.env` et variantes, stores de credentials et cles ;
- refuse le franchissement d'un depot imbrique ou sous-module ;
- calcule metadata et BLAKE3 du contenu borne ;
- compare l'empreinte lors de `revalidate`.

Les frontieres Git identifient separement worktree root, gitdir, commondir,
index et object directory. Pour un depot principal, gitdir doit etre exactement
`<workspace>/.git`; l'index et le dossier objects doivent etre les emplacements
attendus. Un prefixe lexical n'est jamais une preuve de confinement.

`PolicyPreflight::revalidate_observed` compare aussi une requete reconstruite
depuis des observations Git/worktree fraiches. Une derive consomme l'approbation
mais bloque l'execution.

## Processus

`RunProcessAction` contient :

```text
executable
arguments[]
cwd
timeout_ms
output_limit_bytes
network
launch
environment_allowlist[]
```

Regles V1 :

- Maven et Gradle seulement pour compile/test/check/build/package ;
- wrappers projet ou executables absolus sous une racine d'installation fiable ;
- wrapper du worktree byte-identique au wrapper source ;
- `cwd` reel dans le worktree, sans depot imbrique ;
- timeout maximal d'une heure et sortie maximale de 16 Mio par flux ;
- environnement herite limite a des noms non secrets documentes ;
- `network: denied` exige `--offline`/`-o`; sans cela le build est refuse,
  tandis qu'un besoin reseau declare exige une approbation ;
- objectifs install/deploy/publish et options de redirection de projet refuses ;
- shells, substitutions, concatenations et redirections refuses ;
- scripts `.cmd`/`.bat` soumis a un filtrage plus strict des metacaracteres ;
- un caractere `&`, `|` ou `>` dans une valeur d'un processus direct reste une
  donnee, tandis qu'un operateur separe ou une commande composee est refuse ;
- Java/javac direct reste ferme : Maven/Gradle est le chemin de verification V1.

Les scripts Windows utilisent le lanceur dedie existant du Process Runner, pas
une commande shell fournie par le modele. Le moteur n'est pas un sandbox OS :
le futur executor doit recopier uniquement les variables approuvees, honorer le
mode reseau et reutiliser le Process Runner borne.

## Approvals

Une approbation ne peut etre emise que par l'API Rust avec une
`NativeConfirmation` issue d'une surface utilisateur explicite. Aucune commande
CLI publique ne fabrique de grant.

Le record lie exactement :

- approval et request IDs ;
- workspace ID et hash de racine ;
- mode ;
- `HEAD` ;
- digest du working tree ;
- diff ;
- fichiers et ordre exact ;
- actions et ordre exact ;
- transaction ;
- dates de creation et expiration.

La consommation cree d'abord un claim atomique avec `create_new`. Un seul
consommateur peut gagner, y compris sous concurrence Windows. Le claim est
conserve apres succes ou crash, donc replay et double consommation echouent.
Expiration, corruption ou toute derive consomment le grant et imposent une
nouvelle verification. Une reponse Chat comme `oui` n'est pas une approbation.

## Audit

Etat par defaut :

```text
%LOCALAPPDATA%/OpticCode/policy-v1/audit/workspaces/<workspace-hash>/events/
```

Chaque event est un fichier JSON cree puis publie atomiquement. Il contient
uniquement metadata bornees : timestamp, request ID, hashes, action, regle,
decision, risque, resultat, duree, transaction hashee et approval hashee. Il ne
contient ni prompt complet, ni source, ni secret, ni token d'approbation brut.

La rotation conserve au maximum 2 048 events et 32 Mio par workspace. Une
ecriture partielle ou corrompue est ignoree et comptee. Les lectures scopees
refusent un workspace different de l'autorite fournie.

## CLI

```powershell
$request | .\target\release\opticcode.exe policy check --json
$request | .\target\release\opticcode.exe policy explain --json
.\target\release\opticcode.exe policy audit --json --workspace-hash <digest>
```

`check` audite et peut consommer un grant. `explain` n'a aucun de ces deux effets.
`audit` retourne des metadata bornees. L'entree stdin est limitee a 1 Mio.

Codes de sortie stables de `check` :

| Code | Signification |
| --- | --- |
| `0` | `Allow` |
| `10` | `RequireApproval` |
| `11` | `Deny` |
| `1` | requete, schema ou stockage invalide |
| `2` | erreur de syntaxe CLI Clap |

`stdout` contient uniquement le JSON demande ; les erreurs et logs restent sur
`stderr`.

## Chat et discovery

Le runtime Rust evalue chaque commande Chat avec `PolicyEngine` et force le mode
effectif `read_only`, meme si le client demande `worktree_edit` ou
`approved_apply`. L'evenement `request_accepted` expose sans secret :

- mode demande et mode effectif ;
- version Policy ;
- decision ;
- `rule_id`.

Les commandes de lecture obtiennent `Allow`. `/fix` produit d'abord un plan en
lecture, puis chaque creation worktree, ecriture, processus, diff et cleanup est
evalue separement. `/apply` et `/rollback` exigent `RequireApproval`, une
confirmation native puis la consommation one-shot.

`version --json`, `capabilities --json` et `doctor --json` exposent le schema,
les trois modes, le moteur, l'audit, les approvals, la CLI et
`chat_write: true` sans supprimer les champs historiques. Cette capacite ne
permet pas au client de choisir lui-meme un mode plus permissif.

## Validation

Gate dediee :

```powershell
.\scripts\run-policy-quality.ps1
.\scripts\run-policy-quality.ps1 -Full
```

Elle utilise uniquement des fixtures et dossiers temporaires. Elle verifie
formatage, Clippy, tests Policy/CLI/Chat, build release, aides debug/release,
JSON pur, codes de sortie, jonction Windows, concurrence approvals/audit,
invariance Git/RAG, absence de lease/processus residuel et chemins sensibles.

Etat valide de POLICY-001 : 288 tests Rust (dont 25 Policy et 59 dans le package
CLI), 36 tests unitaires TypeScript, integration CLI reelle, Extension Host,
build release et audit RustSec sur 195 dependances sans vulnerabilite connue.

## Integration CHAT-EDIT-001

- `ProposalStore` conserve les references d'audit, jamais les grants bruts ;
- les leases GIT-002 portent workspace et request ID ;
- une creation controlee est permise seulement dans le worktree actif reconnu ;
- Maven/Gradle restent offline, shell false et bornes ;
- l'original exige un diff, fichiers, HEAD, digest et transaction exacts ;
- l'approval est cree seulement apres la modale native puis consomme une fois ;
- une transaction etrangere ou un replay sont refuses ;
- TypeScript ne contient aucune regle Policy.

Limite conservee : Policy n'est pas un sandbox OS. L'executor applique donc
encore l'allowlist, l'environnement sans reseau, les timeouts, GIT-002,
APPLY-001 et les revalidations TOCTOU.

Reference : [`chat-edits.md`](chat-edits.md).
