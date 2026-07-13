# LEGACY-002 compile fixture

This Java 8 fixture references every distinct Bukkit 1.8.8 target emitted by
the deterministic legacy rule catalog. It must compile against the pinned
`org.spigotmc:spigot-api:1.8.8-R0.1-SNAPSHOT` artifact.

Run it offline after the dependency has been cached:

```powershell
mvn -q -o -f benchmarks/java-legacy-compile/pom.xml test
```
