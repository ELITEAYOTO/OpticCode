# OpticCode - Benchmark modele local

Derniere mise a jour : 2026-07-06

## Objectif

Verifier que Qwen2.5-Coder 14B est utilisable localement pour OpticCode avant de coder l'agent.

Ce benchmark doit repondre a quatre questions :

1. Le modele tourne-t-il correctement sur la machine ?
2. La latence est-elle acceptable pour un agent interactif ?
3. La qualite est-elle suffisante pour Java 8 / Bukkit 1.8.8 ?
4. Quel runtime choisir pour le MVP : Ollama, LM Studio ou llama.cpp plus tard ?

## Modele de depart

Modele Ollama recommande pour le premier test :

```powershell
ollama run qwen2.5-coder:14b
```

La page officielle Ollama indique que `qwen2.5-coder:14b` fait environ 9.0 Go avec une fenetre de contexte 32K.

Pour un premier test, ne cherche pas encore a maximiser le contexte. Le but est d'abord de verifier vitesse, stabilite et qualite.

## Verification Ollama

Dans PowerShell :

```powershell
ollama --version
ollama list
ollama run qwen2.5-coder:14b
```

Si le modele n'est pas encore present, Ollama le telechargera.

## Prompts de benchmark

### Test 1 - Java 8 strict

```text
Tu es un assistant Java 8. Ecris une classe Java simple compatible Java 8, sans var, sans records, sans streams modernes complexes, qui represente un joueur avec un nom, un UUID et un nombre de coins. Ajoute getters, setters et une methode addCoins.
```

Resultat attendu :

- pas de syntaxe moderne ;
- imports corrects ;
- code simple et compilable avec Java 8.

### Test 2 - Bukkit 1.8.8 legacy

```text
Tu dois ecrire un listener Bukkit compatible Minecraft 1.8.8. Quand un joueur rejoint, envoie un message de bienvenue et donne-lui un item gunpowder. Attention aux noms legacy de Material en 1.8.8.
```

Resultat attendu :

- utilise `Material.SULPHUR`, pas `Material.GUNPOWDER` ;
- pas d'Adventure API ;
- pas de composants modernes ;
- code Bukkit classique.

### Test 3 - Correction d'erreur

```text
Voici une erreur de compilation Java 8 :

error: cannot find symbol
Material.GUNPOWDER

Le projet cible Bukkit/Spigot 1.8.8. Explique la cause et propose la correction.
```

Resultat attendu :

- explique que `GUNPOWDER` est un nom moderne ;
- propose `Material.SULPHUR` pour 1.8.8.

### Test 4 - Raisonnement agent

```text
Je veux ajouter une commande /coins dans un plugin Bukkit 1.8.8. Avant de coder, donne-moi un plan minimal des fichiers/classes a creer, les erreurs Java 8 a eviter, et les points Bukkit legacy a verifier.
```

Resultat attendu :

- plan clair ;
- prudence Java 8 ;
- mention `plugin.yml` ;
- mention `CommandExecutor` ;
- pas d'API moderne.

## Mesures a noter

Pour chaque test, noter :

| Mesure | Valeur |
| --- | --- |
| Runtime | Ollama / LM Studio / autre |
| Modele | qwen2.5-coder:14b |
| Temps avant debut de reponse | approximatif |
| Temps total | approximatif |
| Qualite Java 8 | OK / moyen / mauvais |
| Qualite Bukkit 1.8.8 | OK / moyen / mauvais |
| Erreurs hallucinees | oui / non |
| Remarques | texte libre |

## Decision attendue

Apres benchmark :

- choisir runtime principal du MVP ;
- choisir modele principal ;
- definir contexte de depart ;
- decider si LM Studio doit etre compare ;
- decider si llama.cpp doit etre compile plus tard.

