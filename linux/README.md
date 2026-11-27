# linux/

The launcher and the measuring scripts as they were used, on one machine. They
are here to be read and adapted, not to be run as they are: every
machine-specific path is a token (see `docs/notes/README.md`). Replace the
tokens with your own directories first, for example:

```sh
grep -rlE '@(PORTING|PROFILE|SCRATCH)@' linux | xargs sed -i \
  -e "s#@PORTING@#$PWD/porting#g" \
  -e "s#@PROFILE@#$PWD/profile#g" \
  -e "s#@SCRATCH@#$PWD/scratch#g"
```

| Script | What |
|---|---|
| `launch-ports.sh` | The launcher: every switch of a run, with the reason beside it |
| `run-victory-proton.sh` | Starts the layer under Proton with vkd3d-proton |
| `run-ports.py`, `isolated-supervisor.py` | One supervised run in a memory-limited scope |
| `play.sh` | A play session: `stable`, `today`, `fast`, `quality`, `rt`, `steady` |
| `leg.sh` | One labelled, scripted run: captures, a blink measure, a short recording, a summary |
| `leg-index.py`, `leg-serve.py` | A page of all legs, and a small server for it on the local network |
| `flicker.py` | How much the picture changes between quick captures while nothing moves |
| `capture.py` | Saves the game's window |
| `precompile.sh`, `warm-rt-seed.sh`, `seed-other.sh` | Shaders translated and pipelines made ahead, into a seed |
| `route-merge.py` | A recording of `VICTORY_KEYS_RECORD` made playable |

`tools/userptr-thp/` is the preloaded helper that collapses renderer port 1's
heap into huge pages (see the notes, section 165).
