# LEGACY-002 - Catalogue Bukkit 1.8 prouve

Date de validation : 2026-07-13

## Objectif

LEGACY-002 etend les transformations deterministes de CODE-001B2 sans traiter
un vieux plugin comme une source de verite. Chaque regle expose maintenant :

- un ID stable et un proprietaire Java complet ;
- le membre moderne et sa cible legacy ;
- les versions cibles `1.8.8` et `1.8.9` ;
- un niveau de preuve ;
- les IDs des sources epinglees ;
- une raison metier courte.

Le catalogue est accessible sans analyser de projet :

```powershell
cargo run -q -- java-legacy-rules
cargo run -q -- java-legacy-rules --json
```

Le schema du catalogue et le rule set sont respectivement `2` et
`minecraft_java_1_8_v2`.

## Sources epinglees

Deux JAR sources officiels Spigot presents dans le cache Maven servent de
preuve reproductible :

| Usage | Coordonnees | SHA-256 |
| --- | --- | --- |
| API cible | `org.spigotmc:spigot-api:1.8.8-R0.1-SNAPSHOT:sources` | `f280f22be399e3d08521dfccba7bad2522f7cb2f9e32a27200425d40c37da308` |
| API moderne de reference | `org.spigotmc:spigot-api:1.21.4-R0.1-SNAPSHOT:sources` | `6f8d397dd321817d02d7557e76221c89db27762085480f34d884188491267d0c` |

Le JSON conserve aussi le nom exact de chaque artefact et son URL Spigot. Le
script qualite recalcule les hashes, ouvre `Material.java` et `EntityType.java`
dans les archives et verifie les constantes attendues.

`verified_api_pair` signifie que les deux membres ont ete confirmes dans cette
paire de sources. `verified_legacy_target` est volontairement plus faible : la
cible 1.8 est prouvee, mais le nom moderne historique n'appartient pas a l'API
moderne epinglee. Trois anciennes regles restent dans ce second niveau :
`MONSTER_SPAWNER`, `SPAWN_EGG` et `GOLD_SHOVEL`.

## Extension V2

Le catalogue passe de 14 a 26 regles. Les douze ajouts sont :

| Proprietaire | Moderne | Bukkit 1.8 |
| --- | --- | --- |
| `Material` | `CRAFTING_TABLE` | `WORKBENCH` |
| `Material` | `COBWEB` | `WEB` |
| `Material` | `CLOCK` | `WATCH` |
| `Material` | `FIREWORK_ROCKET` | `FIREWORK` |
| `Material` | `FIREWORK_STAR` | `FIREWORK_CHARGE` |
| `Material` | `NETHER_PORTAL` | `PORTAL` |
| `Material` | `END_PORTAL` | `ENDER_PORTAL` |
| `Material` | `END_PORTAL_FRAME` | `ENDER_PORTAL_FRAME` |
| `EntityType` | `TNT` | `PRIMED_TNT` |
| `EntityType` | `FIREWORK_ROCKET` | `FIREWORK` |
| `EntityType` | `FISHING_BOBBER` | `FISHING_HOOK` |
| `EntityType` | `LIGHTNING_BOLT` | `LIGHTNING` |

Les mappings avec metadata, data values ou perte de variante, par exemple les
bois aplatis ou les spawn eggs specifiques, restent hors catalogue tant que le
contrat de transformation ne peut pas conserver leur semantique.

## Correction de resolution

`FIREWORK_ROCKET` existe a la fois dans `Material` et `EntityType`. Une
recherche basee uniquement sur le nom du membre pouvait choisir la mauvaise
regle. Le moteur selectionne maintenant d'abord l'identite B1 complete :

```text
org.bukkit.Material#FIREWORK_ROCKET
org.bukkit.entity.EntityType#FIREWORK_ROCKET
```

La cible, le remplacement et la preuve du qualificateur restent donc lies au
bon proprietaire. Les homonymes inconnus sont refuses ou ignores, jamais
devines.

## Validation

Le corpus `benchmarks/java-edits-legacy` mesure maintenant :

| Mesure | Resultat |
| --- | ---: |
| fichiers Java | 13 |
| references examinees | 113 |
| noms candidats | 39 |
| cibles Bukkit exactes | 30 |
| propositions attendues | 28 dans 3 fichiers |
| rejets attendus | 11 |
| regles couvertes | 26/26 |
| faux positifs | 0 |

Le fixture `benchmarks/java-legacy-compile` reference les 24 constantes legacy
distinctes et compile reellement en Java 8 contre Spigot API 1.8.8. B3 couvre
aussi les deux proprietaires de `FIREWORK_ROCKET` dans un worktree detache.
La gate workspace atteint 147 tests, Clippy strict et build release valides.

Commande de validation :

```powershell
.\scripts\run-java-legacy-quality.ps1
.\scripts\run-java-legacy-quality.ps1 -Full
```

## Limites

- la presence de deux constantes ne prouve pas a elle seule toutes les
  equivalences de metadata ou comportement runtime ;
- les trois regles `verified_legacy_target` restent visibles comme dette de
  provenance moderne ;
- sons, particules, enchantements, potions, NMS et `plugin.yml` necessitent des
  catalogues separes, car leur forme de migration n'est pas toujours un simple
  renommage enum ;
- aucune regle ne contourne les controles B2/B3, le reparse ou le build.

La prochaine priorite est `CONTEXT-001`, afin d'utiliser l'index symbolique pour
reduire le contexte envoye a Qwen.
