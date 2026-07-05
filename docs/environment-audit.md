# OpticCode - Etat de l'environnement Windows 10

Derniere mise a jour : 2026-07-06

## Objectif

Ce document fige l'etat actuel du PC pour demarrer OpticCode proprement.

OpticCode vise un agent code local en Rust/C++, utilisant un modele local de type Qwen2.5-Coder 14B GGUF Q4_K_M, avec RAG local et specialisation Java 8 / Minecraft 1.8.8 / PandaSpigot.

## Configuration machine

| Element | Etat |
| --- | --- |
| OS | Windows 10 Home 64 bits |
| CPU | Ryzen 7 3700X |
| RAM | 32 Go |
| GPU | AMD Radeon RX 9060 XT 16 Go |
| Stockage | SSD |

## Outils valides

| Outil | Etat | Version / remarque |
| --- | --- | --- |
| Git | OK | 2.51.0.windows.1 |
| Rust | OK | rustc 1.94.0 |
| Cargo | OK | cargo 1.94.0 |
| Rustup | OK | 1.28.2 |
| Visual Studio Build Tools 2022 | OK | installe dans `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools` |
| CMake | OK | 4.3.4 |
| Ninja | OK | 1.13.2 |
| JDK 8 | OK | Temurin OpenJDK 1.8.0_482 |
| Maven | OK | 3.9.9, utilise maintenant Java 8 |
| Node.js | OK | v22.15.1, utile seulement pour analyser certains projets |
| Python | OK | 3.11.9, utile seulement pour scripts/outils annexes |
| Ollama | OK | installe via winget, version 0.31.1 |
| LM Studio CLI | OK | `lms` present |
| Vulkan | OK | `vulkaninfo` present |
| OpenCL AMD | OK | `clinfo` detecte AMD Accelerated Parallel Processing |
| SQLite | OK | sqlite3 3.50.4 |
| VS Code | OK | present |
| IntelliJ IDEA | OK | present |

## Points corriges

- `JAVA_HOME` pointait vers JDK 23.
- Maven utilisait Java 23.
- Correction effectuee : Java, javac et Maven utilisent maintenant JDK 8.

Etat attendu confirme :

```text
java -version  -> openjdk version "1.8.0_482"
javac -version -> javac 1.8.0_482
mvn -version   -> Java version: 1.8.0_482
```

## Points non bloquants

| Element | Etat | Decision |
| --- | --- | --- |
| Gradle global | absent | Pas urgent. Preferer `gradlew` si un projet le fournit. |
| Docker | absent | Pas necessaire maintenant. A revoir si Qdrant via conteneur devient utile. |
| Qdrant | absent | Pas necessaire au demarrage. A installer quand le RAG vectoriel sera pret. |
| Tree-sitter CLI | absent | Pas necessaire globalement. On utilisera probablement les crates Rust. |
| llama.cpp | absent | Pas urgent. A tester apres Ollama/LM Studio. |
| HIP / ROCm Windows | absent | Non bloquant. Sur Windows 10, Vulkan/Ollama/LM Studio sont plus simples pour commencer. |
| RustRover | absent | Non necessaire. VS Code + IntelliJ suffisent. |

## Recommandation actuelle

Pour la suite immediate :

1. Utiliser Ollama ou LM Studio pour tester Qwen2.5-Coder 14B GGUF Q4_K_M.
2. Garder llama.cpp pour une phase d'optimisation ulterieure.
3. Ne pas installer Docker/Qdrant avant d'avoir un prototype RAG minimal.
4. Ne pas cloner de gros depots avant d'avoir defini les objectifs d'analyse.

