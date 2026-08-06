# GROUNDING-METRICS-001 - Grounding strict

Derniere mise a jour : 2026-08-04

## But

Le grounding empeche une reponse Chat d'etre presentee comme fondee sur un
fichier simplement parce que ce fichier a ete selectionne dans VS Code. Rust
reste l'autorite : il resout la reference, applique les controles de chemin,
lit un snapshot borne, construit le manifest, puis valide chaque preuve avant
de rendre le moindre texte du modele.

## Cause racine corrigee

Avant ce jalon, la preparation Chat connaissait la reference utilisateur, mais
`ask` et `plan` repassaient ensuite par l'assistant historique. Celui-ci
reconstruisait un contexte projet avec resume, historique, discovery et RAG.
L'interface affichait la reference candidate comme `attached by user`, sans
preuve que ses octets etaient le contexte dominant ou meme effectivement lus.

Cela expliquait les symptomes observes :

- les classes Java du tour precedent revenaient via le second context builder ;
- des commandes Cargo provenaient du pre-prompt/profil et des exemples internes ;
- la tache stricte etait noyee dans un prompt plus large sans contrat de sortie ;
- aucune preuve machine ne bloquait une affirmation issue d'un autre fichier.

Le correctif ne repose pas sur l'espoir que Qwen suive mieux une consigne. Il
supprime le second enrichissement pour les routes strictes et refuse les sorties
qui ne satisfont pas les invariants Rust.

## Etats d'une reference

Une reference traverse des etats distincts :

1. `reference_selected` : le client l'a recue dans la requete actuelle ;
2. `reference_resolved` : Rust a obtenu un chemin/range admissible ;
3. `reference_injected` : un snapshot exact figure dans le manifest et le prompt ;
4. `reference_refused` : la Policy ou la lecture sure l'a refusee.

Le protocole conserve aussi les notions de contexte decouvert, RAG et historique.
Seul `reference_injected` autorise l'interface a dire qu'un fichier a ete utilise.

## Scopes

### `automatic`

Conserve le comportement large existant : references actuelles, contexte
legacy/symbolique, discovery, RAG et historique borne. Chaque provenance reste
visible. C'est la route compatible avec les usages generaux existants.

### `references_preferred`

Priorise les references de la requete. Lorsqu'une reference lisible existe, la
V1 choisit prudemment la route stricte sans discovery additionnelle. Sans
reference, elle peut revenir a la route automatique.

### `references_only`

N'autorise que les references explicitement jointes au tour courant : aucun
fichier actif implicite, historique source, resume projet, discovery ou RAG.
Sans reference, le runtime renvoie `references_required`. Une consigne telle
que `utilise uniquement ce fichier` reduit un scope client trop large et marque
la raison `server_downgrade` ou `explicit_prompt_restriction`.

## Snapshot et manifest

Chaque snapshot autoritatif contient :

- chemin relatif au workspace et identite du workspace ;
- hash BLAKE3 complet du fichier et des octets injectes ;
- taille, encodage UTF-8 et line ending ;
- range de lignes et range d'octets ;
- origine, raison, etat Git et contenu exact injecte.

Le fichier est relu avant rendu. Un changement entre lecture et validation
produit `reference_snapshot_stale`; la reponse n'est pas affichee.

`ContextManifest` schema v1 est la source unique pour le prompt, les preuves,
`Show Injected Context`, les metriques et les fingerprints. Son fingerprint
depend du scope, du workspace, de l'ordre, des chemins, hashes, ranges, octets
injectes, profil et version du prompt.

Le `prompt_fingerprint` ajoute la tache, la commande, le mode de preuve, le
mode de contexte, l'etat repository, la session, le provider, le modele et les
parametres de generation. Aucun cache de reponse cross-run n'est active ; les
index read-only existants gardent leurs propres cles et ne deviennent jamais
une preuve hors manifest.

## Historique

Chaque tour transporte `source_scope`, `workspace_id`,
`context_fingerprint` et `grounding_status`. Les routes strictes omettent par
defaut son contenu source. Un tour d'un autre workspace, non grounde ou sans
metadata compatible est refuse. `OpticCode: Clear Chat Session Context`
incremente un epoch et efface l'historique/reports recents, sans supprimer les
propositions ni transactions d'edition.

## Evidence et conformite

En `evidence_mode=required`, la sortie provider est un JSON schema v1 avec :

- `answer` ;
- claims `observed`, `inferred`, `general_knowledge` ou
  `insufficient_evidence` ;
- citations chemin/lignes/hash ;
- informations manquantes et drapeau de connaissance generale.

Rust verifie que chaque citation appartient au manifest, a une range injectee,
au hash actuel et au meme workspace. En `references_only`, la connaissance
generale est interdite par defaut. Le validateur de conformite bloque aussi les
fichiers non injectes, recommandations interdites, symboles observes sans
support et marqueurs internes tels que commandes Cargo, gates ou benchmarks.

Une sortie invalide autorise au maximum une correction de forme JSON. Une
erreur de preuve ou de contenu ne declenche aucune nouvelle tentative
fonctionnelle. Les evenements `task_compliance_failed` et
`internal_context_leak_detected` sont emis avant le terminal d'echec ; le texte
hostile n'est jamais rendu.

## DocumentFacts

Les questions structurees simples evitent le modele. La V1 lit YAML, JSON,
TOML, properties et XML simple pour :

- cles racine ;
- presence d'une cle exacte ;
- valeur scalaire ;
- ligne de definition.

Le parseur traite commentaires, Unicode, CRLF, cles imbriquees et doublons
explicites. Les cles generiques doivent etre delimitees dans la question pour
eviter de router un mot naturel comme identifiant. La route produit les memes
claims et preuves que le LLM, avec `model_calls=0`.

## Limites

- Une preuve valide garantit la correspondance au snapshot, pas la verite d'une
  inference metier complexe.
- XML est limite aux elements simples ; les namespaces/attributs complexes
  restent sur la route LLM ou `insufficient_evidence`.
- `automatic` reste volontairement large et ne promet pas la meme isolation que
  `references_only`.
- Le grounding n'autorise aucune ecriture et ne modifie pas CHAT-EDIT-001.

Gate : `scripts/run-grounding-quality.ps1`.