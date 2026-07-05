# OpticCode - Initialisation Git

Derniere mise a jour : 2026-07-06

## Etat actuel

Le dossier `C:\Users\timot\Desktop\OpticCode\.git` existe, mais il etait vide et contenait une regle Windows `DENY` qui empeche Codex d'y ecrire.

Git ne peut donc pas encore initialiser le depot depuis l'environnement sandbox.

## Commandes a lancer toi-meme

Dans PowerShell, lance ces commandes depuis ton utilisateur Windows normal :

```powershell
cd C:\Users\timot\Desktop\OpticCode
icacls .git /remove:d *S-1-5-21-239871269-3359610440-1275906532-2305720531
git init
git status
```

Si la commande `icacls` refuse encore, le plus simple est de supprimer uniquement le dossier `.git` vide depuis l'Explorateur Windows, puis de lancer :

```powershell
cd C:\Users\timot\Desktop\OpticCode
git init
git status
```

Ne supprime pas les autres dossiers du projet.

## Etat attendu

`git status` doit afficher les fichiers non suivis, dont :

- `README.md`
- `.gitignore`
- `docs/`
- `crates/`
- `data/`
- `skills/`
- `models/`
- `benchmarks/`
- `scripts/`

