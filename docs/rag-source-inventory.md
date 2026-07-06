# OpticCode - Inventaire sources RAG

Derniere mise a jour : 2026-07-06

## Objectif

Cette page documente le premier inventaire read-only des projets externes qui pourront alimenter le RAG OpticCode.

Regle importante : ces projets ne sont pas modifies. OpticCode les scanne seulement pour compter les fichiers utiles et reperer les sources a indexer plus tard.

## Commande ajoutee

```powershell
cargo run -q -- rag-scan --limit 8 --path "<chemin-projet-1>" --path "<chemin-projet-2>"
```

La commande ignore volontairement les dossiers de dependances et de build :

```text
.git, .gradle, .idea, .settings, .vscode, target, build, bin, classes, out, lib, libs, node_modules
```

Elle affiche :

- nombre total de fichiers retenus ;
- nombre de fichiers texte indexables ;
- volume texte indexable ;
- principales extensions ;
- fichiers importants : `pom.xml`, `plugin.yml`, configs, docs, patches.

## Sources plugins avancees

| Source | Fichiers retenus | Fichiers indexables | Taille texte indexable | Notes |
| --- | ---: | ---: | ---: | --- |
| `Kanneau` | 16 | 16 | 63 737 o | petit plugin, configs/messages/plugin.yml |
| `Kchat` | 43 | 43 | 454 155 o | chat, channels, filters, tags |
| `Kclassement` | 35 | 35 | 171 460 o | classement, README, config |
| `Kcraft` | 57 | 56 | 410 156 o | docs + crafts YAML |
| `Kfaction` | 118 | 118 | 865 460 o | gros plugin faction, levels, quests |
| `Kgui` | 89 | 89 | 750 726 o | menus/configs GUI nombreux |
| `KjobsUltimate` | 93 | 93 | 1 095 582 o | docs, jobs, GUI, quests |
| `Kminerai` | 8 | 8 | 37 082 o | petit plugin minerai |
| `Kspawners` | 41 | 41 | 402 473 o | spawners, messages, config |
| `Kenchantement` | 31 | 30 | 221 198 o | enchantements, messages |

Ces plugins sont une bonne base metier pour :

- conventions de commandes Bukkit ;
- patterns de configs YAML ;
- menus GUI ;
- spawners custom ;
- jobs, factions, crafts, classement ;
- style de code serveur existant.

## Fork PandaSpigot

Chemin :

```text
C:\Users\timot\Desktop\KhopeSpigot\PandaSpigot-Fork\PandaSpigot
```

Resultat observe apres filtrage :

```text
Files: 10080
Indexable text files: 10031
Indexable text bytes: 44905279
Skipped large text files: 0
```

Extensions principales :

```text
java: 8933
patch: 1002
sh: 26
yml: 19
xml: 16
md: 12
kts: 7
csrg: 6
lang: 4
```

Interpretation :

- PandaSpigot est trop gros pour etre injecte directement dans un prompt ;
- les fichiers `.patch` sont tres importants pour comprendre les modifications serveur ;
- les sources Java doivent etre indexees par recherche texte et plus tard par symboles ;
- les mappings et ressources doivent rester metadata/index, pas prompt brut.

## Priorite d'indexation V1

Ordre recommande :

1. Docs OpticCode et regles legacy.
2. Configs et docs des plugins avances.
3. Sources Java des plugins avances.
4. Resource packs : `.lang`, `.properties`, `.json`, chemins d'assets.
5. PandaSpigot patches.
6. PandaSpigot sources Java ciblees par recherche, pas tout le depot en prompt.

## Prochaine etape

Ajouter un premier index local qui ecrit dans `data/index/` :

- metadata JSONL des documents ;
- contenu texte chunked ;
- source path + hash + taille ;
- type de source : doc, plugin, resource-pack, pandaspigot ;
- premiere recherche locale rapide avant embeddings.
