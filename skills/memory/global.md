# OpticCode Global Memory

## Preferences utilisateur

- Repondre en francais.
- Avancer par petites etapes testees.
- Eviter les solutions lourdes quand une solution deterministe suffit.
- Mesurer avant d'optimiser.
- Ne pas modifier les fichiers sans patch preview ou confirmation explicite.

## Strategie projet

- Construire OpticCode autour du modele, sans modifier Qwen.
- Privilegier Rust pour le core, les tools, la memoire et l'indexation.
- Garder C++ pour llama.cpp ou des besoins bas niveau mesures.
- Preferer Q4_K_M tant que le workflow agent/RAG n'est pas stabilise.
