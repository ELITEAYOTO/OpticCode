# OpticCode - Resultats benchmark modele local

Derniere mise a jour : 2026-07-06

## Contexte

Benchmark initial du modele local pour verifier si OpticCode peut demarrer avec un runtime simple avant d'integrer llama.cpp directement.

## Configuration testee

| Element | Valeur |
| --- | --- |
| Runtime | Ollama API locale |
| Version Ollama | 0.31.1 |
| Modele | qwen2.5-coder:14b |
| Taille locale | 9.0 GB |
| Machine | Ryzen 7 3700X, 32 GB RAM, AMD Radeon RX 9060 XT 16 GB |
| OS | Windows 10 |

Commande API utilisee pour les tests propres :

```powershell
Invoke-RestMethod -Method Post -Uri 'http://localhost:11434/api/generate'
```

La CLI interactive `ollama run` fonctionne, mais elle ajoute des sequences d'affichage et des spinners qui rendent les sorties moins propres pour un benchmark automatise.

## Resultats

### Test 1 - Java 8 strict

Objectif : generer une classe Java 8 simple representant un joueur avec nom, UUID et coins.

Resultat :

- statut : OK ;
- code compatible Java 8 ;
- pas de `var`, pas de records, pas d'API moderne evidente ;
- classe simple avec champs, getters, setters et `addCoins`.

Remarque :

- premier chargement du modele lent ;
- sortie CLI trop bruitee pour etre gardee comme format de reference.

### Test 2 - Bukkit 1.8.8 legacy

Objectif : generer un listener Bukkit 1.8.8 qui donne de la gunpowder au join.

Resultat :

- statut : OK ;
- utilise `Material.SULPHUR`, attendu pour Bukkit/Spigot 1.8.8 ;
- utilise `PlayerJoinEvent`, `sendMessage`, `ItemStack` et `registerEvents` ;
- pas d'Adventure API ni de composants modernes.

Mesures :

| Mesure | Valeur |
| --- | --- |
| Temps total | 22.16 s |
| Tokens generes | 589 |
| Temps generation | 21.77 s |
| Debit approx. | 27 tokens/s |

Conclusion test 2 :

Le modele connait certains details Bukkit legacy, notamment `Material.SULPHUR`, quand le prompt insiste explicitement sur Minecraft 1.8.8.

### Test 3 - Correction d'erreur legacy

Objectif : expliquer l'erreur `cannot find symbol Material.GUNPOWDER` sur Bukkit/Spigot 1.8.8.

Resultat :

- statut : mauvais sur la correction ;
- le modele comprend qu'il y a un probleme de compatibilite entre versions ;
- il ne propose pas la bonne correction `Material.SULPHUR` ;
- il hallucine une solution autour de `Material.GLOWSTONE_DUST.getId()` et d'un ID materiau incorrect.

Mesures :

| Mesure | Valeur |
| --- | --- |
| Temps total | 17.71 s |
| Tokens generes | 475 |
| Temps generation | 17.30 s |
| Debit approx. | 27 tokens/s |

Correction attendue :

```java
new ItemStack(Material.SULPHUR, 1);
```

Conclusion test 3 :

Le modele seul n'est pas assez fiable pour les mappings legacy. OpticCode devra integrer une base documentaire stricte avec des regles Bukkit/Spigot 1.8.8, des exemples valides et des mappings de noms modernes vers noms legacy.

### Test 4 - Raisonnement agent avant code

Objectif : demander un plan minimal pour ajouter une commande `/coins` dans un plugin Bukkit 1.8.8.

Resultat :

- statut : moyen ;
- plan global exploitable ;
- mentionne `plugin.yml`, une classe principale et une classe de commande ;
- mentionne `CommandExecutor` et les precautions autour de `parseInt` ;
- propose une structure raisonnable pour un premier plugin.

Problemes :

- ajoute `api-version: 1.8` dans `plugin.yml`, ce qui n'est pas correct pour Bukkit/Spigot 1.8.8 legacy ;
- ne signale pas assez clairement les pieges classiques : `getCommand(...)` peut etre `null`, gestion console vs joueur, permissions, messages, compatibilite encodage.

Mesures :

| Mesure | Valeur |
| --- | --- |
| Temps total | 47.10 s |
| Tokens generes | 1272 |
| Temps generation | 46.72 s |
| Debit approx. | 27 tokens/s |

Conclusion test 4 :

Le modele est utile pour produire une premiere structure, mais OpticCode devra relire ses propositions avec des regles projet : Java 8 strict, `plugin.yml` legacy, pas d'`api-version`, pas d'API Bukkit moderne.

## Synthese

| Critere | Evaluation |
| --- | --- |
| Installation modele | OK |
| Ollama API locale | OK |
| Vitesse apres chargement | Acceptable pour MVP |
| Java 8 general | OK |
| Bukkit 1.8.8 general | Moyen a OK selon prompt |
| Mappings legacy precis | Insuffisant sans RAG |
| Pertinence pour OpticCode | Oui, avec garde-fous |

## Decision provisoire

Pour le MVP experimental, OpticCode peut demarrer avec Ollama via API locale.

Cette decision ne remplace pas llama.cpp pour la suite. Elle permet simplement de construire et tester les couches agent, outils, memoire et RAG sans bloquer sur l'optimisation runtime.

## Regles a integrer tot dans OpticCode

- Toujours rappeler la cible Java 8 dans les prompts systeme.
- Toujours rappeler Bukkit/Spigot/PandaSpigot 1.8.8 quand le projet est legacy.
- Interdire `api-version` dans `plugin.yml` pour les plugins 1.8.8.
- Preferer `Material.SULPHUR` a `Material.GUNPOWDER` en 1.8.8.
- Ajouter une base de mappings legacy verifiee.
- Ajouter des tests de compilation Java 8 des que possible.
- Ne jamais considerer la sortie du modele comme fiable sans verification quand elle touche aux noms d'API legacy.

## Prochaine etape recommandee

Passer a la Phase 3 : recherche ciblee des depots externes, en commencant par les sources qui aident directement le MVP :

1. Ollama API / schemas de requetes locales.
2. Qwen Code pour l'architecture agentique.
3. llama.cpp pour GGUF, serveur local et options Vulkan.
4. Tree-sitter Java pour l'analyse de code.
5. Tantivy pour l'index texte local.
