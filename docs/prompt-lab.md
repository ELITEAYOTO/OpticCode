# VS Code Prompt Lab

Derniere mise a jour : 2026-08-04

## Niveau d'integration

Le Prompt Lab lance VS Code 1.131.0 via `@vscode/test-electron`, active
l'extension de developpement et invoque exactement le callback passe a
`vscode.chat.createChatParticipant`. Il utilise :

- le participant enregistre `opticcode.chat` ;
- le normaliseur reel de references ;
- de vrais `vscode.Uri`, `vscode.Location`, `vscode.Range` et
  `CancellationTokenSource` ;
- `OpticCodeService`, `OpticCodeProtocolClient`, le vrai `opticcode.exe` release
  et le protocole stdin/NDJSON ;
- le presenter, les boutons de preuves et le collecteur de metriques reels.

L'API stable ne permet pas de saisir programmatiquement une requete dans le
champ visuel Chat. Aucune commande privee `workbench.*` et aucune proposed API
n'est utilisee. Le champ visuel reste donc un smoke manuel guide ; le rapport
porte explicitement `visible_chat_input_automated: false`.

## Isolation de la fixture

`benchmarks/grounding-plugin` est versionne. Le runner le copie dans un dossier
temporaire verifie avant le test, modifie seulement cette copie pour le cas de
hash, puis la supprime avec un garde de chemin. Aucun projet personnel n'est lu
ou modifie. Les rapports vont dans `benchmarks/runs`, ignore par Git et exclu du
VSIX.

## Provider mock

Le mock implemente uniquement les endpoints Ollama loopback necessaires. Le
vrai binaire recoit son URL par un hook Prompt Lab actif seulement lorsque
`OPTICCODE_PROMPT_LAB=1`; le client refuse tout protocole autre que HTTP et tout
hostname non loopback. Le mock ne conserve ni prompt ni contenu source.

Matrice verte du 2026-08-04 :

- P1 cles exactes et `api-version absent` ;
- P2 nouvelle tache dans la meme session ;
- P3 meme chemin, nouveau hash/fingerprints ;
- P4 zero fuite Java ;
- P5 zero recommandation/connaissance generale ;
- P6 commande interne refusee avant rendu ;
- P7 citation cross-file refusee ;
- P8 horloges handler/client/runtime ;
- P9 evenement tardif refuse par le client ;
- P10 deux requetes concurrentes sans fuite croisee.

Le premier rapport vert compte 15 runs Extension Host : dix routes
`document_facts` sans modele, trois generations mock (valide, fuite interne,
preuve invalide) et deux branches concurrentes. Les seuils deterministes sont
100 % de conformite attendue, 0 % de fuite cross-file/interne et zero citation
invalide acceptee.

## Cas de reserve

Les cinq holdouts ne servent pas a ajuster le prompt :

1. YAML avec `commands` imbrique ;
2. JSON avec cle absente ;
3. TOML avec table imbriquee ;
4. YAML Unicode ;
5. range de deux lignes au milieu d'un YAML.

Ils ne contiennent aucun nom special-cas dans le runtime. Les cinq passent la
route deterministe avec zero appel modele.

## Qwen local

Le mode Qwen utilise uniquement `qwen2.5-coder:14b` deja installe,
temperature 0, seed 42, timeout explicite et fixture temporaire. Aucun download
n'est autorise. Les taches YAML restent sur `DocumentFacts`; les cas source Java
et preuve insuffisante exercent la route LLM. Les resultats exacts du run final
sont conserves dans `benchmarks/runs/prompt-lab-qwen.json` et resumes dans le
rapport de sprint.

Run final : sept requetes vertes. Les cinq taches `DocumentFacts` ont effectue
zero appel modele et affichent donc zero token modele. La recherche source-only
de `Material.SULPHUR` a pris 9 340,19 ms au mur, dont 9 302,54 ms jusqu'au
contenu, avec 879 tokens de prompt et 205 tokens de sortie. Le refus par manque
de preuve a pris 8 848,60 ms au mur, dont 8 810,50 ms jusqu'au contenu, avec
866 tokens de prompt et 197 tokens de sortie. Face au baseline fautif de 1 992
tokens, ces prompts stricts sont plus petits de 55,9 % et 56,5 %. Les deux
sorties ont des preuves valides, sans fichier non injecte ni marqueur interne.

## Commandes

```powershell
.\scripts\run-vscode-prompt-lab.ps1 -Mock -WithExtensionHost
.\scripts\run-vscode-prompt-lab.ps1 -Holdout
.\scripts\run-vscode-prompt-lab.ps1 -WithQwen
.\scripts\run-vscode-prompt-lab.ps1 -Full
```

`-Full` execute mock, holdouts et Qwen. Le mode Qwen echoue clairement si le
modele n'est pas deja present.

## Smoke manuel restant

Apres installation du VSIX, ouvrir la fixture, joindre `plugin.yml`, lancer
`@opticcode /inspect` puis le prompt strict `/ask`. Verifier Scope,
Injected references, Evidence, `Show Injected Context`, `Show Evidence`, les
trois durees UI et l'absence de classe Java/Cargo. Cette etape est documentee,
mais ne sera jamais declaree automatisee.
