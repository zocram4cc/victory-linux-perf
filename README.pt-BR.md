[English](README.md) | [Русский](README.ru.md) | [简体中文](README.zh.md) | [Español](README.es.md) | Português (BR)

# RAZDA4A XEN

Camada de execução Direct3D 12 para Windows voltada para **16 GB de RAM**, **CPUs de 6 núcleos**, **GPUs de 8–16 GB VRAM** e **Windows 10**.

Não inclui arquivos de jogo, assets ou executáveis. Você deve fornecer sua própria cópia de *O Grande Simulador de Empréstimo de Carros do Estúdio Estelar*.

## Diferenças em relação ao VIctory original

O VIctory original é desenvolvido e perfilado principalmente em 64 GB de RAM, GPUs de 24 GB e Windows 11 (WDDM 3.0+ com `D3D12_HEAP_TYPE_GPU_UPLOAD`). Em PCs com 16 GB de RAM e Windows 10 isso gera crashes `0x8007000E` (`E_OUTOFMEMORY`), quedas pela metade no VSync, ghosting de HUD no Frame Generation e gargalo na thread de envio da CPU.

Medido em **Ryzen 5 5600 (6C/12T), Radeon RX 9060 XT 16 GB, 16 GB DDR4, Windows 10 (19045)** — 1080p perfil Series S, RT desligado, execuções contínuas de 180 s, 0 erros:

| Métrica | Base (configuração upstream) | RAZDA4A XEN |
|---|---|---|
| Private Commit (estável) | 21.8 – 30+ GB (`0x8007000E` OOM) | **19.3 GB** (6.2 GB RAM WS, 5.5 GB livres) |
| VRAM dedicada | 9.92 GB (`5.06 GB` pool de texturas) | **12.11 GB** (`5.12 GB` pool de texturas; até `13.18 GB` / `6.40 GB` pool; auto 8–24 GB DXGI) |
| GPU compartilhada (`NON_LOCAL`) | 6.68 GB (5.58 GB Series S) | **2.18 GB** (`-61%` frente à base Series S; `2.01 GB` com 91% keep) |
| Tráfego copy-back | 27.0 MB / quadro | **0.5 MB / quadro** |
| CPU `preparing` | 45.8 ms | **32.2 – 35.4 ms** |
| Quadro real (mín / med) | 68.6 / 73.1 ms | **64.3 / 66.2 – 69.6 ms** |
| FPS nativo e estabilidade (sem FG, mundo aberto estável) | 14.08 FPS (média 71.03 ms, p99 152.1 ms / 1% low 6.6 FPS) | **16.6 – 19.5 FPS** (p50 **60.0 ms**, `victory-submit` **34.4 ms** vs 59.7 ms, `VirtualQuery` **0.00 ms**, `BufferSweep` **0.82 ms**, `0` OOM / `0` emergency reclaim) |
| Passe `heap buffers` (máx. / picos >50 ms) | 821.1 ms (8 picos >50 ms) | **33.8 ms (0 picos >50 ms; estável 8.6–10.0 ms)** |
| Esperas `settle` e `take()` (estável sem FG) | 0.2–2.6 ms settle / 0.75–1.07 ms `take()` | **0.0 ms settle / 0.31–0.47 ms `take()`** |
| FPS exibido (FSR 3.1.6 FG) | 25–27 FPS (quedas VSync para 15–20) | **29.2 – 31.0 FPS** (sem quedas VSync) |
| HUD e legendas no FG | Ghosting e distorção | Limpo (`HUDLessColor` RGBA8 de 3 slots) |
| Windows 10 D3D12 | Configuração manual do Agility SDK | `D3D12Core.dll` incluso (Agility SDK 620) |

## Alterações técnicas

- **Memória e OOM (`0x8007000E`):** pool global limitado de prelúdios `(64, 16)`; reciclagem imediata de listas e alocadores (`recycle_landed`) com liberação diferida (`bury_lag`); `emergency_reclaim()` proativo abaixo de 2.5 GiB de commit livre ou >=88% de Shared GPU (`NON_LOCAL`), com limpeza LRU agressiva de texturas/staging/pools e limite dinâmico de concorrência em `thin` (1–2 workers sob pressão); recuperação, retry e reset/retirada segura do alocador em `E_OUTOFMEMORY` em `HostList::close()`, `make_list()` e `record_prelude()`; `MEM_RESET` em páginas físicas liberadas não mapeadas; correções upstream `#2143`, `#2234`, `#2462`, `#2844` (`CpuViews` RAII) e `#3540` (alinhamento de 16 bytes em views raw); páginas físicas opcionais em NVMe (`FILE_FLAG_DELETE_ON_CLOSE` em `%LOCALAPPDATA%\victory\physical`, >11 GB livres no carregamento). Verificado em execução nativa de 180 s sem FG com paginação padrão (`VICTORY_FILE_PHYSICAL=0`): 0 erros, 0 OOM, `min_free_commit 3.47 GB`, `5.10–5.57 GB` livres em regime estável.
- **Apresentação e Frame Generation:** captura `HUDLessColor` RGBA8 de 3 slots antes de compor UI/legendas/HUD; swapchain triple-buffer com `DXGI_PRESENT_ALLOW_TEARING`; AMD FSR 4.1.2 ML nativo (fallback FSR 3.1 em placas anteriores).
- **Envio CPU e copy-back:** escritas write-combined mantidas em VRAM (`27 -> 0.5 MB/quadro`); skip AVX2 `vptest` de 128 B e `merge_piece` vetorizado; pool lock-free work-stealing em `land`; shards write-watch alinhados em 64 B; varredura fundida `ask_once`/`stale_under_flights`; `mirror` lock-free; `OnceLock` em assinaturas de comando; `Few<Barrier, 16>` na pilha; `PLACED_TEXTURES` em 64 shards; `unfold` sem lock e `wake()` lock-free; zeragem de `upload::ARENAS` fora do mutex; prioridade elevada em 6 núcleos; listas pré-planejadas; hash antes de detile; reuso de buffers de upload.
- **Auto-VRAM dinâmico DXGI e redução de Shared GPU:** escalonamento automático 8 / 12 / 16 / 24+ GB via `DedicatedVideoMemory` e `DXGI_QUERY_VIDEO_MEMORY_INFO` ao vivo (`VICTORY_VRAM_MODE=auto|8gb|12gb|16gb|24gb|legacy`, suporte a `"auto"` e overrides manuais, detecção UMA e redução gradual por margem de Commit). Em GPUs de 16 GB adapta o limite à RAM: `<=20 GB` RAM usa `84%` keep / `90%` room (`12.11 GB` VRAM, `2.18 GB` Shared GPU, sem paginação de working set, `+55%` em 1% low FPS) e `>=32 GB` RAM usa `91%` keep / `95%` room (`~13.2–14.2 GB` VRAM). Janelas VRAM diretas sem staging (`CardCpu::Direct`), cópias CPU compactas (`CardCpu::Cpu`), fila `DIRTY_CARDED` O(dirty), descarte incremental de janelas com histerese (máx. 8/s), scratch compartilhado para depth-stencil (upstream `#2162`), correção do limiar `host_pressure` em 16 GB RAM (`send_allocs` `117/15s -> 0/15s`), autodetecção NVMe (`VICTORY_FILE_PHYSICAL=auto`) e contabilização apenas do título em `XMemGetWorkingSetStatistics`.
- **Frame pacing e eliminação de picos (nativo sem FG):** `sweep()` assíncrono incremental (`victory-sweep`, `VICTORY_SWEEP_ASYNC=1`) removendo o dreno síncrono `settle(WHY_SYNC)` a cada 1.5 s; coletor em segundo plano `bury()` com throttling (`victory-bury`, `VICTORY_BURY_ASYNC=1`); esperas `WHY_FENCE`/`WHY_VALUE` via `Line.seq` sem travar `host.submitted`; omissão de `sync_heaps` redundante em `execute_own` (`0.0` rodadas sem marca); varredura de 64 bits `AtomicU64` em `port_descriptors::take()` (`2–3×` mais rápida); `frame::wait` preciso sem `Sleep(0)`; telemetria por quadro `present pacing` (`avg / p50 / p95 / p99 / 1% low / max / >50 ms / >100 ms`, `VICTORY_FRAME_TIMES_FILE`) e `heap buffers` plano (`VICTORY_TEXTURE_STAGE_MS=4`, `VICTORY_TEXTURE_CATCHUP=0`, `0` picos >50 ms).
- **Fase 2 — otimização de envio CPU e descritores (`feat/phase2-30fps`):** cache `VirtualQuery` `MappedCache` com invalidação por faixa (`12.56 ms -> 0.00 ms`, `own_cs` `15.1 -> 2.5 ms`); tabela atômica `ENTRY_GENS` + cache TLS de 8192 entradas (`100.0%` de acerto); caminho rápido inline-CBV em `draw_bind::tables` + lotes `RETIRE_TLS`; `BufferSweep` incremental (`HBP_SPANS` `10.94 -> 0.51 ms`); pools ampliados `PRELUDES_MOST` `(512, 128)`, `SPARE_BATCH` `(128/256)`, `VICTORY_LIST_IDLE_MOST=1024`; fila prioritária compute PSO + 4 workers compute/screen (`p99.9` `-71%`, `1% low` `2×`); correção dos limiares `residency` em 16 GB RAM (`0` pressões falsas, `unmap_null` `-97%`, `VICTORY_KEEP_SHOWN=2`); bitmap `gpu_written_pages` de 64 KiB; HUD no canto superior direito (`VICTORY_PANEL_POS=top-right`) e caixa de seleção para ativar/desativar Frame Generation no launcher.
- **Correções e controles:** orçamento de espera compute (`5000 ms`) corrigindo rostos ausentes; skip do shader problemático em RDNA4 (`240cb516c1af0bc3`); filtro de probes roxos; flips assíncronos seguros; retry de detile; teclado e mouse PC (`F2`/`~` câmera, `F3` overlay FPS no canto superior direito).

## Limites conhecidos (em andamento na `dev`)

- 30 FPS nativos (`<= 33.3 ms`) seguem em otimização na `dev`: `victory-submit` já caiu para `34.4 ms/quadro`; o próximo foco é a gravação paralela (`victory-list-0..3`) e cópias de CBV.
- Picos de Commit na transição prólogo -> mundo aberto em arquivos de paginação apertados são mitigados na `dev` via `emergency_reclaim()` e autodetecção NVMe (`VICTORY_FILE_PHYSICAL=auto`).

## Início rápido

1. Baixe a `main` (**Code -> Download ZIP** ou `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) e extraia (ex.: `D:\RAZDA4A-XEN\`). As DLLs necessárias já estão incluídas.
2. Edite **`config.bat`** apontando para sua pasta e **`seu_arquivo_executavel.exe`**:
   ```bat
   set "GAME_DIR=D:\Caminho\Da\Sua\Pasta"
   set "GAME_EXE=%GAME_DIR%\seu_arquivo_executavel.exe"
   ```
3. Execute **`RAZDA4A XEN.bat`** ou escolha um preset em `presets\`:

| Arquivo | Perfil |
|---|---|
| `RAZDA4A XEN.bat` | Equilibrado (padrão): 29–31 FG FPS, ~8.2 GB VRAM, 6.2 GB RAM, HUDLess FG + Tearing |
| `presets\1_Low_Commit_NVMe_16GB.bat` | Paginação apertada / navegador aberto: páginas físicas em NVMe, ~8.0 GB VRAM, >11 GB livres no carregamento |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Equilibrado + buffers constantes in-place (experimental) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Renderização nativa sem Frame Gen, menor latência |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB VRAM / modo seguro: limites rígidos, VSync ligado, varredura completa de descritores |

- **`F2` / `~`** — alternar mouse (`Esc` libera cursor)
- **`F3`** — overlay FPS
- **`warm_shaders.bat`** — rode uma vez após fechar a primeira sessão para pré-compilar shaders
- **`backup_saves.bat`** — backup/restauração de `data\`
- **`pagefile_16gb_ram.ps1`** — script opcional de administrador (paginação SSD 16–32 GB)

## Compilar do código-fonte e verificação

Se preferir não usar binários sem assinatura, compile diretamente da branch `dev`:

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=znver3"
cargo build --release --locked -p victory
```

SHA-256 de `RAZDA4A_XEN.exe`: `7b2d7b7a5059ec41c423c60c34d0fd0025dc141239fe81a645fbfe4f2307376b`

## Branches e licença

- `main`: binário compilado, DLLs, presets e documentação.
- `dev`: código-fonte completo em Rust, testes, CI e `AGENTS.md`.

Derivado do VIctory sob **GPL-3.0-or-later** ([LICENSE](LICENSE)). Código-fonte completo em [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
