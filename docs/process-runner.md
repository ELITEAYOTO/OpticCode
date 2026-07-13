# OpticCode - Process runner borne

Derniere mise a jour : 2026-07-11

Statut : implemente et valide sur Windows 10.

## Objectif

`PROC-001` fournit une execution commune et bornee pour Maven, Gradle et les
futurs outils lances par OpticCode. Un agent ne doit jamais pouvoir attendre
indefiniment un build, accumuler une sortie illimitee ou abandonner un
`java.exe` apres avoir tue seulement `cmd.exe`.

Le code est isole dans :

```text
crates/opticcode-tools/src/process_runner.rs
crates/opticcode-tools/src/process_runner/windows.rs
```

La couche Win32 reste compilee uniquement sous Windows via `windows-sys`.
Aucune dependance Python, Node.js ou service externe n'est ajoutee au runtime.

## API Rust

Types publics principaux :

```rust
ProcessRequest
ProcessResult
ProcessStatus
ProcessOutputStats
ProcessTermination
CancellationToken
```

Un `ProcessRequest` contient :

- le programme et ses arguments separes ;
- le repertoire de travail ;
- le timeout ;
- la limite de sortie retenue par flux ;
- le mode de lancement direct ou script de commande Windows ;
- des variables d'environnement controlees par l'appelant.

Les quatre statuts stables sont :

```text
success
failed
timed_out
cancelled
```

L'annulation est distincte du timeout. `CancellationToken` est clonable et
peut etre declenche depuis le futur orchestrateur agent. Une annulation deja
active empeche le lancement du processus. Pour `opticcode build`, `Ctrl+C`
declenche le token, laisse le runner terminer l'arbre, puis produit un statut
`cancelled`.

## Valeurs bornees

| Parametre | Valeur |
| --- | ---: |
| timeout par defaut | 600 s |
| timeout maximal | 3 600 s |
| sortie retenue par defaut | 1 048 576 octets par flux |
| sortie retenue maximale | 16 777 216 octets par flux |
| frequence de polling | 20 ms |
| attente maximale de fermeture d'un pipe | 2 s |

La limite porte sur les octets conserves, pas sur les octets lus. Deux threads
drainent `stdout` et `stderr` en parallele jusqu'a EOF pour eviter le deadlock.
Quand une limite est depassee, seul le tail borne reste en memoire, tandis que
les compteurs conservent le volume total observe.

Le resultat expose separement :

- octets lus et retenus pour chaque flux ;
- troncature de `stdout` et `stderr` ;
- erreurs eventuelles de capture ;
- code de sortie et duree ;
- timeout ou annulation ;
- strategie et resultat de terminaison.

## Arbre de processus Windows

Sous Windows, le runner cree un Job Object avec :

```text
JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
```

Flux :

```text
creer le Job Object
  -> lancer le processus racine
  -> l'assigner immediatement au Job Object
  -> drainer stdout/stderr
  -> attendre succes, erreur, timeout ou annulation
  -> TerminateJobObject en cas d'arret demande
  -> fermer le Job Object
  -> attendre le processus racine et finaliser le rapport
```

Les enfants heritent du Job Object par defaut. Cela couvre le chemin cible :

```text
cmd.exe -> mvn.cmd -> java.exe
```

La fermeture du dernier handle avec `KILL_ON_JOB_CLOSE` sert aussi de garde-fou
si le processus racine termine alors qu'un descendant reste actif.

Le test Windows ne se contente pas de verifier le parent. Une fixture lance un
second processus, publie son PID, force le timeout, puis utilise
`OpenProcess`/`GetExitCodeProcess` pour confirmer que le descendant n'est plus
actif.

## Integration build

`build_java_project_with_options` utilise maintenant le runner. Une variante
`build_java_project_with_cancellation` expose l'annulation au futur core agent.

Options CLI :

```powershell
cargo run -q -- build --path benchmarks/mini-bukkit-plugin `
  --timeout-seconds 600 `
  --output-limit-bytes 1048576 `
  --json
```

Le mode strict Git reste combinable :

```powershell
cargo run -q -- build --path benchmarks/mini-bukkit-plugin `
  --fail-on-worktree-change `
  --timeout-seconds 600 `
  --json
```

Le code de sortie CLI reste non nul pour `failed`, `timed_out`, `cancelled` ou
une violation Git stricte.

## JSON

Les champs de build existants restent presents. Le schema versionne ajoute un
objet `process` :

```json
{
  "schema_version": 1,
  "build_success": false,
  "overall_success": false,
  "process": {
    "process_id": 1234,
    "status": "timed_out",
    "timed_out": true,
    "cancelled": false,
    "timeout_ms": 1000,
    "output": {
      "limit_bytes_per_stream": 1024,
      "stdout_bytes": 14,
      "stderr_bytes": 0,
      "stdout_retained_bytes": 14,
      "stderr_retained_bytes": 0,
      "stdout_truncated": false,
      "stderr_truncated": false,
      "output_truncated": false,
      "capture_errors": []
    },
    "termination": {
      "attempted": true,
      "succeeded": true,
      "strategy": "windows_job_object",
      "error": null
    }
  }
}
```

`status` reste l'autorite pour distinguer une erreur normale, un timeout et une
annulation. Le code de sortie apres `TerminateJobObject` n'est pas utilise pour
deviner la cause.

## Tests couverts

- processus court en succes avec `stdout` et `stderr` ;
- code de sortie 7 ;
- sortie superieure aux deux limites sans deadlock ;
- processus bloque termine par timeout ;
- annulation pendant l'execution ;
- annulation avant spawn ;
- refus d'un timeout superieur a une heure avant spawn ;
- refus d'une limite de sortie excessive avant spawn ;
- descendant Windows confirme inactif apres timeout ;
- vrai binaire CLI avec faux Maven bloque et JSON `timed_out` ;
- non-regression du Build Git State Guard strict.

Toutes les fixtures sont temporaires. Aucun plugin original, PandaSpigot ou
pack de ressources n'est modifie par ces tests.

Validation globale du workspace : 63 tests reussis.

Validation Maven reelle sur la copie dediee Kspawners :

| Mesure | Resultat |
| --- | ---: |
| statut CLI/processus | `success` |
| code de sortie | 0 |
| duree | 3 056 ms |
| timeout configure | 120 000 ms |
| limite par flux | 65 536 octets |
| sortie tronquee | non |
| terminaison demandee | non |
| strategie Windows | `windows_job_object` |

Le snapshot Git a classe les 61 entrees de la copie comme preexistantes et
n'a detecte aucun changement supplementaire pendant ce build. L'original
`C:\Users\timot\Desktop\minecraft\SparrowMCALL\Kspawners` n'a pas ete touche.

## Limites assumees

- Sous les plateformes non Windows, le fallback tue actuellement le processus
  racine, pas un process group complet. La cible produit actuelle reste
  Windows 10.
- Sous Windows, l'assignation au Job Object arrive immediatement apres le
  `spawn`. Ce modele est adapte aux outils de build de confiance. Un futur
  support de binaires non fiables devrait les placer dans le job au moment de
  leur creation.
- Le mode `WindowsCommandScript` est reserve aux commandes internes connues
  comme Maven/Gradle. Aucun argument CLI ne permet de soumettre une commande
  shell arbitraire.
- Les commandes Git historiques n'ont pas ete migrees dans ce sprint. Le
  runner devient la voie obligatoire pour les nouveaux outils longs.

## Suite

`APPLY-001` et `GIT-002` sont maintenant termines ; voir
[`apply-transaction.md`](apply-transaction.md) et
[`worktree-verification.md`](worktree-verification.md). Le runner borne les
commandes Git et le build du worktree jetable.
