[English](../en/running.md) | [Русский](../ru/running.md) | [简体中文](../zh/running.md) | [Español](../es/running.md)

# Como executar

[Voltar ao README](../../README.pt-BR.md)

Inicie pelo launcher fornecido: **`RAZDA4A_XEN.exe`** ou **`RAZDA4A XEN.bat`**. Não execute o executável do jogo diretamente. O runtime só é aplicado quando o launcher inicia o jogo.

Nenhum arquivo, asset ou executável do jogo está incluído. Você deve fornecer sua própria cópia de *O Grande Simulador de Empréstimo de Carros do Estúdio Estrelado*.

## Início Rápido (Windows)

1. Baixe `main` (**Code -> Download ZIP** ou `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) e extraia. Inclui `D3D12\D3D12Core.dll` e `fidelityfx\*.dll`.
2. Inicie com **`RAZDA4A_XEN.exe`** ou **`RAZDA4A XEN.bat`**. Não dê duplo clique no `.exe` do jogo.
   - **`RAZDA4A XEN.bat`** abre a janela do launcher: clique em **Browse...** para escolher **`your_executable.exe`** (salvo em UTF-8 no `config.bat`), escolha o perfil e Frame Generation e clique em **Launch**.
   - Launchers externos (`VILauncher`, etc.): use **`RAZDA4A XEN.bat --no-gui`**.

| Arquivo | Uso |
|---|---|
| `RAZDA4A XEN.bat` | Launcher principal (seletor `.exe`, FG ON/OFF, auto-Safe-Mode 8 GB, Warm Shaders e Export Logs) |
| `RAZDA4A_XEN.exe` | Runtime que o launcher inicia. Não pule essa etapa para rodar o `.exe` do jogo diretamente. |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB RAM / pagefile pequeno: páginas físicas em NVMe |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Equilibrado + CBV in-place experimental (`VICTORY_PORT_CBV=1`) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Nativo sem Frame Gen, menor latência |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | Safe Mode 8–12 GB VRAM (`65%` / `2048 MB` targets / `768 MB` buffers, VSync ON) |
| `warm_shaders.bat` / `collect_logs.bat` | Pré-compilar PSOs / gravar um relatório sanitizado com `logs\last_run.log` neste PC. |

Controles: **`F2` / `~`** alterna câmera no mouse (`Esc` libera cursor), **`F3`** alterna HUD FPS.

## Início Rápido (Linux — Proton / Wine / VKD3D-Proton)

1. Instale **Proton Experimental / GE-Proton** (`umu-run`, Steam ou Lutris) com **Mesa RADV** em GPU AMD RDNA2+.
2. Execute `launch_linux_proton.sh` (define automaticamente `WINEDLLOVERRIDES="d3d12,d3d12core=n"` para evitar que a `D3D12Core.dll` do Windows 10 bloqueie o `vkd3d-proton` e cause 1 FPS / tela 3D vazia). O script inicia o `RAZDA4A_XEN.exe`. Não execute o executável do jogo diretamente.
   ```bash
   chmod +x launch_linux_proton.sh
   GAME_DIR="/path/to/unpacked" \
   GAME_EXE="/path/to/unpacked/your_executable.exe" \
   RUNNER="umu-run" \
   XEN_PROFILE="safe" \
   XEN_FRAMEGEN="0" \
   ./launch_linux_proton.sh
   ```

## Veja também

- [Desempenho](performance.md)
- [Hardware](hardware.md)
- [O que a Wave 11 traz](technical-changes.md)
- [Limites conhecidos](known-limits.md)
- [Segurança, checksums e compilação](security-and-build.md)
- [Branches e licença](project.md)
