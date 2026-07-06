# OpticCode - Scan resource packs

Derniere mise a jour : 2026-07-06

## Objectif

Cette page documente le premier inventaire read-only des resource packs Minecraft 1.8 utiles au futur RAG OpticCode.

Le but n'est pas de copier les packs dans le depot. Ils restent a leur emplacement d'origine. OpticCode lit seulement leur structure pour savoir quelles donnees pourront etre indexees plus tard.

## Commande ajoutee

```powershell
cargo run -q -- pack-scan --path "<chemin-du-pack>" --limit 25
```

La commande affiche :

- presence de `pack.mcmeta` ;
- nombre total de fichiers ;
- repartition par categories ;
- chemins legacy utiles autour de spawners, nether wart/stalk, shovels/spades et spawn eggs.

## Packs testes

### Resource pack legacy 1.8

Chemin :

```text
C:\Users\timot\Desktop\RAG-1.8-Minecraft\1.8-JavaDoc\resource-pack-1.8\LegacyPack
```

Resultat observe :

```text
pack.mcmeta: yes
Files: 3817
```

Categories principales :

```text
assets/other: 1194
models/block: 1080
models/item: 515
textures/blocks: 382
blockstates: 340
textures/items: 229
lang: 75
root/other: 2
```

Exemples de chemins legacy detectes :

```text
assets\minecraft\blockstates\mob_spawner.json
assets\minecraft\blockstates\nether_wart.json
assets\minecraft\models\block\mob_spawner_cage.json
assets\minecraft\models\block\nether_wart_stage0.json
assets\minecraft\models\item\diamond_shovel.json
assets\minecraft\models\item\mob_spawner.json
assets\minecraft\models\item\spawn_egg.json
assets\minecraft\textures\blocks\mob_spawner.png
assets\minecraft\textures\items\nether_wart.png
assets\minecraft\textures\items\spawn_egg.png
```

### Pack custom Volkaria

Chemin :

```text
C:\Users\timot\Desktop\minecraft\Volkaria\Pack-Volkaria
```

Resultat observe :

```text
pack.mcmeta: yes
Files: 2407
```

Categories principales :

```text
assets/other: 1813
mcpatcher/cit: 382
textures/blocks: 151
textures/items: 50
root/other: 9
lang: 2
```

Exemples de chemins legacy detectes :

```text
assets\minecraft\mcpatcher\cit\Autres\piece_spawners\piece_spawners.properties
assets\minecraft\mcpatcher\cit\item_gui\spawners\mobs-head\cochon_zombie.properties
assets\minecraft\mcpatcher\cit\item_gui\spawners\mobs-head\creeper.properties
assets\minecraft\mcpatcher\cit\item_gui\spawners\mobs-head\enderman.properties
assets\minecraft\mcpatcher\cit\item_gui\spawners\mobs-head\zombie.png
```

## Interpretation

Le pack legacy est utile pour verifier les noms vanilla 1.8 : blockstates, models, textures, fichiers `.lang`.

Le pack Volkaria est surtout utile comme contexte metier custom : CIT, spawners custom, noms d'items, assets d'interface, conventions visuelles.

Pour le RAG, il faut eviter d'injecter les images elles-memes dans le prompt. Les premiers index doivent plutot stocker :

- chemins de fichiers ;
- noms d'assets ;
- contenu texte des `.lang`, `.properties`, `.json`, `.mcmeta` ;
- liens entre noms legacy et usage metier.

## Prochaine etape

Ajouter un premier index local texte/metadata pour les documents et resource packs, puis mesurer :

- taille de l'index ;
- temps d'indexation ;
- temps de recherche ;
- nombre de caracteres injectes dans le prompt ;
- impact sur la qualite de reponse Qwen.
