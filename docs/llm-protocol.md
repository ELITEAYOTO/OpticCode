# LLM/PROTOCOL-001 - Provider local et protocole machine

## Statut

`LLM/PROTOCOL-001` est implemente. Il ajoute une frontiere provider explicite,
le streaming Ollama reel, l'annulation cooperative et une sortie JSONL
versionnee pour les futures interfaces OpticCode.

Le sprint ne change ni le contexte par defaut, ni le comportement historique de
`ask`, `plan` et `eval`. `legacy` reste le mode par defaut et les appels EVAL
restent non streamables afin de conserver des mesures comparables.

## Perimetre

Inclus :

- contrats provider independants d'Ollama dans `opticcode-llm` ;
- implementation locale `OllamaProvider` ;
- health check, inventaire des modeles, generation et streaming NDJSON ;
- annulation avant envoi et pendant la lecture du flux ;
- evenements provider `opticcode.llm` schema 1 ;
- evenements assistant `opticcode.assistant` schema 1 ;
- sorties CLI `--stream` et `--protocol-jsonl` ;
- validation des identites, request IDs, sequences et terminaux ;
- compatibilite de `OllamaClient` pour les chemins existants.

Exclus : extension VS Code, daemon, MCP, provider distant, llama.cpp, politique
d'outils, boucle agent, nouveau profil, embeddings et nouveau moteur RAG.

## Architecture

```text
opticcode-cli
  -> protocole assistant / rendu humain ou JSONL
  -> opticcode-core
       -> preparation legacy, symbol ou compare
       -> validation et encapsulation des evenements provider
       -> dyn LlmProvider
            -> OllamaProvider
                 -> API locale /api/tags et /api/generate
```

Les responsabilites sont separees :

- `opticcode-llm/src/protocol.rs` contient les schemas provider neutres ;
- `opticcode-llm/src/provider.rs` contient le trait et les canaux bornes ;
- `opticcode-llm/src/ollama.rs` adapte uniquement l'API Ollama locale ;
- `opticcode-core/src/protocol.rs` definit le cycle assistant ;
- `assistant_runtime.rs` compose le contexte et enveloppe le flux provider ;
- la CLI ne parse jamais directement le NDJSON Ollama.

## Trait provider

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn endpoint(&self) -> &str;
    fn capabilities(&self) -> ProviderCapabilities;

    async fn health(&self, request: HealthRequest)
        -> Result<HealthReport, ProviderError>;
    async fn list_models(&self)
        -> Result<Vec<ModelInfo>, ProviderError>;
    async fn generate(
        &self,
        request: GenerationRequest,
        cancellation: CancellationToken,
    ) -> Result<GenerationResult, ProviderError>;
    async fn stream(
        &self,
        request: GenerationRequest,
        events: EventSink,
        cancellation: CancellationToken,
    ) -> Result<GenerationResult, ProviderError>;
}
```

`OpticCode::with_provider` permet l'injection d'un provider sans modifier le
runtime assistant. Les constructeurs historiques restent des raccourcis Ollama.

## Bornes du contrat

| Element | Limite |
| --- | ---: |
| request ID | 128 octets ASCII, alphabet controle |
| nom de modele | 256 octets, aucun caractere de controle |
| prompt provider | 16 Mio |
| sortie generee cumulee | 16 Mio |
| timeout provider | 1 a 3 600 000 ms |
| ligne NDJSON Ollama | 1 Mio |
| capacite d'un canal public | 1 a 4 096 evenements |
| canal assistant par defaut | 64 evenements |
| livraison d'un evenement terminal | 5 secondes |

Les URL Ollama restent limitees a HTTP local (`localhost`, loopback IPv4 ou
IPv6), sans credentials, query, fragment ou chemin API fourni par l'appelant.

## Protocole provider

Chaque evenement contient :

- `schema_version: 1` ;
- `protocol: "opticcode.llm"` ;
- un `request_id` stable ;
- une `sequence` commencant a zero et strictement croissante ;
- un `type` Serde explicite.

Types : `started`, `delta`, `completed`, `failed`, `cancelled`.
`completed`, `failed` et `cancelled` sont terminaux. Un flux valide en contient
exactement un et aucun evenement ne peut le suivre. Le coeur revalide aussi le
provider, la requete, la sequence et le resultat terminal avant exposition.

Exemple abrege :

```json
{"schema_version":1,"protocol":"opticcode.llm","request_id":"ask-1:legacy","sequence":0,"type":"started","provider":"ollama","model":"qwen2.5-coder:14b"}
{"schema_version":1,"protocol":"opticcode.llm","request_id":"ask-1:legacy","sequence":1,"type":"delta","text":"Bonjour"}
{"schema_version":1,"protocol":"opticcode.llm","request_id":"ask-1:legacy","sequence":2,"type":"completed","result":{"schema_version":1,"request_id":"ask-1:legacy","provider":"ollama","model":"qwen2.5-coder:14b","output":"Bonjour","finish_reason":"stop","prompt_chars":100,"usage":{},"timings":{"client_ms":25}}}
```

## Protocole assistant

La CLI expose une seconde enveloppe, car la preparation du contexte appartient
a OpticCode et non au provider. Chaque ligne utilise :

- `protocol: "opticcode.assistant"` ;
- `schema_version: 1` ;
- le request ID de l'appel CLI ;
- une sequence globale stricte.

Cycle normal :

```text
started
-> context_prepared
-> provider_event(started)
-> provider_event(delta) ...
-> provider_event(completed | failed | cancelled)
-> completed | failed | cancelled
```

Le mode `compare` sans `--compare-generate` produit seulement `started`,
`context_prepared`, puis `completed` et ne contacte pas Ollama. Une erreur de
setup survenue avant le runtime produit une unique ligne terminale `failed` de
sequence zero. Le request ID fourni est conserve seulement s'il est valide.
Si ses 128 octets ne laissent pas la place au suffixe provider `:legacy` ou
`:symbol`, OpticCode derive un ID provider court par BLAKE3 ; l'ID assistant
original reste intact dans l'enveloppe externe.

Les metadata de `context_prepared` ne contiennent ni prompt, ni extrait source.
Le texte genere apparait dans les deltas et dans le resultat provider terminal.

## CLI

Streaming humain :

```powershell
cargo run -q -- ask "Explique Helpers#ping()." `
  --path benchmarks/java-index-mini `
  --profile none --no-memory --no-rag --stream
```

Protocole machine :

```powershell
cargo run -q -- plan "Localiser plugin.yml" `
  --path benchmarks/java-index-mini `
  --profile none --no-memory --no-rag `
  --protocol-jsonl --request-id vscode-plan-1
```

Regles de sortie :

- `--stream` imprime chaque delta une seule fois puis les metriques demandees ;
- `--protocol-jsonl` imprime un objet JSON compact par ligne sur stdout ;
- apres parsing CLI reussi, aucune narration humaine n'est ajoutee au JSONL ;
- `--request-id` exige `--protocol-jsonl` ;
- JSONL est incompatible avec `--json`, `--metrics`, `--metrics-json` et
  `--stream` ;
- une requete terminee en `failed` ou `cancelled` retourne le code processus 2.

## Annulation et backpressure

`CancellationToken` est partage entre CLI, coeur et provider. `Ctrl+C` annule
la requete active. Ollama est interrompu pendant l'envoi ou la lecture et le
provider tente toujours d'emettre un terminal `cancelled` borne dans le temps.

Les canaux Tokio bornes imposent la backpressure. Le coeur consomme le flux
provider en concurrence avec la generation, puis la CLI consomme le flux
assistant en concurrence avec le runtime. Aucun buffer de deltas sans limite
n'est ajoute entre Ollama et stdout.

## Compatibilite

- sans nouveau drapeau, `ask` et `plan` gardent la generation non streamee et
  leurs sorties historiques ;
- `--json` et `--metrics-json` conservent leurs schemas existants ;
- `eval --with-llm` continue d'utiliser `generate`, pas `stream` ;
- `legacy`, `symbol` et `compare` gardent leurs regles CONTEXT-002 ;
- aucun choix de provider supplementaire n'est expose dans la CLI.

## Validation

La gate dediee est :

```powershell
.\scripts\run-llm-protocol-quality.ps1
```

Le smoke reel, optionnel, utilise le modele local deja installe sans le
telecharger :

```powershell
.\scripts\run-llm-protocol-quality.ps1 -WithLlm
```

Les tests couvrent notamment health/model info, NDJSON fragmente, reconstruction
des deltas, timeout, serveur absent, modele absent, JSON malforme, annulation
avant et pendant le flux, provider injecte, provider mal sequence, JSONL CLI,
mode humain sans duplication, setup invalide et compare sans reseau.

## Suite

La prochaine etape est `POLICY-001`, separee de ce protocole. Aucune boucle
agent capable d'appeler des outils ou d'ecrire ne doit commencer avant cette
politique deny-by-default et ses contrats d'approbation.
