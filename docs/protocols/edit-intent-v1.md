# Edit Intent Protocol v1

`EditIntent` is the trusted, bounded contract created by the OpticCode runtime before any
untrusted model output is accepted as an `EditPlan`.

## Purpose

The intent layer separates two responsibilities:

1. OpticCode decides which repository targets and operations are permitted.
2. The local model proposes concrete byte-range edits inside those trusted boundaries.

An intent never contains replacement text, patches, guessed offsets, or full source files.

## Lifecycle

```text
ChatRequest
  -> resolved references and repository state
  -> validated EditIntent + BLAKE3 intent hash
  -> untrusted model-generated EditPlan
  -> EditPlan validation against the intent
  -> disposable worktree verification
  -> native confirmation
  -> transactional apply or rollback
```

## Trust boundaries

The runtime supplies and validates:

- `request_id`
- `workspace_id`
- `workspace_root_hash`
- `base_head`
- `working_tree_digest`
- allowed existing targets and their content hashes
- explicitly permitted creation paths
- reference IDs
- hard edit limits
- creation and expiry timestamps

The model must not create or alter any of those trusted values.

## Targets

### Existing file

An `existing_file` target is tied to:

- a canonical workspace-relative path;
- a trusted 64-character BLAKE3 content hash;
- zero or more trusted reference IDs;
- a provenance value.

### Prospective file

A `prospective_file` target is allowed only when the runtime explicitly listed the exact
creation path. Its extension must match the path and the allowlisted extension inventory.

## Security requirements

Schema v1 always requires:

- a clean main worktree;
- offline verification;
- native confirmation before apply;
- UTF-8 byte offsets;
- workspace-relative canonical paths;
- no `.git` or `.opticcode` targets;
- no absolute paths, parent traversal, Windows drive/ADS syntax, or non-portable device names;
- limits at or below OpticCode hard maxima;
- a maximum intent lifetime of 15 minutes.

## Canonicalization and hashing

After validation, OpticCode sorts:

- targets by canonical path and target kind;
- reference IDs;
- allowed operations;
- allowed extensions.

The canonical JSON is hashed with BLAKE3. The resulting `intent_hash` is stable for
semantically identical intents whose input array order differs.

## Discovery and chat events

The discovery report advertises an `edit_runtime` capability object. It describes the exact
schema versions, enabled selection modes and operations, validation stages, hard limits, hash
algorithm, and confirmation requirements implemented by the running binary. Clients must not infer
write capabilities from a single boolean.

The Chat protocol emits this ordered intent lifecycle before model generation:

1. `edit_intent_started`
2. `edit_intent_ready`
3. `edit_plan_started`
4. `edit_plan_ready`
5. `proposal_stored`

`edit_intent_ready` exposes only bounded metadata: the intent identity, schema version, BLAKE3
hash, selection mode, target count, and expiry. `proposal_stored` repeats the intent identity and
hash so clients can bind review artifacts to the persisted authority without receiving task text
or source content.

## Versioning

- Protocol schema: `1`
- JSON Schema: `schemas/edit-intent.v1.schema.json`
- Rust source of truth: `crates/opticcode-edit/src/intent.rs`

Breaking wire changes require a new schema version. Schema v1 must remain readable for any
stored proposal that references it.
