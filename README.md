# rsping

> **Monitor de estabilidade de rede em TUI**, inspirado no `PsPing` da Sysinternals, escrito em Rust.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`rsping` mede latência ICMP e TCP em tempo real e mostra tudo numa interface de terminal com quatro painéis: log de pings, estatísticas ao vivo, histograma e gráfico em braille. Ao final, imprime um relatório consolidado com avaliação de estabilidade.

![TUI do rsping](docs/screenshot.png)
<!-- Substitua pelo print real do programa -->

---

## Índice

1. [Destaques](#destaques)
2. [Requisitos](#requisitos)
3. [Instalação](#instalação)
4. [Manual de uso](#manual-de-uso)
   - [Alvos](#alvos)
   - [Opções da CLI](#opções-da-cli)
   - [Teclas durante a execução](#teclas-durante-a-execução)
   - [Receitas prontas](#receitas-prontas)
5. [Entendendo as métricas](#entendendo-as-métricas)
6. [O relatório final](#o-relatório-final)
7. [Arquitetura do projeto](#arquitetura-do-projeto)
8. [Dependências do Cargo](#dependências-do-cargo)
9. [Cross-compilation para Windows](#cross-compilation-para-windows)
10. [Solução de problemas](#solução-de-problemas)
11. [Contribuindo](#contribuindo)
12. [Licença](#licença)

---

## Destaques

- **ICMP e TCP ping** no mesmo binário — `host:porta` ativa o modo TCP automaticamente
- **Múltiplos alvos** com abas e navegação por `Tab`
- **Sem Administrador no Windows** para ICMP — usa a API `IcmpSendEcho` via `winping-rs`
- **TUI ao vivo** com gráfico em braille (resolução 2×4 por célula), histograma e log rolável
- **Métricas de estabilidade**: jitter RFC 3550, desvio padrão, percentis P50/P95/P99, spikes, outliers
- **Relatório final** com tabela comparativa, ranking e avaliação qualitativa
- **Export CSV** de todos os alvos com uma tecla
- **Sintaxe compatível com PsPing** (`-n`, `-i`, `-l`, `-t`, `-w`, `-ipv4`, `-ipv6`)

---

## Requisitos

| Sistema | Requisitos |
|---|---|
| **Linux** | `libcap` + `setcap` (uma vez) **ou** rodar como root |
| **Windows** | Windows 10+ (64 bits). Sem privilégios para ICMP/TCP |
| **macOS** | Nenhum privilégio extra |

**Rust** (para compilar do código): `rustc` 1.75+ e `cargo`. Se você usa `rustup`, está coberto.

---

## Instalação

### Compilar do código-fonte

```bash
git clone https://github.com/SEU_USUARIO/rsping.git
cd rsping
cargo build --release
./target/release/rsping --help
```

### Permissões no Linux (obrigatório uma vez)

O envio de pacotes ICMP exige `CAP_NET_RAW`. Há duas opções:

**Opção A** — conceder a capability ao binário (recomendado):

```bash
sudo setcap cap_net_raw+ep ./target/release/rsping
```

Depois disso, você pode rodar `./target/release/rsping` com seu usuário comum.

**Opção B** — rodar com `sudo`:

```bash
sudo ./target/release/rsping
```

> A Opção A é mais segura e não afeta o resto do sistema. Se você recompilar o binário, precisa rodar o `setcap` novamente (ele é por arquivo).

### Binários pré-compilados

Veja a página [Releases](https://github.com/SEU_USUARIO/rsping/releases). Os arquivos estão nomeados por SO e arquitetura:

- `rsping-linux-x64`
- `rsping-windows-x64.exe`
- `rsping-macos-arm64` (Apple Silicon)
- `rsping-macos-x64` (Intel)

No Linux/macOS, lembre de `chmod +x rsping-*` e de rodar `setcap` no Linux.

---

## Manual de uso

### Alvos

Um **alvo** pode ser especificado de três formas:

| Sintaxe | Significado |
|---|---|
| `8.8.8.8` | ICMP ping para o IP |
| `google.com` | ICMP ping — DNS resolvido uma vez por execução |
| `google.com:443` | **TCP ping** na porta 443 |
| `[::1]:80` | IPv6 com porta — colchetes obrigatórios |

Para **múltiplos alvos**, use `-m a,b,c`. Também é possível passar vários alvos como argumentos posicionais:

```bash
rsping 8.8.8.8 1.1.1.1 google.com:443
```

Todos os alvos são válidos — a diferença é o protocolo.

### Opções da CLI

#### `-m <a,b,c>` — múltiplos alvos

Lista separada por vírgulas. Cada item segue a mesma sintaxe de alvo acima.

```bash
rsping -m 8.8.8.8,1.1.1.1,google.com:443
```

> No modo não-contínuo, os alvos são testados **sequencialmente** (um após o outro). No modo `-t`, são testados em **paralelo**.

#### `-n <N>` ou `-n <N>s` — modo de término

Controla quando parar:

| Valor | Comportamento |
|---|---|
| `-n 100` | 100 pings por alvo (**padrão**) |
| `-n 10s` | 10 segundos por alvo |
| `-t` | Contínuo até você sair pelo menu |

`-n` sobrescreve `-t` e vice-versa, conforme a última opção na linha de comando.

```bash
rsping 8.8.8.8 -n 50
rsping 8.8.8.8 -n 30s
```

#### `-i <segundos>` — intervalo entre pings

Aceita decimais. Padrão: **1 segundo**.

```bash
rsping 8.8.8.8 -i 0.2      # 5 pings por segundo
rsping 8.8.8.8 -i 0        # o mais rápido possível (~10ms)
rsping 8.8.8.8 -i 2.5      # 1 ping a cada 2.5s
```

> **Dica**: para monitorar estabilidade em links de produção, `-i 0.5` a `-i 1` é o intervalo típico. Para detectar micro-picos, use `-i 0.05`. Abaixo de 10ms, você começa a medir a fila do seu próprio kernel.

#### `-l <bytes>` — tamanho do payload

Tamanho do payload ICMP (não inclui cabeçalhos IP/ICMP). Padrão: **56 bytes** (equivalente ao `ping` do sistema).

```bash
rsping 8.8.8.8 -l 1400     # payload grande, testa MTU
rsping 8.8.8.8 -l 1k       # 1 KiB
rsping 8.8.8.8 -l 64k      # 64 KiB (fragmenta)
```

Sufixos aceitos: `k` (KiB), `m` (MiB). Sem sufixo = bytes.

#### `-t` — modo contínuo

Pinga até você abrir o menu com `q` e escolher **Sair**. Útil para monitoramento ao vivo.

```bash
rsping 8.8.8.8 -t
```

Em múltiplos alvos, `-t` coloca **todos em paralelo** (senão ficaria travado no primeiro alvo para sempre).

#### `-w <N>` — warmup

Descarta os primeiros `N` pings de cada alvo antes de começar a contar. Útil quando o primeiro ping sempre tem latência maior (cache DNS, ARP, etc.).

```bash
rsping 8.8.8.8 -w 5        # 5 pings aquecimento, depois começa a contar
```

> No código, `-w` soma ao `-n`: `-n 10 -w 5` envia **15 pings por alvo**, mas só contabiliza os 10 últimos.

#### `-r` / `--report` — compatibilidade

**Deprecated**: o relatório agora é impresso **sempre** ao final. A flag continua aceita para não quebrar scripts antigos.

#### `-q` / `--quiet` — silenciar o relatório

Impede a impressão do relatório final. Útil em scripts onde você só quer o exit code.

```bash
rsping 8.8.8.8 -n 10 -q && echo "rede OK"
```

#### `-ipv4` / `-ipv6` — forçar família de endereço

Sem essas flags, `rsping` usa o que o SO preferir (tipicamente IPv4 primeiro).

```bash
rsping google.com -ipv6
rsping google.com -ipv4
```

#### `--alert-rtt <ms>` — alerta de latência

Pinta a borda do painel de estatísticas de **vermelho** quando a média passar desse valor.

```bash
rsping 8.8.8.8 -t --alert-rtt 50
```

#### `--alert-loss <%>` — alerta de perda

Idem, para taxa de perda.

```bash
rsping 8.8.8.8 -t --alert-loss 1
```

### Teclas durante a execução

| Tecla | Ação |
|---|---|
| `q` / `Esc` | Abre o menu **Sair / Repetir / Continuar** |
| `Espaço` | Pausa / retoma a exibição (pings continuam no fundo) |
| `Tab` | Alterna entre alvos (só com múltiplos alvos) |
| `↑` / `↓` | Scroll no log de pings |
| `End` | Volta ao modo auto-scroll (`[AUTO]` no título) |
| `e` | Exporta CSV (`rsping_<alvo>.csv`) |
| `Ctrl+C` | Sai imediatamente |

Dentro do **menu**:

| Tecla | Ação |
|---|---|
| `↑` / `↓` | Navega entre as opções |
| `Enter` | Confirma |
| `Esc` | Cancela e volta (equivale a **Continuar**) |

### Receitas prontas

**Diagnóstico rápido de um host:**

```bash
rsping google.com -n 30 -i 0.5
```

**Comparação de dois DNS resolvers:**

```bash
rsping -m 8.8.8.8,1.1.1.1 -n 50 -i 0.2
```

**Verificar se o problema é ICMP ou TCP:**

```bash
rsping -m 8.8.8.8,google.com:443 -n 20
```

Se o ICMP falha mas o TCP passa, é bloqueio de firewall no caminho.

**Detectar micro-picos em uma VPS:**

```bash
rsping <ip-da-vps> -t -i 0.1 --alert-rtt 100
```

**Rodar 60 segundos e salvar relatório em log:**

```bash
rsping -m 8.8.8.8,1.1.1.1 -n 60s -i 0.5 > teste.log 2>&1
```

**Script CI — falhar se perda > 1%:**

```bash
rsping 8.8.8.8 -n 20 -q || exit 1
# (o exit code será 0 se completou; analise o CSV para falhas)
```

---

## Entendendo as métricas

| Métrica | O que significa | Como interpretar |
|---|---|---|
| **Mín / Máx** | Menor e maior RTT do período | Máx muito acima da média indica jitter ocasional |
| **Média** | RTT médio | Sozinha, esconde instabilidade |
| **Desvio padrão** | Dispersão em torno da média | σ alto = latências espalhadas |
| **Jitter (RFC 3550)** | Variação entre pings consecutivos, suavizada | **< 10ms**: excelente para VoIP/jogos; **> 30ms**: instável |
| **P50 / P95 / P99** | Percentis — 50%, 95% e 99% dos pings foram menores que este valor | **P99 >> P50** = cauda longa; há picos esporádicos |
| **Spikes** | RTT mais que **dobrou** em relação ao anterior (>1ms) | Rede com buffer bloat ou GC |
| **Limiar outlier** | `média + 3 × desvio` — pings acima disso são outliers | No gráfico braille, aparecem em vermelho |
| **Perda** | % de pacotes sem resposta | **> 1%** já é problema real |

**Jitter RFC 3550** é o cálculo oficial de VoIP (usado no RTP): a cada novo `D = |transit_atual − transit_anterior|`, atualiza `J += (D − J) / 16`. Ele suaviza a série para evitar que um único pico distorça a métrica.

### Gráfico braille vs histograma

- **Braille** (painel inferior): série temporal do RTT. Cada coluna é um ping. Ajuda a ver **quando** os picos ocorreram.
- **Histograma** (painel do meio): **distribuição** das latências em 10 faixas. Ajuda a ver **se há dois clusters** (ex: 20ms 90% do tempo, 200ms nos outros 10%) — sinal clássico de buffer bloat.

---

## O relatório final

Impresso automaticamente ao sair (a menos que `-q`). Exemplo:

```
════════════════════════════════════════════════════════════════════════════
  📊  rsping — RELATÓRIO DE TESTE
════════════════════════════════════════════════════════════════════════════
  Duração total: 12.34s   ·   Modo: 20 pings/alvo   ·   Intervalo: 200ms   ·   Warmup: 0
  Alvos testados: 3 de 3

  ┌─ Resumo comparativo ───────────────────────────────────────────────────
  │ #  Alvo                           Env  Rec  Perda     Mín     Méd     Máx
  ├──────────────────────────────────────────────────────────────────────────
  │ 1  8.8.8.8 ·ICMP                   20   20   0.0%   10.20   12.40   18.90
  │ 2  1.1.1.1 ·ICMP                   20   20   0.0%    8.10    9.30   15.20
  │ 3  google.com ·ICMP                20   19   5.0%   11.30   14.70   45.20
  └──────────────────────────────────────────────────────────────────────────

  ┌─ Alvo 1 de 3: 8.8.8.8 [8.8.8.8] (ICMP) ──────────────────────────────
  │ PACOTES
  │   Enviados .........     20      Recebidos ........     20
  │   Perdidos .........      0      Perda ............   0.0%
  │ LATÊNCIA (ms)
  │   Mínimo ...........      10.20      Máximo ...........      18.90
  │   Média ............      12.40      Desvio ...........       2.10
  │ DISTRIBUIÇÃO (ms)
  │   P50 ..............      11.80      P95 ..............      16.40
  │   P99 ..............      18.70      Spikes ...........          0
  │ JITTER (RFC 3550)
  │   0.85 ms
  │ ESTABILIDADE
  │   ████████████████████████████████████████ [96] EXCELENTE
  │ Avaliação: EXCELENTE — latência baixa, sem perda, jitter baixo
  └──────────────────────────────────────────────────────────────────────────

  ┌─ Ranking ──────────────────────────────────────────────────────────────
  │ Melhor latência .... 1.1.1.1 (9.30 ms)
  │ Melhor jitter ...... 1.1.1.1 (0.42 ms)
  │ Maior perda ........ google.com (5.0%)
  └──────────────────────────────────────────────────────────────────────────
════════════════════════════════════════════════════════════════════════════
```

**Score de estabilidade** (0–100): começa em 100, subtrai `perda × 10`, `jitter × 0.5` e `(média − 50) / 10`. Rótulos: `≥90` EXCELENTE, `70–89` BOM, `50–69` RAZOÁVEL, `30–49` INSTÁVEL, `<30` RUIM.

---

## Arquitetura do projeto

O `main.rs` é um único arquivo (~1300 linhas), organizado em blocos. Se você for modificar, é útil saber a divisão:

| Bloco | Responsabilidade |
|---|---|
| `TargetSpec` | Parse do alvo `host[:porta]` e formatação para exibição |
| `Config` | Parse de argumentos da CLI (estilo PsPing) |
| `PingUpdate` / `Message` | Estruturas de comunicação entre coletor e UI |
| `TargetStats` | Estatísticas por alvo (média, desvio, jitter, percentis, spikes) |
| `App` | Estado da UI + orquestração (sequencial/paralelo, pausa, menu) |
| `icmp_loop` / `tcp_loop` | Coletores assíncronos — um por alvo |
| `resolve` / `resolve_socket` | Resolução DNS com preferência IPv4/IPv6 |
| `render_*` | Desenho dos painéis (log, stats, histograma, braille) |
| `print_summary` | Relatório final no terminal padrão |
| `main` | Loop de eventos (`tokio::select!` sobre teclado, timer, canal) |

### Fluxo de dados

```
[icmp_loop / tcp_loop]  ──PingUpdate──▶  mpsc channel  ──▶  App.push_update()
      (tokio task)                                              │
                                                                ▼
                                          draw()  ◀── terminal.draw(|f| app.view(f))
                                                                │
                                                                ▼
                                                     [ratatui desenha a TUI]
```

O loop principal usa `tokio::select!` para simultaneamente:

1. Ler eventos do teclado (`EventStream` do crossterm)
2. Processar mensagens do canal de ping (`mpsc`)
3. Disparar redesenho a cada 50ms (`tick`)

Nenhuma thread bloqueia outra — pings rodam em `tokio::spawn` separado por alvo.

---

## Dependências do Cargo

Cada crate foi escolhido por um motivo específico. Se você for manter o projeto, entenda o papel de cada uma:

### `ratatui = "0.29"`

**Framework de TUI.** É o sucessor do `tui-rs`, ativamente mantido. Fornece os widgets que usamos:

- `Paragraph` — log de pings (painel esquerdo) e painel de estatísticas
- `Sparkline` — *não usamos* (substituído por `Canvas`)
- `BarChart` + `Bar` + `BarGroup` — histograma
- `Canvas` + `Line` — gráfico em braille
- `Layout` — divisão proporcional dos painéis
- `Block` — bordas e títulos
- `Clear` — limpa área do menu flutuante
- `Frame` — abstração da tela renderizada

### `crossterm = "0.28"` (features: `event-stream`)

**Backend de terminal.** Cuida de:

- Colocar o terminal em modo raw (sem eco, sem line-buffering)
- Ler teclas (incluindo setas, `Tab`, combinações como `Ctrl+C`)
- Restaurar o estado do terminal ao sair

O feature `event-stream` habilita `EventStream`, que expõe eventos como um `Stream` (usado no `tokio::select!`).

> **Por que não `termion`?** `crossterm` é multiplataforma (funciona em Windows nativo); `termion` é Linux/macOS apenas.

### `surge-ping = "0.8"`

**Cliente ICMP assíncrono** para Linux/macOS. Construído sobre `tokio`, `socket2` e `pnet_packet`. Vantagens:

- Assíncrono nativo (não bloqueia a UI)
- Suporta *unprivileged ICMP sockets* no Linux (dispensa `CAP_NET_RAW` em alguns kernels)
- Calcula RTT em nível de microssegundos

**Não é usado no Windows** — lá, usamos `winping-rs` (ver abaixo).

### `tokio = "1"` (features: `full`)

**Runtime assíncrono.** Toda a concorrência do programa é gerenciada por ele:

- `tokio::spawn` — cria uma task por alvo (ping independente)
- `tokio::select!` — multiplexa teclado + canal + timer no loop principal
- `tokio::time::sleep` / `sleep_until` — agendamento por deadline (sem drift)
- `tokio::sync::mpsc` — canal unbounded entre coletor e UI
- `tokio::net::TcpStream` — TCP ping
- `tokio::net::lookup_host` — resolução DNS assíncrona
- `tokio::task::spawn_blocking` — ponte para o `winping-rs` no Windows (API síncrona)

O feature `full` traz todos os módulos (`rt-multi-thread`, `net`, `time`, `sync`, `macros`).

### `futures = "0.3"`

**Utilitários de stream.** Usado apenas para `StreamExt::next()`, que permite consumir `EventStream` (do crossterm) dentro do `tokio::select!`.

### `winping-rs = "0.11"` (apenas em Windows)

**Cliente ICMP via API nativa `IcmpSendEcho`.** Marcado como `target.'cfg(windows)'.dependencies`:

```toml
[target.'cfg(windows)'.dependencies]
winping-rs = "0.11"
```

- **Por que não `surge-ping` no Windows?** Porque raw sockets exigem Administrador. `winping-rs` usa a API `iphlpapi.dll`, que funciona para usuários comuns.
- **Trade-off**: `winping-rs` é **síncrono**, então é chamado dentro de `spawn_blocking`.
- O nome do pacote é `winping-rs`, mas a biblioteca é importada como `winping` (drop-in do antigo `winping`).

### Resumo do grafo de dependências

```
rsping
├── ratatui ──── (widgets)
│   └── crossterm
├── crossterm ──── (terminal + eventos)
├── surge-ping ──── (ICMP Linux/macOS)
│   ├── tokio
│   ├── socket2
│   └── pnet_packet
├── winping-rs ──── (ICMP Windows, só target 'cfg(windows)')
├── tokio ──── (async runtime)
└── futures ──── (StreamExt)
```

---

## Cross-compilation para Windows

Se você está em Linux e quer gerar o `.exe`:

### Pré-requisitos (Arch/CachyOS)

```bash
sudo pacman -S mingw-w64-gcc
rustup target add x86_64-pc-windows-gnu
```

> **Atenção**: não instale o pacote `rust` do pacman — ele **conflita** com `rustup` e não suporta cross-compilation. Use apenas `rustup`.

### Configuração (uma vez, por projeto)

Crie `.cargo/config.toml` na raiz:

```toml
[target.x86_64-pc-windows-gnu]
linker = "x86_64-w64-mingw32-gcc"
ar = "x86_64-w64-mingw32-gcc-ar"
rustflags = ["-C", "target-feature=+crt-static"]
```

O `+crt-static` embute o runtime MinGW **dentro** do `.exe`, tornando-o portátil. Sem isso, o Windows pode reclamar de `libwinpthread-1.dll`.

### Build

```bash
cargo build --release --target x86_64-pc-windows-gnu
```

Binário em: `target/x86_64-pc-windows-gnu/release/rsping.exe`

### Verificar portabilidade

```bash
x86_64-w64-mingw32-objdump -p target/x86_64-pc-windows-gnu/release/rsping.exe | grep "DLL Name"
```

Deve listar apenas DLLs do Windows (`KERNEL32.dll`, `WS2_32.dll`, `IPHLPAPI.dll`, `msvcrt.dll`). Se aparecer `libwinpthread-1.dll`, o `+crt-static` não pegou — verifique o `.cargo/config.toml`.

### Levar para Windows

O `.exe` resultante:

- **Não precisa de Admin** para ICMP (usa `IcmpSendEcho`) nem para TCP
- Pode ser disparado por duplo-clique, mas **abra um terminal na pasta** para ver o relatório final
- Na primeira execução, o SmartScreen pode bloquear: **Mais informações → Executar assim mesmo**

---

## Solução de problemas

### `Operation not permitted` no Linux

Você esqueceu do `setcap`:

```bash
sudo setcap cap_net_raw+ep ./target/release/rsping
```

Se continuar falhando em containers ou WSL, pode ser política do kernel. Tente `sudo ./target/release/rsping`.

### `Address already in use` ou erros de socket

Sua versão do Rust está antiga. Atualize: `rustup update stable`.

### `surge-ping` falha com "host is unreachable" em todas as tentativas

Firewall corporativo bloqueando ICMP. Use o modo TCP: `rsping host:443`.

### `Tab` não muda de alvo

Só funciona com **mais de um alvo**. Se você passou um só (`rsping 8.8.8.8`), não há abas.

### Gráfico braille aparece vazio

Seu terminal não suporta caracteres braille Unicode. Configure um terminal moderno (Alacritty, Kitty, WezTerm, Windows Terminal) ou use `-q` para o relatório apenas.

### Windows: "Windows protegeu o PC"

SmartScreen não reconhece o `.exe` (sem assinatura digital). Clique em **Mais informações → Executar assim mesmo**. Só ocorre na primeira execução.

### Windows: `.exe` fecha imediatamente ao dar duplo-clique

O programa imprime o relatório no `stdout`, que é fechado quando o terminal fecha. **Sempre execute por um terminal**: `cmd` ou PowerShell → `cd pasta` → `rsping.exe`.

---

## Contribuindo

Issues e PRs são bem-vindos. Antes de mandar PR:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Para mudanças na UI, anexe um print da TUI no PR.

---

## Licença

MIT — veja [LICENSE](LICENSE).