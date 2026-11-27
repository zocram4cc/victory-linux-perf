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
| VRAM dedicada | 9.92 GB (`5.06 GB` pool de texturas) | **8.18 GB** (`3.65 GB` pool de texturas) |
| GPU compartilhada (`NON_LOCAL`) | 6.68 GB | **5.45 GB** |
| Tráfego copy-back | 27.0 MB / quadro | **0.5 MB / quadro** |
| CPU `preparing` | 45.8 ms | **32.2 – 35.4 ms** |
| Quadro real (mín / med) | 68.6 / 73.1 ms | **64.3 / 66.2 – 69.6 ms** |
| FPS nativo (sem FG) | ~13.7 FPS | **14.5 – 15.5 FPS** |
| FPS exibido (FSR 3.1.6 FG) | 25–27 FPS (quedas VSync para 15–20) | **29.2 – 31.0 FPS** (sem quedas VSync) |
| HUD e legendas no FG | Ghosting e distorção | Limpo (`HUDLessColor` RGBA8 de 3 slots) |
| Windows 10 D3D12 | Configuração manual do Agility SDK | `D3D12Core.dll` incluso (Agility SDK 620) |

## Alterações técnicas

- **Memória e OOM (`0x8007000E`):** pool global limitado de prelúdios `(64, 16)`; reciclagem imediata de listas e alocadores (`recycle_landed`) com liberação diferida (`bury_lag`); limpeza e retry em `E_OUTOFMEMORY` em `make_list()` e `record_prelude()`; controle `NON_LOCAL` (`92%`, histerese `-10%`); páginas físicas opcionais em NVMe (`FILE_FLAG_DELETE_ON_CLOSE` em `%LOCALAPPDATA%\victory\physical`, >11 GB livres no carregamento).
- **Apresentação e Frame Generation:** captura `HUDLessColor` RGBA8 de 3 slots antes de compor UI/legendas/HUD; swapchain triple-buffer com `DXGI_PRESENT_ALLOW_TEARING`; AMD FSR 4.1.2 ML nativo (fallback FSR 3.1 em placas anteriores).
- **Envio CPU e copy-back:** escritas write-combined mantidas em VRAM (`27 -> 0.5 MB/quadro`); skip AVX2 `vptest` de 128 B e `merge_piece` vetorizado; pool lock-free work-stealing em `land`; shards write-watch alinhados em 64 B; varredura fundida `ask_once`/`stale_under_flights`; `mirror` lock-free; `OnceLock` em assinaturas de comando; `Few<Barrier, 16>` na pilha; `PLACED_TEXTURES` em 64 shards; `unfold` sem lock e `wake()` lock-free; zeragem de `upload::ARENAS` fora do mutex; prioridade elevada em 6 núcleos; listas pré-planejadas; hash antes de detile; reuso de buffers de upload.
- **Correções e controles:** orçamento de espera compute (`5000 ms`) corrigindo rostos ausentes; skip do shader problemático em RDNA4 (`240cb516c1af0bc3`); filtro de probes roxos; flips assíncronos seguros; retry de detile; teclado e mouse PC (`F2`/`~` câmera, `F3` overlay FPS).

## Limites conhecidos (em andamento na `dev`)

- 30 FPS nativos (`<= 33.3 ms`) ainda são limitados pela thread `victory-submit` (`49–60 ms/quadro`; GPU em 52–58%).
- O perfil Series S usa ~8.2 GB VRAM em placas de 16 GB; orçamento dinâmico de VRAM em andamento na `dev`.
- A transição prólogo -> mundo aberto pode exceder o arquivo de paginação padrão em 16 GB RAM (`0x8007000E`); correção em código sem mexer na paginação em andamento na `dev`.

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

SHA-256 de `RAZDA4A_XEN.exe`: `e90b23f05f6d3caa488beaf3afc4cd0e94151b75a33606bfd75594a373e4f18b`

## Branches e licença

- `main`: binário compilado, DLLs, presets e documentação.
- `dev`: código-fonte completo em Rust, testes, CI e `AGENTS.md`.

Derivado do VIctory sob **GPL-3.0-or-later** ([LICENSE](LICENSE)). Código-fonte completo em [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
