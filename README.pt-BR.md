<p align="center">
  <span style="font-size:64px">🦋</span>
</p>

<h1 align="center">RAZDA4A XEN</h1>

<p align="center">
  Runtime Direct3D 12 para AMD RDNA.<br>
  <b>Wave 11</b> · <b>~31 native / 62-63 FG</b>
</p>

<p align="center">
  <img alt="Versão: Wave 11" src="https://img.shields.io/badge/release-Wave%2011-2ea44f">
  <a href="https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip"><img alt="Baixar o ZIP da main" src="https://img.shields.io/badge/download-main%20ZIP-0969da"></a>
  <a href="LICENSE"><img alt="Licença GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue"></a>
  <img alt="Windows 10/11 e Linux com Proton" src="https://img.shields.io/badge/platform-Windows%2010%2F11%20%7C%20Linux%20Proton-informational">
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.zh.md">简体中文</a> ·
  <a href="README.es.md">Español</a> ·
  Português (BR)
</p>

## Resultados

| | |
|---|---|
| **Versão** | Wave 11 (`main`) |
| **Máquina** | Radeon RX 9060 XT + Ryzen 7 7800X3D |
| **FPS** | **~31 native / 62-63 FG** |

`62-63 FG` é frame generation. O nativo é ~31.

Placas de 8 GB usam Safe Mode. Uma RX 7600 mediu **12,7 nativos / 24,86 FG**. Tabelas: [Desempenho](docs/pt-BR/performance.md).

## Início rápido

Este repositório não traz arquivos do jogo. Use a sua cópia.

1. Baixe [main.zip](https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip) ou `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git` e extraia.
2. Inicie **`RAZDA4A_XEN.exe`** ou **`RAZDA4A XEN.bat`**.
3. No launcher, escolha o `.exe` do jogo, um perfil e frame generation, e clique em **Launch**.

Não execute o `.exe` do jogo diretamente. O runtime só entra quando o launcher inicia o jogo.

`F2` ou `~` alterna a câmera no mouse (`Esc` solta o cursor). `F3` alterna o FPS.

No Linux, `launch_linux_proton.sh`. Presets e Linux: [Como executar](docs/pt-BR/running.md).

## Requisitos

| | Wave 11 |
|---|---|
| **GPU** | AMD RDNA2 / RDNA3 / RDNA4 (RX 6000 / 7000 / 9000), DX12 Ultimate e mesh shaders |
| **CPU** | x86-64-v3 (AMD Zen 2–5, Intel Haswell até Arrow Lake) |
| **Memória** | 16 GB de RAM em SSD. Commit apertado: pagefile de 24–32 GB (`pagefile_16gb_ram.ps1`). 8–12 GB de VRAM: Safe Mode |
| **SO** | Windows 10 / 11, ou Linux com Proton / VKD3D-Proton |
| **Não é esta build** | NVIDIA, Intel Arc, RDNA1, GTX 10/16, placas de 4 GB e HDD |

A versão NVIDIA está a caminho. A AMD saiu primeiro. Até lá: [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA).

## Presets

| Arquivo | Uso |
|---|---|
| `RAZDA4A XEN.bat` | Janela do launcher: `.exe` do jogo, frame generation, Safe Mode em 8 GB |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB de RAM, páginas em NVMe |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Equilibrado, mais buffers de constantes in-place |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Só nativo, frame generation desligado |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB de VRAM |

## FAQ

<details>
<summary>O jogo vem incluído?</summary>

Não. Traga a sua cópia. Um GDK vazado não é necessário. Este repositório não aponta para downloads do jogo.
</details>

<details>
<summary>NVIDIA ou Intel Arc?</summary>

Wave 11 é AMD RDNA. A versão NVIDIA está a caminho. A AMD saiu primeiro. O fork da comunidade está acima. Intel Arc não é suportado. Um adaptador que não seja AMD pode ficar preto ou fechar.
</details>

<details>
<summary>8 GB de VRAM, ou menos?</summary>

8–12 GB: Safe Mode (preset 4, ou o launcher escolhe isso em 8 GB). RX 7600: 12,7 nativos / 24,86 FG. 6 GB ou menos não é uma configuração suportada. RDNA1 e GTX 10/16 não têm os mesh shaders desta build.
</details>

<details>
<summary>Ray tracing?</summary>

Desligado na Wave 11. O construtor de estruturas de ray tracing não roda.
</details>

<details>
<summary>HD mecânico, ou 16 GB de RAM?</summary>

HDD não é suportado. Use um SSD NVMe ou SATA. Com 16 GB de RAM, `pagefile_16gb_ram.ps1` ou o preset 1 cobrem um pagefile apertado.
</details>

## Se algo falhar

- A primeira execução compila pipelines (cerca de 1–3 minutos de engasgos). Feche, rode `warm_shaders.bat` uma vez e abra de novo pelo launcher.
- O Windows Defender pode marcar o ZIP sem assinatura como `Trojan:Script/Wacatac.H!ml`. Compare `RAZDA4A_XEN.exe` com [SHA256SUMS.txt](SHA256SUMS.txt): `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622`.
- `collect_logs.bat`, ou **Export Logs** no launcher, grava `logs\bug_report_*.txt` neste PC.
- Linux perto de 1 FPS ou imagem vazia: use `launch_linux_proton.sh`, que impede a `D3D12Core.dll` de substituir o vkd3d-proton.

[Como executar](docs/pt-BR/running.md) · [Segurança e compilação](docs/pt-BR/security-and-build.md)

## Sobre o projeto

RAZDA4A XEN é uma camada Direct3D 12 de código aberto para 16 GB de RAM, CPUs de 6 núcleos e 8–16 GB de VRAM. A Wave 11 na `main` é o launcher, os presets, as DLLs e estas páginas. O código Rust está na [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).

## Créditos

- [VIctory](https://github.com/Marvisak/VIctory), de Marvisak, upstream, sob GPL-3.0-or-later ([LICENSE](LICENSE)).
- `nathanbzrr` — Safe Mode de 8 GB, publicado na `main`.
- [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA) — fork NVIDIA da comunidade.

## Documentação

- [Desempenho](docs/pt-BR/performance.md)
- [Como executar](docs/pt-BR/running.md)
- [Hardware](docs/pt-BR/hardware.md)
- [O que a Wave 11 traz](docs/pt-BR/technical-changes.md)
- [Limites conhecidos](docs/pt-BR/known-limits.md)
- [Segurança, checksums e compilação](docs/pt-BR/security-and-build.md)
- [Branches e licença](docs/pt-BR/project.md)
