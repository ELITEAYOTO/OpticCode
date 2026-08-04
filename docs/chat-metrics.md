# CHAT-METRICS - Horloges et durees

Derniere mise a jour : 2026-08-04

## Cause de l'ancien ecart

L'extension affichait essentiellement `metrics.total_ms` sous un libelle
generique `Duration`. Cette valeur etait calculee dans Rust et couvrait son
pipeline complet ; elle ne mesurait ni le debut du handler VS Code, ni le
premier contenu effectivement rendu, ni le dernier contenu visible. Un temps
provider ou un post-traitement pouvait donc etre presente comme temps de
reponse percu. Le protocole validait deja les request IDs : l'enquete n'a pas
montre un cache volontaire de metriques, mais un perimetre et un libelle faux.

Le baseline Qwen de la fixture illustrait aussi le cout reel : 75 662,5 ms au
mur, 75 649 ms dans Rust et environ 72 788 ms avant contenu, pour 1 992 tokens
de prompt. Ce run etait effectivement lent ; le probleme UI distinct etait de
ne pas permettre de separer les phases.

## Trois horloges non melangees

### Rust

`std::time::Instant`, unite milliseconde : resolution de references, contexte,
construction du prompt, provider, validation et total runtime. Les durees
Ollama en nanosecondes sont converties explicitement avant publication.

### Client processus TypeScript

`performance.now()` monotone : debut spawn, processus cree, requete ecrite,
premier evenement protocole, premier/dernier delta, terminal recu et processus
termine.

### Handler Extension Host

`performance.now()` monotone : debut du handler, premier/dernier contenu rendu,
terminal rendu, rapport persiste et handler termine. Les timestamps absolus de
deux processus ne sont jamais soustraits entre eux ; seules leurs durees
locales sont comparees.

Chaque rapport porte le `request_id`, le run, le workspace, l'horloge, l'unite
et le composant mesureur.

## Libelles UI

- `First token` : debut handler -> premier contenu rendu ;
- `Answer streaming` : premier contenu -> dernier contenu ;
- `Visible response` : debut handler -> dernier contenu utile ;
- `Total pipeline` : debut handler -> terminal final rendu ;
- `Model` : duree provider rapportee et validee ;
- `Context build` : construction du contexte Rust ;
- `Post-processing` : fin du contenu visible -> terminal rendu.

Les phases qui se chevauchent ne sont pas sommees. Le rapport complet conserve
les details techniques ; l'UI normale n'affiche que les libelles utiles.
`DocumentFacts` affiche `0 model tokens` ; son prompt structure eventuellement
prepare reste seulement une estimation technique et n'est jamais envoye au
provider.

## Invariants

- valeurs finies et positives ou nulles lorsque non observables ;
- meme `request_id` a tous les niveaux ;
- `first_token <= visible_response <= total_pipeline` ;
- `answer_streaming <= visible_response` ;
- aucun evenement, terminal ou metric apres terminal ;
- une ancienne requete ne peut pas ecrire dans l'etat de la nouvelle ;
- tolerance de comparaison : `max(250 ms, 10 %)`.

Le client rejette un evenement tardif avec `terminal_duplicate` avant de le
transmettre au presenter. Un ancien request ID est refuse avec
`request_mismatch`.

## Validation mesuree

Le Prompt Lab mock injecte 350 ms de delai provider. Dans le run final, le vrai
handler Extension Host a mesure 439,14 ms jusqu'au premier et dernier contenu,
et 441,65 ms jusqu'au terminal. Le surcout inclut spawn, verification de
modele, protocole, snapshot et validation. Les labels affiches et l'horloge
independante respectent la tolerance.

Un test synthetique separe aussi une reponse visible de 3,0 s d'un terminal a
23,05 s, soit 20,05 s de post-traitement. Il verifie la semantique sans imposer
20 secondes d'attente a chaque gate.

Gate : `scripts/run-chat-metrics-quality.ps1`.
