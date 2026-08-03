# OpticCode evaluation corpus

`context-retrieval-v1.json` is the canonical EVAL-001 suite.

- 45 stable cases;
- five balanced categories;
- only synthetic, versioned fixtures;
- optional external aliases `pandaspigot` and `kspawners`;
- no personal absolute path;
- read-only execution with before/after fingerprints.

Run the release gate from the repository root:

```powershell
.\scripts\run-eval-quality.ps1
```

Generated JSON and Markdown reports are written under `benchmarks/runs/eval/`
and remain ignored by Git. See `docs/evaluation.md` for metric definitions and
baseline comparison commands.
