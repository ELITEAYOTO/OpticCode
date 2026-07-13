# Java index mini corpus

Corpus read-only pour CODE-001B1. Il couvre les imports explicites, wildcard et
statiques, les types du meme package, `java.lang`, les classes imbriquees, les
overloads, un enum, un nom pleinement qualifie, un type ambigu et un type non
resolu.

CONTEXT-001 reutilise ce corpus avec un `pom.xml` Java 8 et un `plugin.yml`
minimal pour verifier que les fichiers de configuration ne sont ajoutes que
lorsque la demande les vise.
