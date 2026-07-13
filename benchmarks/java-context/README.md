# CONTEXT-001 benchmark

This benchmark compares the symbol-guided Java selector with
`legacy_file_priority_v1`, the previous fixed file-priority context builder.

The five tasks cover an exact overload and caller, an ambiguous simple name,
`plugin.yml`, `pom.xml`, and an unresolved symbol. `tasks.json` pins expected
symbols, snippet roles, and known-noise exclusions. The quality script records:

- selected files and snippets;
- rendered characters and estimated tokens;
- legacy baseline files and estimated tokens;
- construction time and every truncation flag;
- required-symbol/role misses and forbidden-noise hits.

Run from the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/run-java-context-quality.ps1
```

Use `-Full` to include the complete workspace test and release-build gate. The
token estimator is deliberately simple (`ceil(unicode_chars / 4)`), so these
numbers compare prompt size reproducibly; they do not prove that Qwen answers
better. A later LLM evaluation must measure answer quality separately.
