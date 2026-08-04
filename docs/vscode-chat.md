# VSCODE-CHAT-001 - Participant Chat natif

Derniere mise a jour : 2026-08-04

## Statut

`@opticcode` est implemente avec l'API Chat stable de VS Code. Le participant
reste un client mince : il ne parse pas Java, ne construit pas le RAG et ne
manipule ni Git ni les fichiers. Toutes ces decisions restent dans le runtime
Rust.

Le Chat commence toujours en `read_only`. POLICY-001 et CHAT-EDIT-001 sont
actifs : Rust peut ouvrir un worktree de verification puis demander une
approbation native pour une transaction originale exacte.

## Ouvrir et invoquer

1. Ouvrir un dossier de projet dans VS Code 1.125 ou plus recent.
2. Ouvrir la vue Chat native.
3. Saisir `@opticcode`, puis une commande ou une question.
4. Ajouter un fichier ou une selection avec les controles de contexte du Chat.

Sans slash command, le comportement est identique a `/ask` :

```text
@opticcode Explique le cycle de vie de ce listener Bukkit.
```

Le participant visible est `OpticCode Local`, avec l'identifiant stable
`opticcode.chat`. L'icone est attribuee par l'API runtime stable. Aucune proposed
API et aucune installation VS Code Insiders ne sont requises.

## Commandes

| Commande | Etat VSCODE-CHAT-001 | Ecriture |
| --- | --- | --- |
| `/ask` | reponse locale streamee | aucune |
| `/plan` | plan d'implementation | aucune |
| `/inspect` | faits documentaires exacts avec preuves | aucune |
| `/context` | contexte borne et explique | aucune |
| `/analyze` | analyse Java | aucune |
| `/index` | etat/index symbolique Java | aucune |
| `/legacy` | constats Minecraft 1.8 | aucune |
| `/status` | etat repository/runtime | aucune |
| `/runs` | metadata recentes bornees | aucune |
| `/help` | commandes et limites | aucune |
| `/fix` | plan + verification worktree | worktree OpticCode uniquement |
| `/verify` | revalidation d'une proposition | worktree OpticCode uniquement |
| `/diff` | snapshots et diff natif | aucune |
| `/apply` | transaction confirmee | original apres modale + approval |
| `/rollback` | restauration transactionnelle | original apres modale + approval |

Une commande inconnue est refusee dans l'extension avant de lancer le CLI.
Le Chat demarre toujours en mode `read_only`. Si un client fabrique une requete
`worktree_edit` ou `approved_apply`, Rust force le mode effectif read-only puis
refuse la requete avant de resoudre les references.

## Architecture

```text
VS Code ChatRequest
  -> normalisation TypeScript bornee
  -> une requete opticcode.chat schema 1 sur stdin
  -> opticcode.exe chat --protocol-jsonl
  -> PolicyEngine Rust deny-by-default
  -> resolution sure des references dans Rust
  -> contexte/RAG/Java existants
  -> LlmProvider existant pour Ask/Plan/EditPlan
  -> opticcode-edit + GIT-002 + APPLY-001 pour les editions
  -> evenements NDJSON opticcode.chat schema 1
  -> ChatResponseStream natif
```

Le parseur NDJSON TypeScript est partage avec `opticcode.assistant`. Les deux
flux reutilisent les memes controles : UTF-8 strict, limites d'octets et
d'evenements, request ID, sequence monotone, terminal unique, timeout et
annulation cooperative.

Le Chat ne transporte jamais une conversation complete dans les arguments du
processus. `opticcode.exe chat --protocol-jsonl` lit exactement une requete
initiale bornee, puis uniquement des messages de controle bornes.

## Requete machine

Le contrat `opticcode.chat` schema 1 contient notamment :

- request, workspace et session IDs ;
- racine workspace ;
- commande et prompt ;
- profil, provider, modele et mode de contexte ;
- references structurees ;
- historique borne ;
- budgets et options de generation ;
- mode de securite ;
- metadata client ;
- versions de protocoles attendues.

Les commandes `/ask` et `/plan` passent encore par le protocole Assistant et le
provider Ollama existants cote Rust. Il n'existe pas de second client Ollama
dans l'extension.

Chaque commande passe d'abord par `opticcode-policy`. L'evenement
`request_accepted` contient le mode demande, le mode effectif, la version de
politique, la decision et le `rule_id`. TypeScript valide ces champs mais ne
reimplemente aucune regle.

## References

References prises en charge :

- fichier joint ;
- range jointe ;
- selection active en UTF-16 ;
- fichier actif ;
- symbole structure ;
- finding OpticCode ;
- run precedent ;
- diff/proposition precedente.

Les chemins sont convertis en chemins relatifs au workspace lorsqu'ils sont
acceptes. L'extension refuse lexicalement une reference hors du workspace ; le
runtime Rust refait ensuite toutes les verifications de securite :

1. canonicalisation de la racine et du fichier ;
2. verification de confinement ;
3. refus des symlinks, jonctions et reparse points ;
4. refus des noms et contenus sensibles selon RAG-SAFE ;
5. controle de taille et UTF-8 ;
6. empreinte avant/apres lecture contre les derives TOCTOU.

Une reference jointe autorise uniquement une lecture. Elle n'est jamais une
autorisation d'ecriture.

Si une selection precise est jointe, OpticCode n'ajoute pas automatiquement le
fichier actif entier. Plusieurs fichiers, espaces, accents et emojis dans les
noms sont pris en charge.

La reponse distingue :

- **User references** : choix explicites de l'utilisateur ;
- **Discovered context** : declarations, appelants ou configurations trouves ;
- **Local RAG** : nombre de hits recuperes dans l'index local.

## Historique et sessions

Limites V1 envoyees au runtime :

```text
12 tours maximum
32 K caracteres maximum
8 K tokens estimes maximum
8 K caracteres maximum par tour
24 references maximum
1 Mio de contenu de references maximum
32 K tokens maximum pour le prompt
1 024 tokens de sortie par defaut
```

Les tours les plus recents sont prioritaires. Les historiques malformes sont
ignores, les gros blocs diff/JSON sont remplaces par un resume d'omission et les
motifs de secrets evidents sont redactes avant transport. Rust reapplique ses
propres bornes et son filtrage.

VS Code fournit le contenu de la conversation active. OpticCode ne persiste que
des metadata bornees : workspace ID, namespace de session, digest repository,
run IDs recents et chemin du dernier rapport. Aucun prompt, historique, code ou
secret n'est stocke dans ce registre.

Le namespace inclut la racine canonique, l'etat repository connu, la session du
participant et la version de schema. Deux workspaces ne partagent ni connexion,
ni metadata, ni run IDs injectes.

## Evenements et rendu

Les evenements couvrent l'acceptation, les references, le contexte, le RAG, le
provider, les deltas, findings, avertissements, metriques et les futurs cycles
edit/verify/diff/apply/rollback. Chaque enveloppe contient protocole, schema,
request ID, sequence et temps relatif.

Le rendu stable utilise :

- `progress` pour les etapes ;
- `markdown` pour les deltas et resumes ;
- `reference` et `anchor` pour les fichiers/ranges ;
- `filetree` pour les fichiers de contexte ;
- `button` pour Show Context, Show Full Report, Open Output et Refresh Status.

Les commandes des boutons techniques ne sont pas contribuees au manifeste et
ne polluent donc pas la palette publique.

## Annulation et erreurs

Une annulation VS Code envoie un message structure :

```json
{"schema_version":1,"protocol":"opticcode.chat.control","request_id":"...","type":"cancel"}
```

Un terminal `cancelled` est la seule confirmation propre. Les cas suivants
restent differents : timeout, annulation non confirmee, kill force, interruption
processus, protocole incompatible, sequence invalide, terminal absent ou double.

`stdout` reste reserve au NDJSON. `stderr` est borne et dirige vers le canal
Output OpticCode.

## Stockage et rapports

Les rapports Chat sont ecrits dans le stockage global de l'extension, hors du
projet source. Ils contiennent la reponse, le resume machine et les evenements ;
les deltas sont remplaces par leur taille pour eviter une seconde copie complete.
Le prompt et l'historique ne sont pas recopies dans le rapport.

Les vues Status, Findings et Runs restent le tableau de controle. Le Chat est
l'interface principale pour la conversation.

## Validation

La gate dediee est :

```powershell
.\scripts\run-vscode-chat-quality.ps1 -WithExtensionHost
.\scripts\run-vscode-chat-quality.ps1 -Full
```

Elle couvre notamment :

- schema et round-trip strict des references ;
- fichiers, ranges et selection Unicode ;
- sortie de racine, fichier sensible, fichier absent ;
- jonction Windows reelle ;
- historique borne, malforme et redacte ;
- NDJSON fragmente ;
- sequences externes et LLM imbriquees ;
- terminal absent/double ;
- timeout, annulation confirmee et kill force distinct ;
- Markdown, references, boutons et metriques ;
- isolation de sessions/workspaces ;
- enregistrement du participant dans un vrai Extension Host ;
- handlers deterministes `/help`, `/status`, `/context`, Ask et `/fix` ;
- decision Policy et mode effectif read-only pour chaque commande ;
- refus d'un client demandant un mode plus permissif avant toute reference ;
- snapshots virtuels Unicode/CRLF, boutons de revue et absence d'Apply apres
  verification echouee ;
- E2E Rust `/fix`, typed apply sans mutation, modale simulee, apply et rollback.

L'API de test VS Code ne permet pas de saisir de facon stable une requete dans
la vue Chat comme un utilisateur. Le Prompt Lab active donc le vrai participant
et invoque exactement son callback enregistre dans l'Extension Host. Le meme
test traverse le service, le vrai client processus, le vrai `opticcode.exe`,
NDJSON, le presenter et les metriques ; seul le champ visuel n'est pas
automatise. Voir [`prompt-lab.md`](prompt-lab.md).

## Premier test manuel

1. Construire `opticcode.exe` en release.
2. Lancer `npm run compile` dans `extensions/vscode-opticcode`.
3. Ouvrir `benchmarks/java-index-mini` dans l'Extension Development Host.
4. Ouvrir Chat et saisir `@opticcode /help`.
5. Tester `/status` puis `/context Locate Helpers#ping().`.
6. Joindre `Helpers.java`, puis selectionner une range et lancer `/ask`.
7. Verifier les ancres, le contexte, les tokens et la duree.
8. Annuler un Ask en cours et verifier que l'issue est explicitement confirmee
   ou rapportee comme forcee/non confirmee.
9. Tester `/fix` sur une copie Git propre et ouvrir le diff natif.
10. Confirmer que l'original reste propre avant la modale Apply.
11. Appliquer, puis utiliser le bouton de rollback et verifier le retour a la base.

## Limites actuelles

- aucun shell arbitraire, package install, Git push ou publication ;
- pas de boucle autonome ;
- pas de daemon reseau ;
- `legacy` reste le mode de contexte par defaut ;
- le provider brut dans le selecteur de modeles VS Code n'est pas implemente.
- `references_preferred` est le scope UI par defaut ; une restriction explicite
  du prompt est reduite a `references_only` par Rust.

Le cycle detaille, les limites et la recuperation sont dans
[`chat-edits.md`](chat-edits.md).
