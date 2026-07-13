# OpticCode en bref

## Qu'est-ce qu'OpticCode ?

OpticCode est un assistant de developpement local specialise dans Java 8,
Minecraft 1.8.8/1.8.9, Bukkit, Spigot et PandaSpigot. Il utilise un modele deja
entraine, actuellement Qwen2.5-Coder 14B via Ollama, puis ajoute autour de lui
les outils necessaires pour comprendre un projet, rechercher de la documentation,
proposer une modification et verifier son resultat.

OpticCode n'entraine pas son propre modele et ne modifie jamais un projet sans
passer par des controles explicites.

## Comment fonctionne le projet ?

Le fonctionnement cible est le suivant :

```text
demande utilisateur
-> inspection du projet et de Git
-> recherche RAG dans les docs et regles Minecraft legacy
-> analyse syntaxique et symbolique Java
-> contexte borne envoye au modele local
-> proposition de patch
-> application dans un worktree Git jetable
-> compilation et tests avec timeout
-> diff final et validation utilisateur
-> promotion controlee vers le projet original
```

Le coeur est ecrit en Rust. Tree-sitter analyse Java, Ollama execute le modele
local, Git isole les essais et Maven/Gradle compilent les projets Java.

## Protections deja presentes

- etat Git capture avant et apres les operations ;
- commandes bornees par timeout et taille de sortie ;
- apply transactionnel avec rollback et recovery ;
- verrou par workspace et controles Windows contre les jonctions ;
- verification dans un worktree detache ;
- analyse Java read-only avec offsets d'octets, hashes et diagnostics ;
- index inter-fichiers avec resolutions exactes, ambigues ou non resolues ;
- propositions d'edits Java avec hash, ranges, octets attendus et reparse ;
- revalidation et application de ces edits dans un worktree detache ;
- transaction, reparse disque, build borne, hashes Git finaux et cleanup ;
- aucune modification automatique provenant directement du modele.

## Etat actuel

OpticCode sait deja inspecter un projet, construire un contexte, interroger
Qwen, rechercher dans un RAG local, analyser Java, indexer les declarations et
references entre fichiers, proposer certains patches legacy, appliquer une
transaction recuperable, compiler et verifier un patch dans un worktree
temporaire.

La production read-only d'edits Java cibles est disponible pour 26 regles
Bukkit 1.8 avec sources epinglees et compilation legacy. L'index refuse de choisir arbitrairement entre deux classes ou
methodes, puis le moteur verifie hash, qualificateur, octets et syntaxe. Le
pipeline B3 recalcule ensuite ces preuves dans un worktree au `HEAD` exact,
applique transactionnellement, reparse, compile et controle le diff final sans
toucher la source.

## Ce qui reste avant une V1 autonome

- index symbolique incremental et persistant pour les tres grands depots ;
- extension des domaines legacy au-dela des enums deja prouves ;
- RAG scalable avec provenance ;
- boucle agent bornee plan -> tools -> build -> correction ;
- approbation finale et promotion controlee ;
- evaluation qualite, vitesse et consommation de tokens sur des projets reels.

Pour les details techniques, voir la [roadmap](roadmap.md),
l'[architecture](architecture.md), la
[preuve des regles legacy](java-legacy-rules.md), la
[verification B3](java-edit-worktree.md) et
l'[audit complet](project-audit-2026-07-11.md).
