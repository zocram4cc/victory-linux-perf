# Notes

| File | What |
|---|---|
| `NOTES.md` | The running log of the work, sections 5 to 172: what was tried, the run it was measured in, the number. Read the last sections first. |
| `AGENTS-workspace.md` | The rules and layout of the workspace the log was written in. Its paths are that workspace's, not this repository's. |
| `wave64-code-audit.md`, `wave64-summary.md` | How the console's wave sizes are translated, with file and line references. |

## Paths are tokens

The notes and the scripts in `linux/` were written on one machine. Every
machine-specific path is replaced by a token:

| Token | Meaning |
|---|---|
| `@PORTING@` | The workspace directory |
| `@WORKSPACE@` | Its parent; the working copy of the title lives beside the workspace |
| `@DUMP@` | The original copy of the title (never modified) |
| `@PROFILE@` | Where runs, warm seeds and measurements are kept |
| `@SCRATCH@` | Scratch build and run directory (keep it on disk, not in a RAM disk) |
| `@WORKTREES@` | Where extra git worktrees were checked out |
| `@AGENT_WORK@` | Parent of `@PROFILE@` |
| `@HOME@` | Home directory |
| `@BACKUP_HOST@`, `@BACKUP_DIR@`, `@PRIVATE_REMOTE@` | A private host, its directory, a private git remote |

Run names in the log (`f1-floor`, `p1-port1-off`, ...) are folders of captures
and logs that are not part of this repository: captures of the title are never
published.
