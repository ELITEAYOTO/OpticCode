# CODE-001B2 legacy edit corpus

This corpus measures read-only Java edit proposals for Bukkit 1.8.8 names.
It intentionally mixes eligible references with comments, strings, custom
types, ambiguous imports, static imports and lexically shadowed qualifiers.

Expected baseline:

- 28 validated proposals across three files;
- all 26 versioned legacy rules represented at least once;
- zero proposal in `negative/`;
- source files byte-identical before and after analysis.

The corpus contains more than 70 positive or negative symbol occurrences. A
notable regression is `CreatureSpawnEvent.SpawnReason.SPAWNER`, observed in the
real Kspawners project: it must never be rewritten as `Material.MOB_SPAWNER`.

The files are syntax fixtures. They are not meant to compile against one
single Bukkit version because the modern names and their 1.8 replacements are
deliberately present together.
