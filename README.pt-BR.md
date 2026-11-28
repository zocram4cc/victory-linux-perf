[English](README.md) | [Русский](README.ru.md) | [简体中文](README.zh.md) | [Español](README.es.md) | Português (BR)

# RAZDA4A XEN

Camada de execução Direct3D 12 para Windows voltada para **GPUs AMD RDNA2 / RDNA3 / RDNA4**, **16 GB de RAM**, **CPUs de 6 núcleos**, **8–16 GB de VRAM** e **Windows 10 / 11** (além de Linux via Proton / VKD3D-Proton).

Nenhum arquivo, asset ou executável do jogo está incluído. Você deve fornecer sua própria cópia de *O Grande Simulador de Empréstimo de Carros do Estúdio Estrelado*.

## Desempenho Medido (FPS Nativos vs. Frame Generation)

Todos os números separam **FPS nativos (Frame Gen OFF)** de **FPS exibidos (FSR 3.1.6 Frame Gen 2x)** com cena e hardware exatos.

### Base 16 GB VRAM / 16 GB RAM (Mundo Aberto, 1080p Series S, RT OFF)

Medido em **Ryzen 5 5600 (6C/12T), Radeon RX 9060 XT 16 GB, 16 GB DDR4, Windows 10 (19045)** — execuções de 180 s em mundo aberto + estabilidade Wave 11 de 22 minutos (18.393 quadros), 0 erros, 0 OOM:

| Métrica | Base (config upstream) | RAZDA4A XEN (Wave 7 / Wave 11) |
|---|---|---|
| **FPS nativos (FG OFF, Mundo Aberto)** | 14,08 média (71,03 ms, p99 152,1 ms / 1% low 6,6 FPS) | **18,19 p50** (54,97 ms), **16,17** média, 15,2–23,4 estável, p99.9 **478,6 ms** |
| **FPS exibidos (FSR 3.1.6 FG 2x)** | 25–27 FPS (quedas para 15–20 com VSync) | **29,2 – 31,0 FPS** estáveis (anel RGBA8 `HUDLessColor`, sem quedas VSync) |
| Private Commit (estável) | 21,8 – 30+ GB (`0x8007000E` OOM) | **19,3 GB** (6,2 GB RAM WS, 5,5 GB commit livre) |
| VRAM dedicada | 9,92 GB (`5,06 GB` pool de texturas) | **12,11 GB** (auto 8–24 GB DXGI; `0` recusas VRAM em 22 min no Wave 11) |
| GPU compartilhada (`NON_LOCAL`) | 6,68 GB (5,58 GB Series S) | **2,18 GB** (`-61%` vs Series S) |
| Tráfego copy-back | 27,0 MB / quadro | **0,5 MB / quadro** |
| CPU `preparing` / `heap buffers` | 45,8 ms / 821,1 ms máx. | **32,2 – 35,4 ms** / **33,8 ms máx.** (0 picos >50 ms) |
| Views raw (`LIST_VIEWS`, Wave 11) | 372,1–575,8 null/quadro (árvores/objetos sumindo) | **0,0 null/quadro** (28.212/quadro; árvores, folhagem, cercas e postes corrigidos) |
| Windows 10 D3D12 | Configuração manual Agility SDK | `D3D12Core.dll` incluso (Agility SDK 620) |

### Verificação 8 GB VRAM — Safe Mode (PR #1 por `nathanbzrr`)

Testado em **Ryzen 5 5500X3D, Radeon RX 7600 8 GB, 32 GB DDR4, Windows 11 (26340)** — 1080p Series S, Safe Mode (`65%` texturas / `2048 MB` targets / `768 MB` buffers), FSR FG 2x, 180 s:

| Métrica | Antes do ajuste 8 GB | Após ajuste ao orçamento real |
|---|---:|---:|
| **FPS nativos (sem FG)** | 3,67 | **12,7** (média 79,01 ms, p50 68,90 ms, p99 **242,26 ms** vs 1047,30 ms) |
| **FPS exibidos (FSR FG 2x)** | 7,32 | **24,86** (1803 gerados / 1803 renderizados) |
| VRAM pico / orçamento real | 6548 MB | **6169 MB** / 5934–5981 MB (`0` erros, `0` OOM, `0` device removal) |

## Mudanças Técnicas

- **Wave 11 (folhagem/objetos e binário portátil):** corrige `372–575` views raw `NULL` por quadro em `LIST_VIEWS` (acaba com árvores, folhagem, cercas, postes e placas sumindo); suporte `SW_64KB_D`; ajuste de residência 16 GB (`0` recusas VRAM em 22 min); binário portátil **`x86-64-v3`** (AVX2/BMI2/FMA em AMD Zen 2–5 e Intel Haswell–Arrow Lake).
- **Wave 6–7 (texturas, descritores e estabilidade):** cache TLS de 8192 slots (`100%` acertos), fila de texturas intercalada (fila em cutscenes `3936 -> 0`), prioridade normal de CPU e log em arquivo `VICTORY_LOG_FILE`.
- **Memória e OOM (`0x8007000E`):** `recycle_landed`, `emergency_reclaim()` proativo, recuperação `E_OUTOFMEMORY`, Auto-VRAM DXGI (`8/12/16/24 GB`) e páginas físicas NVMe (`VICTORY_FILE_PHYSICAL=auto`).
- **Apresentação e controles:** `HUDLessColor` RGBA8 para FSR 3.1.6 FG, tearing swapchain, FSR 4.1.2 ML nativo, espera compute `5000 ms` (rostos), escudo RDNA4 (`240cb516c1af0bc3`), teclado/mouse PC (`F2`/`~` câmera, `F3` HUD FPS).

## Limites Conhecidos (Em Desenvolvimento na `dev`)

- Áreas urbanas densas ainda podem cair para `5–10` FPS nativos com travamentos longos em sessões extensas (**Wave 12** em andamento na `dev`).
- Ray Tracing continua desativado enquanto o render base e os 30 FPS nativos são estabilizados.
- A primeira execução compila shaders PSO (1–3 min com engasgos); execute `warm_shaders.bat` depois de fechar para suavizar as próximas sessões.

## Compatibilidade de Hardware e FAQ

| Pergunta | Resposta |
|---|---|
| **GPUs AMD suportadas?** | **AMD RDNA2 / RDNA3 / RDNA4** (RX 6000 / 7000 / 9000; exige DX12 Ultimate + Mesh Shaders). |
| **NVIDIA / Intel Arc?** | **Não suportado neste fork.** Placas RTX apresentam tela preta/crashes; use forks NVIDIA da comunidade como [`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA). |
| **CPU Intel + GPU AMD?** | **Sim.** A partir do Wave 11 o binário usa `-C target-cpu=x86-64-v3`. |
| **8 GB / 6 GB / <=4 GB VRAM?** | **8 GB:** funciona com **Safe Mode** (12,7 nativos / 24,86 FG na RX 7600; testado na RX 6600). **6 GB:** experimental/pesado (Safe Mode + FG OFF). **<=4 GB, RDNA1 (RX 5500/5700), GTX 10/16:** não suportado (sem Mesh Shaders ou pouca VRAM). |
| **16 GB RAM / HDD?** | **16 GB RAM:** suportado em **SSD NVMe/SATA** com pagefile de 24–32 GB (`pagefile_16gb_ram.ps1`) ou preset `Low-Commit NVMe`. **HDD:** não suportado. |
| **Precisa do GDK vazado?** | **Não.** Runtime open-source independente (`GPL-3.0-or-later`). |
| **Onde enviar bugs e logs?** | Execute **`collect_logs.bat`** (ou **Export Logs** no launcher) para gerar `logs\bug_report_*.txt` sem caminhos pessoais e anexe em [GitHub Issues](https://github.com/crossducking/RAZDA4A-XEN/issues). |

## Início Rápido (Windows)

1. Baixe `main` (**Code -> Download ZIP** ou `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) e extraia. Inclui `D3D12\D3D12Core.dll` e `fidelityfx\*.dll`.
2. Execute **`RAZDA4A XEN.bat`**, clique em **Browse...** para escolher **`your_executable.exe`** (salvo em UTF-8 no `config.bat`), escolha o perfil e Frame Generation e clique em **Launch**.
   - Launchers externos (`VILauncher`, etc.): use **`RAZDA4A XEN.bat --no-gui`**.

| Arquivo | Uso |
|---|---|
| `RAZDA4A XEN.bat` | Launcher principal (seletor `.exe`, FG ON/OFF, auto-Safe-Mode 8 GB, Warm Shaders e Export Logs) |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB RAM / pagefile pequeno: páginas físicas em NVMe |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Equilibrado + CBV in-place experimental (`VICTORY_PORT_CBV=1`) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Nativo sem Frame Gen, menor latência |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | Safe Mode 8–12 GB VRAM (`65%` / `2048 MB` targets / `768 MB` buffers, VSync ON) |
| `warm_shaders.bat` / `collect_logs.bat` | Pré-compilar PSOs / exportar relatório sanitizado com `logs\last_run.log` |

Controles: **`F2` / `~`** alterna câmera no mouse (`Esc` libera cursor), **`F3`** alterna HUD FPS.

## Início Rápido (Linux — Proton / Wine / VKD3D-Proton)

1. Instale **Proton Experimental / GE-Proton** (`umu-run`, Steam ou Lutris) com **Mesa RADV** em GPU AMD RDNA2+.
2. Execute `launch_linux_proton.sh` (define automaticamente `WINEDLLOVERRIDES="d3d12,d3d12core=n"` para evitar que a `D3D12Core.dll` do Windows 10 bloqueie o `vkd3d-proton` e cause 1 FPS / tela 3D vazia):
   ```bash
   chmod +x launch_linux_proton.sh
   GAME_DIR="/path/to/unpacked" \
   GAME_EXE="/path/to/unpacked/your_executable.exe" \
   RUNNER="umu-run" \
   XEN_PROFILE="safe" \
   XEN_FRAMEGEN="0" \
   ./launch_linux_proton.sh
   ```

## Segurança, Falsos Positivos e Compilação pelo Código-Fonte

Brave/Edge e Windows Defender ML podem sinalizar ZIPs sem assinatura contendo `.exe` + `.bat` + `.ps1` como **`Trojan:Script/Wacatac.H!ml`**. Verifique o SHA-256 ou compile você mesmo a partir da `dev`:

```powershell
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe` (`x86-64-v3`) SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622` (todas as DLLs em [`SHA256SUMS.txt`](SHA256SUMS.txt)).

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## Branches e Licença

- `main`: binário pré-compilado, DLLs, presets, launcher, checksums e documentação.
- `dev`: código-fonte Rust completo, testes, `xtask policy`, CI e `AGENTS.md`.

Derivado de VIctory sob **GPL-3.0-or-later** ([LICENSE](LICENSE)). Código completo em [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
