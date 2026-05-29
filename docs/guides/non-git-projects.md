# Working with Non-Git Projects

Myceliums is designed for git repositories, but it also works without git. This guide covers how to use it with non-git directories, the tradeoffs involved, and tips for getting the best results.

## The `--no-git-check` Flag

By default, myceliums expects a `.git` directory in the project root. If it does not find one, it will exit with an error. To bypass this check, pass the `--no-git-check` flag:

```bash
myc analyze ./my-folder --no-git-check
```

This works with any myceliums command that targets a project path:

```bash
myc analyze ./my-folder --no-git-check
myc session ./my-folder --yes --no-git-check
myc search "query" --no-git-check
```

## Cache and Change Detection

With git, myceliums uses `git diff` to detect which files changed since the last analysis. This is fast and precise: only modified files get re-analyzed.

Without git, myceliums falls back to **file modification timestamps** (mtime). This approach has a few consequences:

- **Saves without content changes may trigger re-analysis.** If your editor writes the file on save (even when nothing changed), myceliums will see a new mtime and re-process the file.
- **Bulk operations like `touch` or `cp` will trigger re-analysis** of all affected files, even if the content is identical.
- **Clock skew matters.** If files are synced from another machine with a different clock, mtime comparisons may produce incorrect results.

## Watch Mode

You can use watch mode to monitor filesystem events and re-index in real time:

```bash
myc analyze ./my-folder --watch --no-git-check
```

This listens for file create, modify, and delete events and updates the graph incrementally. It works well for active development sessions where you want the knowledge graph to stay current as you edit.

## Recommendation: Initialize Git Anyway

Even for non-code projects (documentation folders, config repositories, data directories), running `git init` gives myceliums significantly better change detection and performance:

```bash
cd ./my-folder
git init
git add -A
git commit -m "initial commit"

# Now myceliums works without --no-git-check
myc analyze .
```

This one-time setup takes seconds and eliminates all the mtime-related issues described above. You do not need to push to a remote or maintain an active git workflow. The local `.git` directory is enough.

## Limitations Without Git

When running without git, the following features are unavailable or degraded:

| Feature | With git | Without git |
|---------|----------|-------------|
| Incremental analysis | Precise (git diff) | Approximate (mtime) |
| Commit tracking | Full history | Not available |
| Impact analysis via diff | Uses git diff | Not available |
| `get_review_context` | Shows changes since last commit | Limited to full file analysis |
| Change detection accuracy | Exact | May include false positives |

If you rely on impact analysis or review context, initializing a git repository is strongly recommended.
