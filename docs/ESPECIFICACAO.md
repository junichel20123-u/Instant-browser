# PROMPT MESTRE — Projeto "Rajada": acelerador e gerenciador de downloads para Linux

> **Nome provisório:** "Rajada" (binários `rajada`, `rajadad`, `rajada-gtk`, `rajada-host`). Pode ser trocado depois. Evitei "Fluxo" porque já existe um projeto chamado FluxDown.
>
> **Resumo em uma frase:** um motor de download para Linux, escrito em Rust, que descobre sozinho a forma mais rápida **e educada** de baixar cada arquivo, nunca entrega um arquivo errado, é seguro por padrão e explica em português claro por que um download está lento. Ele roda como um daemon leve, com CLI, TUI, extensão de navegador e uma interface gráfica nativa em GTK4/libadwaita por cima.

---

## 0. Como você (Claude Code) deve usar este documento

1. Este documento é a **especificação completa** do projeto. Ele resume três pesquisas: o que realmente acelera downloads, onde as ferramentas existentes falham e como deve ser a interface gráfica. Trate-o como a fonte de verdade sobre visão, prioridades e decisões de arquitetura.
2. No primeiro commit, salve uma cópia integral deste texto em `docs/ESPECIFICACAO.md`. Depois crie um `CLAUDE.md` **curto (menos de 200 linhas)** com o essencial: visão em 3 linhas, princípios inegociáveis, layout do workspace, comandos de build/teste, regras de código e um ponteiro para `docs/ESPECIFICACAO.md`. Arquivos `CLAUDE.md` longos são seguidos com menos fidelidade, então mantenha-o enxuto e atualize-o quando uma decisão mudar.
3. **Comece em modo de planejamento.** Leia tudo, proponha a estrutura do workspace e o plano detalhado da Fase 0, e **espere minha aprovação antes de escrever código**.
4. Trabalhe em **tarefas pequenas e verificáveis**: implemente uma coisa, escreva os testes, rode `cargo test`, `cargo clippy` e `cargo fmt --check`, e só então passe para a próxima. Faça commits pequenos com mensagens claras.
5. Quando algo aqui conflitar com a realidade (a API de um crate mudou, uma versão nova saiu, uma premissa está errada), **avise, explique e proponha a alternativa**. Não invente APIs e não finja que algo funciona sem ter rodado.
6. **Pergunte antes de:** adicionar uma dependência pesada ou com licença incomum; mudar uma decisão de arquitetura descrita aqui; publicar qualquer coisa (crates.io, lojas de extensões, Flathub); escolher o nome definitivo e a licença final; executar qualquer comando que exija root ou altere configurações do sistema.
7. **Nunca declare ganho de desempenho sem número.** Toda afirmação do tipo "ficou mais rápido" ou "ficou mais leve" precisa de um benchmark reproduzível, registrado em `BENCHMARKS.md`.
8. Idiomas: código, identificadores, comentários de código e mensagens de commit em **inglês**. Textos para o usuário (CLI, TUI, GUI, doctor, notificações) em **português do Brasil e inglês**, via sistema de tradução (gettext). A documentação do usuário (`README`, `docs/`) fica em pt-BR, com versão em inglês quando o projeto amadurecer.

---

## 1. Missão

Construir uma ferramenta de download para Linux que:

- **acelera de verdade quando é possível**, usando múltiplas conexões HTTP Range, espelhos e IPs de CDN medidos, ajustados automaticamente;
- **não promete o impossível**: nenhum software passa da velocidade contratada do plano de internet, e o programa deixa isso claro;
- **é confiável**: retoma depois de queda, crash, suspensão do notebook, troca de Wi-Fi e link assinado que expirou, e **nunca marca como concluído um arquivo corrompido ou incompleto**;
- **é seguro por padrão**: sem portas abertas, sem RPC sem autenticação, sem telemetria, com builds assinados e reproduzíveis;
- **é educado com servidores**: respeita `Retry-After`, recua diante de 429/503 e tem teto de conexões por host;
- **é intuitivo**: sem jargão por padrão, com explicações humanas, interface nativa, leve e bonita;
- **é leve**: daemon de cerca de 15 MB de RAM parado, GUI com ~0% de CPU quando nada acontece.

O objetivo não é ser "mais um acelerador". É ser **o gerenciador de downloads em que o usuário confia**, porque ele prova que o arquivo está certo, explica o que está acontecendo e não pesa no sistema.

---

## 2. Contexto técnico: o que realmente acelera um download (e o que é mito)

Leve este contexto em conta em todas as decisões, na documentação e nos textos da interface.

### 2.1 O que funciona

- **Download segmentado (HTTP Range).** O cliente divide o arquivo em pedaços e baixa cada um numa conexão TCP separada com `Range: bytes=início-fim`, gravando cada pedaço direto na posição certa do arquivo final (`pwrite`), sem etapa de "juntar". Isso acelera em três cenários:
  1. **O servidor limita a velocidade por conexão.** Num teste publicado, com um servidor limitado a 2 MiB/s por conexão, wget e curl levaram cerca de 24 s para 50 MB; o aria2 com 16 conexões e segmentos de 1 MB levou cerca de 2,3 s. Com o tamanho mínimo de segmento padrão do aria2 (20 MB), o arquivo virou só 2 pedaços e levou 12,2 s. **Lição: o tamanho do segmento importa tanto quanto o número de conexões.**
  2. **Latência alta (RTT alto).** A vazão de uma conexão TCP é limitada por janela ÷ RTT (produto banda × atraso, o BDP). Várias conexões somam janelas. Um benchmark de blog mostrou wget e aria2 empatados com 8 ms de RTT e o aria2 com 16 conexões 5,6× mais rápido com 110 ms (é um número ilustrativo, sem metodologia auditável).
  3. **Perda de pacotes.** Uma única conexão TCP perde muita velocidade com perdas pequenas, e várias conexões diluem esse efeito.
- **Retomada robusta.** Na prática é o maior ganho de tempo para o usuário comum: não recomeçar do zero.
- **Múltiplas fontes.** Espelhos, Metalink (RFC 5854) e `Link: rel=duplicate` (RFC 6249) somam capacidades e contornam limites por IP.
- **Escolha de IP de CDN.** Resolver todos os registros A/AAAA, medir cada IP e distribuir os segmentos pelos melhores. Trocar de DNS em si quase não muda a velocidade; o que muda é qual nó de CDN você recebe.
- **Reuso de conexão e HTTP/2** ajudam muito em **lotes de arquivos pequenos**, mas não contornam limite por conexão num arquivo grande (todos os fluxos HTTP/2 dividem uma única conexão TCP).

### 2.2 O que não funciona ou é mito

- **Mais conexões quando o seu link já está cheio:** só criam disputa e *bufferbloat*.
- **Limites por IP ou por conta:** mais conexões não somam nada e podem gerar bloqueio.
- **BBR no cliente:** é um algoritmo do lado de quem **envia**. No PC do usuário ele afeta sobretudo uploads. Os ganhos de "3× a 10×" de blogs de VPS são do lado do servidor. Não prometa isso.
- **Buffers do kernel (`net.core.rmem_max`, `net.ipv4.tcp_rmem`):** afetam o download, mas só quando o BDP passa do máximo do autoajuste (links de centenas de Mbps a Gbps com RTT alto). Em banda larga doméstica comum o ganho é de poucos por cento. O programa pode **sugerir** ajustes no `doctor`, mas **nunca altera o sysctl sozinho**.
- **HTTP/3/QUIC para downloads grandes em rede rápida:** um estudo de 2024 ("QUIC is not Quick Enough over Fast Internet", WWW '24) mediu até 45% menos vazão que TCP+TLS+HTTP/2 por custo de processamento no receptor. HTTP/3 fica para uma fase tardia, opcional, com volta automática para HTTP/2.

### 2.3 Expectativas realistas (use estes textos no README e na interface)

| Cenário | Ganho esperado |
|---|---|
| Servidor que limita a velocidade por conexão | várias vezes mais rápido, até o limite do seu plano |
| CDN grande (GitHub Releases, Steam, Microsoft, Cloudflare) em link doméstico de baixa latência | normalmente 0–20%; uma conexão já costuma encher o link |
| Servidor em outro continente ou rede com perdas | 2× a 5× é plausível |
| Qualquer cenário | **confiabilidade**: retomar, verificar e não perder horas. Venda isso tanto quanto a velocidade |

---

## 3. Princípios inegociáveis (em ordem de prioridade)

Quando dois princípios entrarem em conflito, o de cima vence.

1. **Nunca entregar um arquivo errado.** Nada fica "concluído" sem passar por verificação (tamanho exato, validador do recurso inalterado e hash quando disponível). O arquivo é baixado como `.part` e só recebe o nome final depois de verificado.
2. **Seguro por padrão.** Nenhuma porta TCP aberta por padrão. Comunicação local por socket Unix com permissão 0600. Token gerado automaticamente quando houver TCP. Diretórios de gravação restritos a uma lista permitida. Sem telemetria. Segredos nunca em argv nem em logs.
3. **Educado com servidores.** Teto de conexões por host, respeito a `Retry-After`, recuo diante de 429/503, User-Agent honesto. Mais conexões que o padrão exigem opt-in explícito por host.
4. **Honesto sobre velocidade.** Nada de "até 10× mais rápido". O programa mede, decide e explica. Se mais conexões não ajudam, ele usa uma.
5. **Leve.** Daemon em Rust sem GC, GUI nativa sem WebView, zero trabalho quando nada muda (sem polling, sem spinners infinitos, sem animação contínua).
6. **Uma verdade só.** CLI, TUI, GUI e extensão usam os mesmos nomes de estado, os mesmos códigos de diagnóstico, os mesmos textos e os mesmos ícones. Todos são clientes da mesma API.
7. **Explicável.** Todo estado tem um motivo legível, e toda falha tem um próximo passo concreto (um botão ou um comando), nunca só um código de erro.

---

## 4. Lições dos concorrentes: o que eles erram e o que fazemos diferente

Estas falhas são documentadas em issues públicas (os números são das issues nos repositórios de cada projeto). Use-as como casos de teste e como motivação de design.

### 4.1 aria2 (o motor de referência)

| Falha | Causa raiz | Nossa resposta |
|---|---|---|
| Uma conexão abortada termina com status "OK", mas o arquivo sai corrompido (#886) | Segmento falho não é reatribuído nem verificado; "concluído" vem de contadores | Segmento só fica "done" com exatamente `end-start+1` bytes recebidos e gravados no journal; download só fica "complete" depois do verificador final |
| Servidores sem resume deixam arquivos corrompidos com o nome final (#1278, #1279) | Sem controle quando não há Range; arquivo parcial fica com o nome final | Sempre escrever em `nome.part`, renomear atomicamente só depois de verificar; marcar downloads não retomáveis na interface |
| EOF prematuro não dispara retry automático; um arquivo de 122 GiB precisou de 13 retries manuais (#2408) | EOF classificado como erro terminal | Taxonomia de erros: EOF prematuro é transitório, com backoff e jitter, sem limite total enquanto houver progresso |
| Alocação de arquivo bloqueia o RPC e a interface (#953); `falloc` em sistemas de arquivos antigos trava tudo | I/O de disco síncrono no thread principal | Alocação em `spawn_blocking`, estratégia escolhida pelo tipo de sistema de arquivos; o RPC nunca bloqueia |
| Limite fixo de 16 conexões por servidor (#126, #580, #729, #1039) | Limite codificado como proteção anti-abuso, sem controle adaptativo | Teto configurável, mas o número efetivo é decidido pelo controlador adaptativo, e a interface explica quando o teto limita |
| `Retry-After` ignorado: um 429 mata o download (#2295) | Política de retry fixa | Ler `Retry-After` (segundos ou data HTTP) em 429/503, com teto; reduzir conexões pela metade |
| RPC sem segredo por padrão; a issue #2402 descreve `addUri` com `dir=/etc/cron.d` levando a execução de código | RPC nasce aberto, segredo opcional | Socket Unix 0600 por padrão, token automático no TCP, lista de diretórios permitidos |
| Misturar URIs de recursos diferentes pode corromper sem aviso (documentado no manual) | Sem verificação de identidade entre espelhos | "Contrato de identidade": mesmo tamanho e mesmo validador; espelho divergente vai para quarentena |
| Arquivos de controle órfãos e arquivos esparsos corrompidos em lote (#682) | Finalização não atômica | Journal com `fdatasync` antes de marcar progresso; rename atômico; limpeza na mesma transação |
| URLs assinadas (S3/CDN) expiram e o download morre; o S3 devolve `ExpiredToken` como 400, não 403 | Não existe o conceito de "URL renovável" | Estado "Precisa de você: link expirou", progresso preservado, hook de renovação |
| Última versão oficial em novembro de 2023; forks absorvem correções | Bus factor de um mantenedor | Governança aberta, CI público, releases regulares, `SECURITY.md` |

**Lição extra:** uma reescrita do aria2 em Rust (`balovess/aria2_rust`) teve bugs de retomada voltando ao primeiro ponto de pausa (#78) e corrupção ao truncar o arquivo enquanto o checkpoint ainda listava peças concluídas (PR #87). **Rust elimina bugs de memória, não bugs de estado.** Journal e testes de crash são obrigatórios.

### 4.2 Interfaces e outros gerenciadores

| Projeto | Falha documentada | Nossa resposta |
|---|---|---|
| AriaNg | Rotas por link (`#/new/:url`, `#/settings/rpc/set/...`) permitem que um link forjado adicione downloads ou redirecione o RPC (#842) | API exige token, confere `Origin` e `Host` (anti DNS-rebinding); ação vinda de deep link sempre pede confirmação |
| Motrix (original) | Por anos deixou o aria2 com RPC sem segredo (#138); a série 2.x de 2026 passou a prender o RPC ao loopback | Seguro desde o primeiro dia, um único binário, sem Electron |
| Motrix Next / Rayburst | Engine que não inicia, extensão que não aciona o app, Google Drive salvando 38 KB de HTML em vez de 30 GB (#61); WebView a 99–100% de CPU (#799), corrigido parando atualizações com a janela oculta | Detectar HTML no lugar do arquivo **antes** de salvar; GUI nativa que não trabalha quando oculta |
| Persepolis | "Aria2 didn't respond!" recorrente; na versão 5 abandonou o aria2 (limite de 16 conexões, sem SOCKS5, crashes no XML-RPC) | Motor como biblioteca no mesmo processo do daemon, sem processo externo; SOCKS5 suportado |
| uGet | Declarado sem desenvolvimento ativo; issues sem resposta | Ocupar o espaço de "app GTK moderno"; aprender com o `uget-integrator` |
| AB Download Manager | ~300 MB de RAM parado (#1475); travamentos de dropdown no KDE (#1053); travava ao abrir no Wayland (#660); bandeja instável (#126, #1396) | Daemon Rust (meta de ~15 MB); GUI GTK4 nativa; não depender de bandeja |
| Surge | CPU 4,5× maior que a do aria2 em velocidade parecida (#73); downloads da extensão salvos em `/` quando rodando como serviço (#645, #697); faltava login/cookies (#96) | Buffers grandes e `pwrite` sem cópias; todo caminho resolvido para absoluto; extensão envia cookies e cabeçalhos |
| Gopeed | Rampa de conexões lenta: ~10 s de atraso no início com 256 threads (#1450); só 1 conexão quando falta `Accept-Ranges` mas o servidor responde 206 (#1652); data races (#1563); velocidade reportada inflada (#1646) | Sondas em lote (1→2→4→8); sondagem real de Range; modelo de atores com dono único do estado; velocidade medida em bytes do socket |
| XDM | Extensão fora das lojas, instalada em modo desenvolvedor (#918); só funciona com o app aberto (#589); erros no MV3 (#1168); interface minúscula em HiDPI/Wayland (#1337); sem modo escuro (#152) | Extensão só de captura (sem baixar vídeo de sites), publicada nas lojas; native host que liga o daemon; GUI com escala fracionária e tema do sistema |
| axel | Arquivo de estado corrompe e impede retomar (#233); timeout reinicia do zero **sem truncar**, deixando o arquivo maior e corrompido (#303) | Journal atômico; reinício do zero sempre trunca; tamanho final conferido |
| wget2 | `--chunk-size` com segfaults e sem truncar arquivo existente (#177, #279, #459, #463); compressão aplicada em ranges | Sempre `Accept-Encoding: identity` em requisições com Range; tratar GOAWAY do HTTP/2 |
| lftp | `pget` entrega arquivo com tamanho certo e **checksum errado, sem erro** (#523); `mirror` apaga o status e deixa arquivo incompleto (#688) | Hash por segmento quando houver digest; nunca concluir sem verificar |
| curl | O mantenedor disse que segmentar um único arquivo não está no plano; `--parallel` só paraleliza URLs diferentes | Não competir com o curl; oferecer flags familiares (`-H`, `-b`, `-o`, `-O`, `-A`) para facilitar migração de scripts |
| KGet | Downloads "parados" sem conseguir retomar; crashes com vários downloads; praticamente sem mantenedor | Watchdog de stall com reatribuição automática |
| JDownloader 2 | Instalador com adware; padrões que ninguém pediu ligados por padrão; lento para abrir | Sem instalador com ofertas; padrões inteligentes; divulgação progressiva |
| Free Download Manager | O site oficial redirecionou parte dos usuários Linux, entre 2020 e 2022, para um `.deb` com backdoor (relatório da Kaspersky, set/2023) | Distribuição só por canais assinados, builds reproduzíveis, proveniência verificável |
| Replicate pget | Lista como melhoria futura escrever no disco conforme chega e checar hash em segundo plano | Escrita incremental e hash incremental desde o MVP |

### 4.3 Problemas transversais do ecossistema

1. "Concluído" não significa "correto".
2. A identidade do recurso não é garantida entre conexões e retomadas.
3. Links que expiram e downloads que dependem de sessão de login quebram tudo.
4. Ninguém trata suspensão do notebook e troca de rede de forma explícita.
5. RPC aberto por padrão é comum.
6. A integração com navegador no Linux é frágil (native messaging, navegadores em Flatpak).
7. O ajuste é manual em todo lugar (`-x`, `-s`, `-k`, `-n`).
8. Pouca cortesia embutida.
9. Governança frágil: muitos projetos parados ou fragmentados.
10. As interfaces bonitas são pesadas, as leves são datadas e as poderosas são confusas.

**Nenhum concorrente junta: visual moderno, leveza real, explicações compreensíveis, verificação comprovada e segurança por padrão.** Esse é o espaço do Rajada.

---

## 5. Arquitetura

### 5.1 Visão geral

```
┌──────────────────────────── Clientes ─────────────────────────────┐
│ rajada (CLI) │ rajada-tui │ rajada-gtk (GUI) │ Extensão → rajada-host │ AriaNg/Motrix (modo compat, opcional) │
└───────────────────────────────┬───────────────────────────────────┘
                                │ JSON-RPC 2.0 sobre socket Unix (padrão)
                                │ WebSocket em 127.0.0.1 + token (opcional, opt-in)
┌───────────────────────────────▼───────────────────────────────────┐
│ rajadad (daemon)                                                   │
│  ├─ API (versionada) + assinatura de eventos por nível             │
│  ├─ Gerenciador de fila e agendamento                              │
│  ├─ Política de cortesia (tetos por host, Retry-After, allow/deny) │
│  ├─ Planejador de fontes (IPs, espelhos, Metalink)                 │
│  ├─ Controlador adaptativo (nº de conexões, tamanho de segmento)   │
│  ├─ Pool de workers (tarefas tokio: Range GET, retry, work stealing)│
│  ├─ Escritor de disco (pwrite, pré-alocação, fdatasync periódico)  │
│  ├─ Journal persistente (SQLite WAL)                               │
│  ├─ Verificador de integridade (tamanho, validador, hash)          │
│  ├─ Doctor (diagnóstico estruturado)                               │
│  └─ Integração com o sistema (logind, NetworkManager, notificações)│
└────────────────────────────────────────────────────────────────────┘
```

O **motor é uma biblioteca** (`rajada-engine`). O daemon o usa, e a CLI também pode usá-lo diretamente, sem daemon, para `rajada get URL` no MVP.

### 5.2 Layout do workspace Cargo (proposta; ajuste no plano da Fase 0 se tiver motivo)

```
rajada/
├─ Cargo.toml                 # workspace
├─ CLAUDE.md
├─ BENCHMARKS.md
├─ SECURITY.md
├─ docs/
│  ├─ ESPECIFICACAO.md        # este documento
│  ├─ ARQUITETURA.md
│  ├─ API.md                  # métodos, eventos, versões
│  └─ adr/                    # registros de decisões de arquitetura
├─ crates/
│  ├─ rajada-types/           # estados, motivos, códigos de diagnóstico, eventos, esquema da API (serde)
│  ├─ rajada-engine/          # motor de download (biblioteca)
│  ├─ rajada-store/           # journal SQLite
│  ├─ rajada-i18n/            # catálogo de mensagens (gettext), formatação de números/tempos
│  ├─ rajada-daemon/          # bin: rajadad
│  ├─ rajada-cli/             # bin: rajada
│  ├─ rajada-tui/             # bin: rajada-tui (ou subcomando `rajada tui`)
│  ├─ rajada-gtk/             # bin: rajada-gtk
│  ├─ rajada-host/            # bin: rajada-host (native messaging)
│  └─ faultbox/               # bin: servidor de teste com injeção de falhas
├─ extension/                 # WebExtension MV3 (Chrome/Chromium/Brave/Edge e Firefox)
├─ packaging/                 # flatpak, systemd, .desktop, metainfo, AUR, deb, rpm
└─ xtask/                     # automação: bench, release, empacotamento
```

### 5.3 Princípios de arquitetura

- **Modelo de atores:** cada download tem um ator (tarefa tokio) dono do seu estado. A comunicação é por canais com limite (backpressure). Não existe estado mutável compartilhado com locks espalhados.
- **O escalonador é a única fonte de verdade** sobre quantas conexões existem e quais downloads estão ativos.
- **Erros tipados** (`thiserror` nas bibliotecas, `anyhow` só nos binários). Sem `unwrap()`/`expect()` em código de rede, disco ou API; use apenas em testes e em invariantes comprovadas com comentário.
- **Clientes não calculam nada importante:** o daemon envia estados, motivos e diagnósticos estruturados (códigos + parâmetros); os clientes só traduzem e exibem.

---

## 6. Stack tecnológica

| Área | Escolha | Observação |
|---|---|---|
| Linguagem | Rust estável (edição 2021 ou 2024) | Binários pequenos, sem runtime, seguros com dados de rede não confiáveis |
| Runtime assíncrono | `tokio` | |
| HTTP | `reqwest` (sobre `hyper`) com `rustls` | Feature `socks` para SOCKS5; HTTP/2 suportado |
| HTTP/3 (fase tardia) | `h3` + `quinn`, ou recurso experimental do reqwest | Opt-in |
| Bytes | `bytes` | Evitar cópias |
| CLI | `clap` (derive) | |
| Progresso na CLI | `indicatif` | |
| TUI | `ratatui` + `crossterm` | |
| GUI | GTK4 + libadwaita via `gtk4-rs`/`libadwaita-rs`, com **Relm4** | Ver seção 12 |
| Bandeja (KDE e outros) | `ksni` (StatusNotifierItem) | Complemento, nunca dependência |
| Portais XDG | `ashpd` | Background, notificações, configurações (cor de destaque, contraste, reduzir animações) |
| D-Bus (logind, NetworkManager) | `zbus` | |
| Journal | `rusqlite` (modo WAL) | |
| Hashes | `sha2`, `blake3` (interno), `md-5`/`sha1` só para compatibilidade com Metalink antigo | |
| Serialização | `serde`, `serde_json` | |
| Logs | `tracing` + `tracing-subscriber` | Com redação de segredos |
| Segredos | `secrecy` + `zeroize` | Tipo `Secret` sem `Debug` |
| i18n | `gettext-rs` | Um catálogo para todos os clientes |
| Testes | `proptest`, `loom`, `wiremock`, `faultbox` próprio, `insta` (snapshots de texto) | |
| Benchmarks | `hyperfine`, `criterion` | |
| Qualidade | `clippy -D warnings`, `rustfmt`, `cargo-deny`, `cargo-audit` | |
| Servidor de teste | `axum`/`hyper` | |
| Extensão | JavaScript moderno (ES modules) sem bundler, ou TypeScript com build mínimo | Simplicidade facilita a revisão nas lojas |

**Por que não Go:** funcionaria (o Surge prova), mas competiria diretamente com Surge e Gopeed. Rust diferencia e entrega memória e CPU menores.
**Por que não Python:** serve para protótipo, mas desempenho com muitas conexões TLS e empacotamento pesam.
**Por que não Tauri/Electron na GUI:** ver seção 12.1.

---

## 7. O motor de download (`rajada-engine`)

### 7.1 Máquina de estados

```
Queued → Probing → Downloading ⇄ Paused
                      │
                      ├→ PoliteWait (servidor pediu para esperar; contagem regressiva)
                      ├→ NeedsUser(reason)  (link expirou, precisa de login, recurso mudou)
                      ├→ Verifying → Complete(VerificationLevel)
                      └→ Failed(reason)
```

- `NeedsUser(reason)` com motivos como `UrlExpired`, `LoginRequired`, `HtmlInsteadOfFile`, `ResourceChanged`, `DiskFull`, `PermissionDenied`.
- `Failed(reason)` só para falhas definitivas, e **toda** falha tem uma ação de recuperação sugerida.
- `VerificationLevel`:
  - `HashMatched { algo, source }`, em que `source` pode ser servidor (`Repr-Digest`/`Content-Digest`, RFC 9530), Metalink ou usuário (`--sha256`);
  - `SizeAndValidator`: tamanho exato e ETag forte ou `Last-Modified` inalterados durante todo o download;
  - `SizeOnly`;
  - `Unverified`: servidor sem `Content-Length`, como streams e ZIPs gerados na hora.
- A interface mostra o nível com honestidade: "Verificado (hash)", "Verificado (tamanho)" ou "Concluído sem verificação".
- Estados e motivos ficam em `rajada-types` e são **os mesmos** em todos os clientes.

### 7.2 Sondagem (Probing)

1. Fazer `GET` com `Range: bytes=0-` e `Accept-Encoding: identity`. Use GET, não HEAD, porque alguns servidores mentem no HEAD.
2. **206 com `Content-Range` válido** (`bytes 0-N/TOTAL`, com N e TOTAL coerentes): suporte a Range confirmado. O corpo dessa resposta **vira o segmento 0** (nada é desperdiçado).
3. **200 sem `Accept-Ranges`:** fazer uma segunda sonda com `Range: bytes=<meio>-<meio+1>`. Se vier 206 com o intervalo exato, ativar a segmentação. Esse é o caso Gopeed #1652, de servidores que não anunciam Range mas o suportam.
4. **200 com corpo inteiro a um pedido de Range** (servidor que ignora Range): seguir com uma conexão, sem corromper.
5. Registrar: tamanho total, ETag (forte ou fraco), `Last-Modified`, `Content-Type`, `Content-Disposition` (nome do arquivo, inclusive `filename*` com UTF-8), digest (`Repr-Digest`/`Content-Digest`), `Alt-Svc` (para uma futura decisão de HTTP/3) e o IP do servidor.
6. **Detectar página HTML no lugar do arquivo:** `Content-Type: text/html` quando o esperado não é HTML (pela extensão da URL, pelo tipo informado pela extensão do navegador ou pelo tamanho esperado), ou os primeiros bytes começando com `<!DOCTYPE html` / `<html`. Resultado: `NeedsUser(HtmlInsteadOfFile)` com a mensagem "Este link abre uma página, não um arquivo. Provavelmente precisa de login ou confirmação."
7. Seguir redirecionamentos (com limite) e registrar a URL final, mas manter a original para renovação.

### 7.3 Contrato de identidade do recurso

- Toda requisição de segmento depois da sondagem leva `If-Range`:
  - com ETag **forte** quando houver (a RFC 9110 não permite ETag fraco em `If-Range`);
  - senão com `Last-Modified`;
  - se não houver nenhum dos dois, validar `Content-Range` (total igual) e, na retomada, comparar uma amostra dos primeiros e últimos 64 KiB já baixados.
- **Resposta 200 onde se esperava 206** significa que o recurso mudou: estado `NeedsUser(ResourceChanged)`, sem misturar bytes de versões diferentes. A opção `--on-change=restart` permite recomeçar automaticamente.
- Todo espelho ou IP adicional precisa ter o mesmo tamanho e, se houver, o mesmo ETag/digest. Uma fonte divergente vai para quarentena e é registrada no doctor.

### 7.4 Escrita em disco

- Escrever sempre em `<nome>.part` no diretório de destino e, só depois da verificação, fazer `rename(2)` atômico para o nome final. Se o nome final já existir, aplicar a política escolhida (renomear com sufixo, perguntar ou sobrescrever).
- Gravação posicional com `std::os::unix::fs::FileExt::write_at` (`pwrite`), em buffers de **256 KiB a 1 MiB** por segmento, sem lock global e sem cópias extras (`bytes::Bytes`). O trabalho de disco roda em `spawn_blocking` ou num pool dedicado; o loop assíncrono nunca bloqueia.
- **Pré-alocação sensível ao sistema de arquivos:** detectar o tipo com `statfs`.
  - ext4, xfs, btrfs: `set_len(total)` + `fallocate` em segundo plano.
  - FAT/exFAT/NTFS-3G e outros: estratégia que não trava (sem `fallocate`), sempre fora do loop principal.
  - A latência da API (p99) deve continuar abaixo de 50 ms durante a alocação.
- Qualquer reinício do zero chama `set_len(0)` antes (evita o bug #303 do axel).
- Verificar espaço livre antes de começar (`statvfs`) e falhar cedo com `NeedsUser(DiskFull)`.
- Resolver todos os caminhos para absolutos no daemon (a partir de `XDG_DOWNLOAD_DIR` ou da configuração). Recusar escrita fora de `allowed_dirs` e recusar caminhos com `..` ou links simbólicos que escapem do diretório permitido. Isso evita o bug do Surge de salvar em `/` e a classe de ataque `dir=/etc/cron.d`.
- Sanitizar nomes vindos do servidor (`Content-Disposition`): sem barras, sem caracteres de controle e com tamanho máximo.

### 7.5 Journal persistente (anti-corrupção)

- SQLite em modo WAL (`rusqlite`), escrito por uma única tarefa (ator do store).
- Tabelas mínimas:
  - `download(id, url_original, url_final, final_path, part_path, size, etag, last_modified, digest_algo, digest_expected, content_type, state, reason, created_at, updated_at)`
  - `segment(download_id, start, end, written, state)`
  - `source(download_id, url, ip, status, measured_speed, quarantined)`
- **Ordem obrigatória:** `pwrite` dos dados → `fdatasync` periódico (a cada 8–32 MiB ou 2 s, o que vier antes) → atualizar `written` no journal. Na retomada, confiar no menor dos dois valores e, quando houver hash por segmento, revalidar o final do trecho.
- Cookies, tokens e cabeçalhos de autorização **nunca** vão para o journal.
- O journal é limpo na mesma transação que marca o download como concluído.

### 7.6 Controlador adaptativo de conexões (o carro-chefe)

Objetivo: dispensar `-x`/`-s`/`-k`. O programa descobre sozinho quantas conexões ajudam.

- Começar com **1 conexão** (a da sondagem) e medir a vazão em janelas de **500 ms**, suavizada por média móvel exponencial.
- **Aumentar em lote** (+1, depois +2, depois +4) enquanto o **ganho marginal da última adição for maior que 10%** e o RTT p50 não subir mais de 50% sobre a linha de base.
- **Estacionar** (fechar) uma conexão que, depois de 2 janelas, esteja mais de 15% abaixo da melhor e não contribua.
- **Reduzir pela metade** diante de 429/503, de RSTs em série ou de salto de RTT, e respeitar `Retry-After`.
- **Teto:** `min(teto_por_host, teto_global)`, com padrão de 8 por host em modo educado e 16 global. Opt-in por host permite até 64. Nunca acima disso.
- **Tamanho de segmento dinâmico:** alvo de "vazão da conexão × 4 s", entre 1 MiB e 64 MiB. Nada de 20 MB fixos sabotando arquivos médios.
- **Histerese** e janelas mínimas para não oscilar.
- `--connections fixed:N` desliga o controlador para quem precisa de previsibilidade em scripts.
- Toda decisão gera um evento de diagnóstico ("abri mais 2 conexões: +35% de vazão", "parei em 6: a sétima não ajudou").
- **Metas mensuráveis no faultbox:** com 2 MiB/s por conexão e 16 MiB/s no total, estabilizar em cerca de 8 conexões e chegar a 90% do platô em **3 s ou menos**; sem limite por conexão, ficar em 1 ou 2.

### 7.7 Work stealing e watchdog de travamento

- **Work stealing:** quando uma conexão fica ociosa e ainda há trabalho, o segmento restante mais lento é dividido ao meio e a conexão ociosa pega a metade final. Isso é essencial no fim do download, quando a última conexão lenta segura tudo.
- **Watchdog de stall:** um segmento sem receber bytes por T segundos (padrão 20 s) é cancelado e reatribuído, mesmo que a conexão não tenha caído (servidor que congela sem enviar RST). A interface mostra o motivo.
- Timeouts de **leitura por segmento**, não só de conexão.

### 7.8 Erros e novas tentativas

- **Taxonomia explícita:**
  - transitório: timeout, EOF prematuro, RST, 5xx, 429 com `Retry-After`;
  - terminal: 404, 410, Range inválido sem alternativa;
  - de autenticação: 401, 403, 400 com corpo XML `ExpiredToken`/`AccessDenied` do S3;
  - de recurso alterado: 200 no lugar de 206, ETag diferente;
  - local: disco cheio, permissão.
- Transitórios usam **backoff exponencial com jitter**, sem limite total enquanto houver progresso (com limite de tentativas sem progresso).
- `Retry-After` (segundos ou data HTTP) é respeitado, com teto configurável (padrão 10 min); durante a espera o estado é `PoliteWait`.
- Mensagens de erro sempre em linguagem humana, com o código técnico disponível em "Detalhes".

### 7.9 Verificação de integridade

- Fluxo obrigatório: `Downloading → Verifying → Complete(nível)`.
- Verificações em ordem:
  1. tamanho exato igual ao `Content-Length`/total;
  2. validador inalterado (ETag/`Last-Modified`) durante todo o download;
  3. hash, quando houver hash esperado (do usuário, do servidor ou do Metalink).
- **Hash incremental** durante o download sempre que os dados chegarem em ordem por segmento. Na verificação final, completar o que faltar em `spawn_blocking`. A verificação mostra progresso determinado ("Verificando integridade… 43%").
- **Hash por segmento** (Metalink com hashes de peças, ou BLAKE3 interno para a retomada) permite baixar de novo **só** o trecho corrompido.
- Se a verificação falhar: identificar o trecho (se possível) e baixar de novo; se não for possível, `Failed(VerificationFailed)` com o arquivo `.part` preservado para inspeção.

### 7.10 Links que expiram e páginas de interstício

- Tratar como "credencial expirada": 400/401/403 com corpo XML de S3 (`ExpiredToken`, `AccessDenied`, `Request has expired`), parâmetros `X-Amz-Expires`/`Expires`/`se=` na URL.
- Estado `NeedsUser(UrlExpired)`: "O link expirou; o progresso (72%) está salvo."
- **Renovação**, em ordem:
  1. pedir à extensão do navegador uma URL nova da mesma página de origem;
  2. comando externo configurável (`refresh_cmd`);
  3. RPC `download.changeSource` (colar um link novo na GUI, TUI ou CLI).
- Ao receber a URL nova, refazer a sondagem e o **contrato de identidade**: se tamanho e validador baterem, continuar de onde parou; se não, `ResourceChanged` e perguntar se deve recomeçar.

### 7.11 Política de cortesia ("modo educado", ligado por padrão)

- Teto de conexões por host (padrão 8) e global (padrão 16).
- Respeito a `Retry-After`; backoff diante de 429/503.
- User-Agent honesto: `Rajada/<versão> (+<url do projeto>)`, configurável por download só quando o usuário pedir explicitamente (a extensão pode repassar o UA do navegador para downloads que dependem de sessão).
- Lista de domínios com regras: "só 1 conexão aqui", "não acelerar", "pode até N".
- Nunca: quebrar CAPTCHA, trocar de IP para fugir de limites, falsificar cabeçalhos para enganar bloqueios.

### 7.12 Múltiplas fontes: espelhos e IPs (fase 2)

- Resolver todos os A/AAAA, com *happy eyeballs* IPv4/IPv6.
- Ler Metalink (RFC 5854) e `Link: rel=duplicate` (RFC 6249).
- Medir cada fonte (RTT e vazão inicial) e distribuir os segmentos proporcionalmente à vazão observada, reavaliando periodicamente.
- Contrato de identidade obrigatório para cada fonte; quarentena para divergentes.

### 7.13 Suspensão do notebook e troca de rede

- Escutar o sinal `PrepareForSleep` do logind (D-Bus, via `zbus`): pausar com ordem, fazer `fdatasync` e gravar o journal antes de dormir.
- Escutar `StateChanged` do NetworkManager: ao voltar a conectividade, retomar com nova resolução de DNS, porque o nó de CDN pode mudar.
- Se não houver logind ou NetworkManager, cair para detecção por timeouts.

### 7.14 Medição de velocidade e ETA

- Velocidade medida em **bytes recebidos do socket**, não a partir do progresso do arquivo (evita o bug de velocidade inflada do Gopeed).
- Média móvel exponencial (α ≈ 0,3, amostras de 1 s) para exibição.
- ETA calculado a partir da média, com histerese: só atualiza se mudar mais de 10% ou a cada 5 s. Arredondado ("~1 min", "~20 s").
- O progresso exibido é **monotônico** (nunca recua na interface).

### 7.15 Doctor (diagnóstico explicável)

Gera diagnósticos **estruturados** (código + parâmetros + severidade + recomendação), que os clientes traduzem. Exemplos de códigos e textos:

| Código | Texto (pt-BR) |
|---|---|
| `ServerPerConnectionLimit { per_conn, conns, total }` | "O servidor limita cerca de 2 MiB/s por conexão; com 8 conexões chegamos a 15,8 MiB/s." |
| `LinkSaturated { link_est }` | "Seu link é o gargalo: o servidor entregaria mais. Mais conexões não ajudariam." |
| `ServerAskedToWait { secs }` | "O servidor pediu para esperar 30 s (Retry-After). Reduzimos para 4 conexões." |
| `HtmlInsteadOfFile { size }` | "O site devolveu uma página (38 KB) em vez do arquivo. Provavelmente precisa de login. Reenvie pela extensão." |
| `RangeNotSupported` | "O servidor não permite baixar em partes; usando uma conexão. A retomada não será possível se a conexão cair." |
| `BdpExceedsRmem { bdp, rmem_max }` | "O atraso até o servidor é alto e o buffer do seu sistema limita a velocidade. Sugestão (requer administrador): `sudo sysctl -w net.ipv4.tcp_rmem='...'`" |
| `MirrorQuarantined { url, why }` | "Um espelho entregou um arquivo diferente e foi ignorado." |
| `StalledConnectionReassigned` | "Uma conexão travou e o trecho foi passado para outra." |

- Comando `rajada doctor URL`: faz uma sondagem curta, mede, e explica o que limita a velocidade **antes** de baixar.
- Cada diagnóstico tem: ícone/severidade, uma frase curta, a evidência numérica e **no máximo uma** recomendação.
- O mesmo texto aparece na CLI, na TUI e na GUI. O relatório pode ser copiado (para colar em issues).
- O doctor **nunca** altera configurações do sistema; só sugere, mostrando o comando.

---

## 8. Daemon (`rajadad`) e API

### 8.1 Execução

- Serviço do usuário via systemd (`rajadad.service` + `rajadad.socket` para ativação por socket). Assim o daemon sobe sozinho quando a CLI, a GUI ou o native host conectam.
- Fallback para distros sem systemd: o cliente inicia o daemon diretamente.
- Socket em `$XDG_RUNTIME_DIR/rajada/rajada.sock` com permissão 0600.
- Configuração em `$XDG_CONFIG_HOME/rajada/config.toml`; dados (journal) em `$XDG_STATE_HOME/rajada/` ou `$XDG_DATA_HOME/rajada/`.

### 8.2 API nativa

- JSON-RPC 2.0 sobre o socket Unix (mensagens delimitadas por linha ou com prefixo de tamanho; escolha e documente em `docs/API.md`).
- **Versionada** (`api_version`), com tipos definidos em `rajada-types` e compartilhados por todos os clientes.
- Métodos mínimos:
  - `download.add { url, dir?, filename?, headers?, cookies?, referer?, user_agent?, expected_hash?, limit?, connections? }`
  - `download.pause`, `download.resume`, `download.remove { delete_file? }`
  - `download.list { filter? }`, `download.get { id }`
  - `download.changeSource { id, url }`, `download.verify { id }`
  - `doctor.diagnose { id | url }`
  - `settings.get`, `settings.set`
  - `events.subscribe { level }`, `events.unsubscribe`
- **Assinatura de eventos por nível**, para os clientes não fazerem polling:
  - `summary@1Hz`: totais, para bandeja e mini-modo;
  - `rows@4Hz`: linhas da lista, só as que mudaram;
  - `detail(id)@4Hz`: detalhe de um download (conexões, mapa de segmentos);
  - eventos **imediatos** de mudança de estado (não esperam o próximo tique).
- O daemon **agrega e limita a frequência** antes de enviar. Clientes ocultos assinam só o `summary`.

### 8.3 Segurança da API

- Socket Unix como transporte padrão; nenhuma porta TCP aberta por padrão.
- TCP (WebSocket/HTTP) só em `127.0.0.1`, só por opt-in, com token aleatório de 256 bits gerado na primeira execução e guardado num arquivo 0600 (nunca em argv nem em variável impressa em logs).
- Conferir `Origin` e `Host` em toda requisição TCP (proteção contra páginas maliciosas e DNS rebinding).
- Recusar escutar fora do loopback sem token **e** TLS.
- `dir`/`out` sujeitos a `allowed_dirs` em todos os transportes.
- Fuzzing do parser JSON-RPC (`cargo-fuzz`).

### 8.4 Camada de compatibilidade com aria2 (fase 2, desligada por padrão)

- Adaptador isolado que implementa um subconjunto do JSON-RPC do aria2: `aria2.addUri`, `aria2.tellStatus`, `aria2.tellActive`, `aria2.tellWaiting`, `aria2.tellStopped`, `aria2.pause`, `aria2.unpause`, `aria2.remove`, `aria2.changeUri`, `aria2.getGlobalStat`, `system.listMethods` e as notificações `aria2.onDownload*`.
- Objetivo: AriaNg e outras interfaces existentes funcionarem como clientes.
- Mesmo nesse modo: token obrigatório, só loopback, `allowed_dirs` valendo. A API nativa continua sendo a principal; o adaptador traduz, sem engessar o design interno.

---

## 9. CLI (`rajada`)

- **MVP sem daemon:** `rajada get URL [-o arquivo] [-d diretório]` roda o motor no próprio processo.
- **Com daemon:** `rajada add`, `rajada ls`, `rajada pause`, `rajada resume`, `rajada rm`, `rajada watch`, `rajada doctor`, `rajada verify`, `rajada setup-browser`, `rajada config`.
- Opções principais:
  - `--sha256 HEX` (e outros algoritmos), `--limit 5M`, `--connections auto|fixed:N`, `--max-per-host N`
  - `--on-change ask|restart|fail`, `--require-resumable`
  - `--json` (saída estruturada para scripts), `--quiet`
- Flags familiares de curl/wget para facilitar a migração de scripts: `-o`, `-O`, `-H 'K: V'`, `-b`/`--cookie`, `-A`/`--user-agent`, `-e`/`--referer`, `-L` (seguir redirecionamentos, ligado por padrão).
- **Códigos de saída documentados:** 0 sucesso; 2 uso incorreto; 3 falha de rede após esgotar tentativas; 4 recurso mudou; 5 verificação falhou; 6 precisa de ação do usuário (link expirou, login); 7 recusado pela política (diretório, cortesia).
- Barra de progresso com velocidade, ETA, conexões em uso e a "linha humana" do doctor.
- Números com largura fixa para não "dançarem".

---

## 10. TUI (`rajada-tui`)

- `ratatui` + `crossterm`, cliente da API do daemon.
- Telas: lista (agrupada por estado, com "Precisa de você" em destaque), detalhe (mapa de segmentos em caracteres, sparkline de velocidade, conexões), painel do doctor e diálogo de adicionar.
- Atalhos iguais aos da GUI sempre que possível: `a` adicionar, `espaço` pausar/retomar, `d` remover (com desfazer), `/` buscar, `?` ajuda, `Enter` detalhe, `D` doctor.
- Respeitar `NO_COLOR` e terminais sem cor: estados sempre com símbolo + texto, nunca só cor.
- Redesenhar só quando chegar evento; nada de laço de 60 fps.

---

## 11. Integração com o navegador: extensão + native host

### 11.1 Como funciona

1. A extensão (Manifest V3, Chrome/Chromium/Brave/Edge e Firefox) percebe quando um download começa.
   - Chromium: `chrome.downloads.onDeterminingFilename` (permite decidir antes de o navegador escolher o nome).
   - Firefox: não tem `onDeterminingFilename`; usar `downloads.onCreated`, cancelar e apagar rapidamente (aceitar um pequeno desperdício inicial).
2. Ela coleta: URL, URL final, referer, user-agent, cookies daquela URL (`cookies.getAll`), nome sugerido, tipo MIME e tamanho, quando o navegador souber.
3. Envia por **native messaging** ao `rajada-host` (binário Rust pequeno), que conecta ao socket do daemon (ativando-o via systemd se estiver parado).
4. Se o daemon aceitar, a extensão cancela o download do navegador e o remove da lista (`downloads.cancel` + `downloads.erase`).
5. Se o handoff falhar por qualquer motivo, a extensão **devolve o download ao navegador** (`downloads.download` com a URL, marcada num conjunto em memória para não ser capturada de novo). **O usuário nunca perde um download por causa da extensão.**

### 11.2 Regras de captura

- Capturar só arquivos grandes (padrão: 5 MB ou mais; configurável). Os pequenos ficam com o navegador.
- Nunca capturar `blob:`, `data:` e `filesystem:` (o daemon não consegue reproduzir).
- Lista de sites "nunca capturar" e "sempre capturar".
- A API de downloads não informa teclas modificadoras, então não prometa "Shift+clique para pular". Ofereça: um botão no popup "Pausar captura por 10 min", menu de contexto "Baixar com o Rajada" e "Baixar com o navegador".
- Se a sondagem do daemon detectar HTML no lugar do arquivo ou exigir POST, devolver ao navegador e avisar.

### 11.3 Extensão

- Permissões mínimas: `downloads`, `nativeMessaging`, `contextMenus`, `storage`, `cookies`. Permissões de host pedidas sob demanda (`optional_host_permissions`) quando necessário para ler cookies.
- **Só captura e handoff.** Sem baixar vídeos de sites (HLS/DASH): isso viola políticas das lojas e foi o que tirou a extensão do XDM das lojas.
- Publicar na Chrome Web Store e no addons.mozilla.org (AMO). Instalação manual em modo desenvolvedor não é aceitável como caminho principal.
- Popup simples: status da conexão com o daemon (✔/✖ com explicação), downloads ativos (via porta `connectNative` só enquanto o popup está aberto), botão de pausar captura e link para abrir a GUI.
- Sem telemetria e sem chamadas de rede próprias.

### 11.4 Native host (`rajada-host`)

- Protocolo do native messaging: mensagens JSON com prefixo de 4 bytes de tamanho; respeitar o limite de 1 MB por mensagem do host para o navegador.
- Valida o `origin` da extensão (IDs permitidos no manifesto do host).
- Não guarda nada em disco. Repassa cookies ao daemon só para aquele download.
- O daemon mantém cookies **só em memória**, com escopo por download, apagados (`zeroize`) ao terminar. Nunca em logs nem no journal. Se o daemon reiniciar no meio, o download vai para `NeedsUser(LoginRequired)` e pede de novo à extensão.

### 11.5 `rajada setup-browser`

- Detecta os navegadores instalados e grava os manifestos de native messaging nos lugares certos:
  - Chrome: `~/.config/google-chrome/NativeMessagingHosts/`
  - Chromium: `~/.config/chromium/NativeMessagingHosts/`
  - Brave: `~/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts/`
  - Edge: `~/.config/microsoft-edge/NativeMessagingHosts/`
  - Firefox: `~/.mozilla/native-messaging-hosts/`
- **Navegadores em Flatpak/Snap** isolam o native host. Pesquise o estado atual das soluções (manifestos em `~/.var/app/<id>/...`, `flatpak-spawn --host` com permissão adequada, ou um portal de WebExtensions se já estiver disponível) e documente a solução que funcionar, com o `doctor` explicando o que falta quando não funcionar.
- `rajada doctor --browser` testa o handshake ponta a ponta.

---

## 12. Interface gráfica (`rajada-gtk`)

### 12.1 Tecnologia: GTK4 + libadwaita (gtk-rs + Relm4)

**Escolha:** GTK4 + libadwaita, via `gtk4-rs`/`libadwaita-rs`, com o modelo de componentes do **Relm4** (estilo Elm: mensagens → estado → visão).

**Por quê:**
- De graça: modo escuro completo (inclusive diálogos), cor de destaque do sistema, alto contraste, "reduzir animações", escala fracionária no Wayland e acessibilidade via AT-SPI (o leitor de tela Orca funciona com os widgets padrão).
- As animações do libadwaita (`AdwAnimation`) **pulam sozinhas** quando o widget não está visível ou quando o sistema desliga animações.
- No Flatpak, o runtime GNOME é compartilhado, o que reduz o custo real de memória.
- A GUI é **só um cliente da API**: trocar de toolkit no futuro não afeta o motor.

**Ponto fraco:** no KDE o visual fica com cara de GNOME. Mitigação: ler cor de destaque e esquema de cores pelo portal `org.freedesktop.appearance` (o KDE também fornece `accent-color`).

**Alternativas avaliadas:**

| Opção | Veredito | Motivo |
|---|---|---|
| Slint | Plano B | Menos RAM nas medições disponíveis, mas visual próprio e menos exemplos |
| Iced / libcosmic | Observar | API ainda instável |
| egui | Não | Modo imediato incentiva redesenho contínuo; aparência de ferramenta |
| Tauri 2 / Dioxus desktop | **Não** | WebKitGTK tem problemas de desempenho no Linux; o Rayburst (sucessor do Motrix Next, em Tauri) chegou a 99–100% de CPU no WebView |
| Qt via cxx-qt | Só se o público for KDE | Ponte C++/Rust complexa |
| Flutter | Não | Runtime pesado; o Gopeed tem problemas de escala e diálogos |

As medições de RAM entre toolkits são conflitantes nas fontes públicas: **meça no nosso ambiente**, com PSS (não só RSS), em Wayland.

### 12.2 Princípios de design da interface

1. **Silencioso por padrão:** se nada mudou, nada é redesenhado. Não existe spinner infinito em lugar nenhum.
2. **Linguagem humana primeiro, detalhes técnicos depois:** "O servidor limita cada conexão a ~2 MiB/s" vem antes de "conexões: 6". "Segmentos" e "conexões" só aparecem em "Detalhes técnicos".
3. **Confiança visível:** cada arquivo concluído mostra *como* sabemos que ele está certo. "Concluído" e "Verificado" são coisas diferentes.
4. **Toda falha tem um próximo passo:** um botão ("Colar novo link", "Abrir no navegador para fazer login", "Tentar mais tarde"), nunca só um código.
5. **Nativo e adaptável:** segue destaque, modo escuro, contraste e "reduzir animações" do sistema; funciona com escala fracionária.
6. **Uma única verdade:** mesmos nomes de estado, textos e ícones da TUI e da CLI.
7. **Densidade ajustável sem perder clareza:** confortável por padrão, modo compacto opcional, mapa de segmentos opcional.

### 12.3 Telas

#### Tela 1 — Janela principal (lista)

```
┌ HeaderBar ───────────────────────────────────────────────────────────┐
│ [+ Adicionar]  [🔍 Buscar]        ↓ 34,2 MB/s · 3 ativos   [⋮ menu]  │
├ Barra lateral (AdwNavigationSplitView) ┬ Lista (GtkListView) ────────┤
│ ● Em andamento (3)               │ [ícone tipo] ubuntu-26.04.iso       │
│ ◷ Na fila (2)                    │ ▓▓▓▓▓▓▓▓░░░ 72% · 18 MB/s · ~1 min │
│ ⚠ Precisa de você (1)            │ ↳ "Rápido: 6 conexões ajudando"     │
│ ✔ Concluídos (40)                │──────────────────────────────────── │
│ ─ Categorias ─                   │ [⚠] curso.zip — Link expirou        │
│   Vídeos / Documentos / ISOs /   │   [Colar novo link] [Abrir página]  │
│   Programas / Compactados        │──────────────────────────────────── │
│                                  │ [🛡✔] driver.run — Verificado       │
│                                  │   SHA-256 conferido · [Abrir pasta] │
└──────────────────────────────────┴─────────────────────────────────────┘
```

- Agrupamento por **estado** primeiro. "Precisa de você" fica em destaque porque contém tudo que exige ação humana. Categorias inteligentes (por tipo MIME e extensão) abaixo.
- `GtkListView` virtualizada (recicla linhas; 1.000+ itens rolando liso).
- Cada linha: ícone do tipo, nome, barra, a **linha humana** vinda do doctor e no máximo duas ações contextuais.
- Busca incremental (Ctrl+F) com filtros como `estado:falhou` e `site:github.com`.
- **Estado vazio:** ilustração simples, "Cole um link (Ctrl+V) ou arraste aqui" e um botão para instalar a extensão do navegador.

#### Tela 2 — Detalhe do download (painel lateral ou página)

```
ubuntu-26.04.iso                                   [Pausar] [⋮]
Status: Baixando · 18,4 MB/s · faltam ~1 min
"Seu link é o gargalo: o servidor entregaria mais."   [Por quê?]
┌ Mapa de segmentos ──────────────────────────────────────────────┐
│ ███████████▒▒▒▒░░░░░░█████████▒▒░░░░░███████████░░░░░░░░        │
└ (cheio = baixado · listrado = em curso · vazio = falta) ───────┘
Velocidade (últimos 60 s): sparkline ▁▂▅▇▇▆▇▇
▸ Detalhes técnicos (recolhido): conexões ativas, velocidade por conexão,
  ETag, servidor/IP, Retry-After, caminho, hash esperado e obtido
```

- O mapa de segmentos usa **padrão + cor** (cheio, listrado, vazio), então funciona sem depender de cor. É **um único widget customizado** que desenha via `snapshot`, não N widgets. Tem descrição textual acessível ("6 de 8 trechos concluídos").
- Sparkline com 60 amostras a 1 Hz, redesenhada sem animação.

#### Tela 3 — Adicionar download

- **Entradas:** Ctrl+V na janela já abre o diálogo com o link preenchido; arrastar e soltar; extensão do navegador; detecção da área de transferência **opcional** (desligada por padrão, oferecida no onboarding com explicação de privacidade).
- **Layout:** um campo de URL. A sondagem roda automaticamente em segundo plano e mostra nome, tamanho, tipo e se aceita retomada. Depois, "Salvar em: Downloads/ISOs (pela categoria)" e o botão principal **Baixar**.
- "Opções" (recolhido): limite de velocidade, agendamento, hash esperado (colar SHA-256), nome do arquivo.
- Se a sondagem detectar HTML no lugar do arquivo, o aviso aparece **antes** de baixar: "Este link abre uma página, não um arquivo. Provavelmente precisa de login. [Abrir no navegador]".
- Vários links colados de uma vez viram um lote, com revisão antes de adicionar.

#### Tela 4 — Configurações (divulgação progressiva)

- **Geral:** pasta padrão, categorias, iniciar com o sistema, comportamento ao fechar a janela.
- **Rede:** "Velocidade: Automática (recomendado)", "Ser educado com servidores (recomendado)", limite global opcional, horários.
- **Navegador:** status da extensão por navegador, tamanho mínimo para captura, sites ignorados.
- **Avançado** (`AdwExpanderRow`, recolhido): máximo de conexões, tamanho mínimo de segmento, proxy/SOCKS5, compatibilidade com aria2 (com aviso de segurança e token), diretórios permitidos.
- Cada opção avançada tem uma frase de contexto, por exemplo: "Mais conexões raramente ajudam; o modo automático já testa isso."

#### Tela 5 — Onboarding (3 passos, pulável)

1. **"Pronto para baixar":** mostra o atalho Ctrl+V e o arrastar e soltar.
2. **"Instale a extensão":** detecta Firefox/Chrome/Brave e mostra o status do native messaging com ✔/✖ e um botão para corrigir.
3. **"Continuar baixando em segundo plano?":** pede permissão pelo portal Background e explica onde o app aparece (bandeja no KDE; "Apps em segundo plano" nas Configurações rápidas do GNOME).

#### Tela 6 — Bandeja, segundo plano e mini-modo

- **Não depender de bandeja.** O GNOME padrão não tem bandeja, e o Flathub não aceita apps que funcionam só como ícone de bandeja: a GUI precisa de uma janela principal real.
- **KDE e outros:** ícone StatusNotifierItem (`ksni`) com progresso agregado; menu com Pausar tudo, Retomar tudo, Abrir e o último concluído.
- **GNOME:** portal Background (o app aparece em "Apps em segundo plano"); ícone de bandeja só para quem tem a extensão AppIndicator.
- Sem host de bandeja, fechar a janela pergunta: "Continuar baixando em segundo plano?" (com "Lembrar minha escolha").
- **Fechar a GUI nunca para os downloads:** quem baixa é o daemon.
- **Mini-modo:** janela pequena (~360×120), opcionalmente sempre visível, com a barra agregada, a velocidade e a quantidade de ativos. Ao terminar, mostra o cartão "Concluído e verificado", de onde dá para **arrastar o arquivo** direto para outro app (recurso pedido por usuários vindos do IDM).

#### Tela 7 — Notificações

- Pelo portal de notificações (`GNotification`/`ashpd`), **só** para eventos que pedem atenção ou encerram algo:
  - concluído e verificado (ações: Abrir, Mostrar na pasta);
  - falhou (ação: Ver motivo);
  - link expirou (ação: Colar novo link).
- Agrupar ("3 downloads concluídos"). **Nunca** notificar progresso.

#### Tela 8 — Painel do doctor

```
Por que está lento?                                    [Copiar relatório]
✔ Sua conexão: ~120 Mbit/s disponível
⚠ Servidor: limita ~2 MiB/s por conexão → estamos usando 6 conexões
ℹ Modo educado: o servidor pediu para esperar 30 s (Retry-After)
→ O que fazer: nada; já estamos no máximo que o servidor permite.
```

- Linhas com ícone, frase curta, evidência numérica e **uma** recomendação. O mesmo texto da CLI e da TUI.

#### Estados especiais

- **Verificação:** a barra vira "Verificando integridade… 43%" (determinada). Ao terminar, aparece o selo 🛡✔ "Verificado". Um popover mostra tamanho, algoritmo, hash (truncado no meio, `3f9a…c21e`, copiável) e a **origem** do hash esperado (servidor, Metalink, usuário ou "nenhum: só tamanho e consistência").
- **Link expirou:** linha âmbar, "O link expirou; o progresso (72%) está salvo", botões [Colar novo link] e [Abrir página de origem]. Ao colar, o daemon confere tamanho e validador e retoma; se o recurso mudou: "O arquivo no servidor mudou; recomeçar do zero?".
- **Modo educado:** chip discreto "Educado · aguardando 30 s" com ícone de ampulheta e contagem regressiva em texto, atualizada a 1 Hz, sem animação.

### 12.4 Identidade visual e tokens de cor

**Cor da marca: ultravioleta `#5B3FD9`.** Foge do azul de FDM, AB DM e IDM e não colide com nenhuma cor de estado. Opção "Usar cor do sistema" lê `accent-color` do portal.

| Token | Claro | Escuro | Contraste aproximado (WCAG) |
|---|---|---|---|
| `bg-window` | `#F6F6F7` | `#1E1E22` | n/a |
| `surface` (lista/visão) | `#FFFFFF` | `#26262B` | n/a |
| `surface-elevated` (popover/diálogo) | `#FFFFFF` + sombra | `#303036` | n/a |
| `border-subtle` (separadores decorativos) | `#E0E0E4` | `#34343A` | decorativo |
| `border-strong` (contorno de controles) | `#8A8A93` | `#74747E` | 3,45:1 / 3,26:1 |
| `text-primary` | `#1D1D20` | `#F2F2F5` | 16,9:1 / 13,5:1 |
| `text-secondary` | `#5C5C66` | `#A8A8B3` | 6,6:1 / 6,4:1 |
| `accent-bg` (botão preenchido, texto branco) | `#5B3FD9` | `#5B3FD9` | branco sobre ele: 6,65:1 |
| `accent` (texto/ícone isolado) | `#5B3FD9` | `#A996FF` | 6,65:1 / 6,1:1 |

- No GTK, mapear para as variáveis CSS do libadwaita (`--accent-bg-color`, `--accent-color`, `--success-*`, `--warning-*`, `--error-*`, `--window-bg-color`, `--view-bg-color` etc.) num único `style.css`. Não assuma que o destaque é azul.
- **Alto contraste** (portal `contrast=1`): `border-subtle` vira `border-strong` e os ícones ficam mais pesados.
- Os contrastes foram calculados manualmente: **valide com uma ferramenta** (por exemplo, o app Contrast do GNOME) antes de congelar os tokens.

### 12.5 Mapeamento de estados (cor + ícone + forma + texto)

| Estado | Cor clara | Cor escura | Ícone (simbólico) | Forma da barra | Rótulo |
|---|---|---|---|---|---|
| Queued | `#5C5C66` | `#A8A8B3` | relógio ◷ | vazia tracejada | "Na fila" |
| Probing | `#5C5C66` | `#A8A8B3` | lupa | sem barra; "Analisando link…" | "Analisando" |
| Downloading | `#5B3FD9` | `#A996FF` | seta ↓ | sólida | "Baixando" |
| Paused | `#5A6478` | `#9AA4B8` | ‖ pausa | sólida dessaturada | "Pausado" |
| PoliteWait | `#1D4ED8` | `#60A5FA` | ampulheta | sólida + chip | "Aguardando servidor" |
| Verifying | `#0E7490` | `#22D3EE` | escudo com relógio | listrada diagonal (estática) | "Verificando" |
| Complete (verificado) | `#15803D` | `#4ADE80` | escudo com ✔ | cheia + selo | "Verificado" |
| NeedsUser | `#B45309` | `#FBBF24` | elo partido / triângulo ⚠ | congelada com borda | "Precisa de você" |
| Failed | `#C01C28` | `#FF7B85` | ✖ em círculo | com marca de corte | "Falhou: motivo" |

- Todas passam AA para texto (≥ 4,5:1) e ≥ 3:1 para ícones e barras (WCAG 1.4.3 e 1.4.11). **Nunca só cor** (WCAG 1.4.1): verde e vermelho se distinguem também por ícone, forma e texto.

### 12.6 Tipografia, espaçamento, raios e elevação

- Fonte do sistema (Adwaita Sans/Cantarell no GNOME, Noto Sans no KDE), com as classes do libadwaita: `.title-1`…`.title-4`, `.heading`, `.body`, `.caption`.
- **Números tabulares** (classe `.numeric` do libadwaita ou `font-feature-settings: "tnum"`) em velocidade, porcentagem e ETA, para o texto não "dançar". Detalhe barato que passa sensação de qualidade.
- Hashes em `.monospace`, truncados no meio e copiáveis.
- Grade de **6 px**: 6 / 12 / 18 / 24 / 36. Linhas da lista com 56 px (confortável) e 40 px (compacto).
- Raios: 6 px (botões, chips internos), 12 px (cards, popovers), totalmente arredondado (chips de estado).
- Elevação em três níveis (plano, card, popover/diálogo) com as sombras do libadwaita; no escuro, elevação por superfície mais clara (`#26262B` → `#303036`), não por sombra.

### 12.7 Animações

Referências: libadwaita (`AdwTimedAnimation` com `ease-out-cubic`, `AdwSpringAnimation`), faixas de duração do Material 3 (curtas de 50–200 ms, médias de 250–400 ms) e do Fluent (83/167/250 ms). São faixas de referência, não regras.

| Elemento | Gatilho | Duração | Curva | Com "reduzir animações" |
|---|---|---|---|---|
| Nova linha na lista | download adicionado | 200 ms (altura + fade) | ease-out-cubic | aparece instantaneamente |
| Mudança de grupo/reordenação | mudança de estado | 250 ms | ease-out-cubic | troca instantânea |
| Barra de progresso | evento do daemon (2–4 Hz) | interpolação linear até o próximo valor, ≤ 10 Hz | linear, sem overshoot, **nunca recua** | salto direto |
| Velocidade e ETA | 2 Hz (velocidade), 1 Hz (ETA) | sem animação; só troca o texto | n/a | igual |
| Célula do mapa de segmentos | trecho concluído | 150 ms de fade só na célula nova | ease-out | instantâneo |
| Verificando → Verificado | hash conferido | 250 ms: barra vira selo (escala 0,9→1 + fade) | desaceleração enfática (`cubic-bezier(0.05, 0.7, 0.1, 1)`) | troca instantânea de ícone e texto |
| Falha / Precisa de você | erro | 200 ms de cor de fundo + ícone (**sem tremer**) | ease-out | instantâneo |
| Abrir detalhe/diálogo | clique | padrão do `AdwNavigationView`/`AdwDialog` | padrão do libadwaita | fade simples |
| Sparkline | nova amostra a 1 Hz | redesenho sem animação | n/a | igual |
| Probing | sondagem | spinner por no máximo 3 s; depois texto estático "Ainda analisando…" | n/a | texto estático |

- Respeitar `gtk-enable-animations` e o `reduced-motion` do portal. **Nenhuma informação depende de animação.**
- **Regra de ouro: nenhuma animação existe na árvore de widgets se não estiver visível e ativa.**

### 12.8 Orçamento de desempenho da GUI

| Métrica | Meta | Como garantir |
|---|---|---|
| CPU com a janela aberta e nada ativo | ~0% (zero despertares da GUI) | Sem timers; a GUI só reage a eventos da API; sem spinners |
| CPU com downloads ativos e janela visível | < 2% de um núcleo | Eventos a 2–4 Hz, barra a ≤ 10 Hz, só a linha que mudou é atualizada |
| CPU com janela oculta ou minimizada | ~0% | Ao esconder (`unmap`), cancelar a assinatura detalhada e assinar só o `summary@1Hz` |
| RAM (PSS) da GUI | meta ≤ 80 MB; alerta > 120 MB | Medir PSS; a GUI pode fechar inteira e o daemon continua |
| Tempo até a primeira janela | < 300 ms a quente | Binário nativo, sem WebView |
| Taxas | velocidade 2 Hz · ETA 1 Hz · barra ≤ 10 Hz · mapa ≤ 4 Hz · sparkline 1 Hz · bandeja 1 Hz | O daemon agrega antes de enviar |

Motivação: o Rayburst (Tauri) foi de 3% para 38% de CPU com a janela oculta até parar de enviar atualizações ocultas e animar no compositor (depois ficou abaixo de 1%). Outros apps Electron mantiveram 27–55% de CPU por causa de um spinner preso ou oculto. **O custo vem das atualizações e animações, não do download.** Projete contra isso desde o primeiro dia e **teste**: medir despertares e uso de CPU da GUI parada por 60 s (`pidstat`, `/proc/<pid>/status`, `powertop`) e falhar o CI se houver regressão.

### 12.9 Acessibilidade (checklist obrigatório)

- [ ] Todo estado tem **texto + ícone + forma**, nunca só cor.
- [ ] Texto com contraste ≥ 4,5:1; ícones, bordas de controle e barras ≥ 3:1, nos dois temas e no alto contraste.
- [ ] Respeita "reduzir animações"; nada depende de animação.
- [ ] Fluxo inteiro pelo teclado: Ctrl+V (adicionar link colado), Ctrl+N (novo), Espaço (pausar/retomar), Delete (remover com desfazer), Ctrl+F (buscar), Ctrl+K (paleta de comandos), Ctrl+, (preferências), Ctrl+? (atalhos, via janela/diálogo de atalhos do GTK/libadwaita).
- [ ] Foco visível com contraste ≥ 3:1.
- [ ] Barras com rótulo acessível ("ubuntu.iso, 72%, 18 megabytes por segundo, falta cerca de 1 minuto"). Anúncios para leitor de tela **só em mudança de estado**, não a cada atualização.
- [ ] Mapa de segmentos com descrição textual equivalente.
- [ ] Texto escalável até 200% sem cortar; testar em escala fracionária de 125%, 150% e 200% no Wayland.
- [ ] Alvos clicáveis ≥ 24×24 px (WCAG 2.2, 2.5.8).
- [ ] Ações destrutivas com "Desfazer" (toast do `AdwToastOverlay`) em vez de diálogo de confirmação.
- [ ] Auditoria com o Orca e navegação só por teclado antes de cada release.

### 12.10 Diferenciais da interface (o que nos separa dos projetos menos intuitivos)

1. **"Verificado" como estado de primeira classe:** selo, hash copiável e origem do hash. Nenhum concorrente mostra isso como prova.
2. **Doctor como linha de status:** cada download diz *por que* está como está, em português claro.
3. **Recuperação com um clique:** "Colar novo link" mantém o progresso; HTML no lugar do arquivo é detectado antes de baixar.
4. **Zero jargão por padrão:** velocidade "Automática"; conexões e segmentos só em Avançado. O oposto dos padrões confusos do JDownloader e dos `-x/-s/-k` do aria2.
5. **Leveza comprovada:** GUI fecha sem parar o daemon, ~0% de CPU parada, números publicados no README.
6. **Integração real com o Linux:** tema, destaque, contraste e animações do sistema; escala fracionária; portal Background no GNOME e SNI no KDE.
7. **O que os usuários do IDM adoram, sem o peso:** mapa de segmentos opcional e mini-janela com arrastar e soltar do arquivo concluído.
8. **Grupo "Precisa de você":** tudo que exige ação humana num só lugar.
9. **Paleta de comandos (Ctrl+K):** qualquer ação por teclado, sem caçar menus.

### 12.11 Notas de implementação para a GUI

- A GUI pode ser **prototipada contra um daemon falso** (mock) que emite eventos simulados, para avançar em paralelo ao motor.
- Componentes Relm4 pequenos e testáveis: `DownloadRow` recebe um `RowSnapshot` e **só atualiza os rótulos que mudaram**; `SegmentMap` é um widget customizado com `snapshot`; `DoctorPanel` só renderiza códigos de diagnóstico traduzidos.
- Tokens num único `style.css` usando as variáveis do libadwaita.
- Metadados corretos: app ID (`io.github.SEU_USUARIO.Rajada`, a definir), arquivo `.desktop`, ícone em SVG, `metainfo.xml` com capturas nos temas claro e escuro.

---

## 13. Testes e benchmarks

### 13.1 `faultbox`: servidor de teste com injeção de falhas (construir na Fase 0)

Binário Rust (axum/hyper) com cenários configuráveis por query string ou arquivo TOML. É o laboratório do projeto e a base de todos os testes de integração.

| Cenário | Simula | Prova qual correção |
|---|---|---|
| `per_conn_rate=2MiB&total=16MiB` | Limite por conexão + teto total | Controlador adaptativo para em cerca de 8 conexões |
| `no_accept_ranges_but_206` | Servidor que não anuncia Range mas suporta | Sondagem dupla (Gopeed #1652) |
| `ignore_range` (200 + corpo inteiro) | Servidor que mente | Detecção sem corromper |
| `no_content_length` | Stream ou arquivo gerado na hora | Uma conexão, estado "sem verificação de tamanho" |
| `etag_change_at=50%` | Arquivo atualizado no meio | `If-Range` → `ResourceChanged` |
| `expire_after=60s&status=400&body=ExpiredToken` | URL assinada do S3 | Renovação de link |
| `eof_every=1GiB` | EOF prematuro | Retry transitório (aria2 #2408) |
| `drop_conn=1of8` | Uma conexão cai no meio do corpo | Re-download do segmento, hash correto (aria2 #886) |
| `stall_conn=30s` (sem RST) | Conexão congelada | Watchdog + work stealing |
| `429_above=4&retry_after=30` | Limite de taxa | Recuo + `Retry-After` |
| `gzip_if_accepted` | Compressão em ranges | `Accept-Encoding: identity` (wget2 #279) |
| `h2_goaway_every=100` | GOAWAY do HTTP/2 | Reabertura de conexão |
| `html_interstitial` | Página de confirmação (Google Drive) | Detecção de HTML |
| `corrupt_byte_in_segment=3` | Corrupção silenciosa | Hash por segmento (lftp #523) |
| `redirect_chain=3` | Redirecionamentos | URL final registrada, original preservada |
| `require_cookie=session` | Download com login | Cookies repassados pela extensão |
| `mirror_mismatch` | Espelho com bytes diferentes do mesmo tamanho | Quarentena |
| `tc netem delay 150ms loss 1%` (script) | Link intercontinental com perda | Ganho real de múltiplas conexões |

### 13.2 Outros testes obrigatórios

- **Crash injection:** matar o daemon (`SIGKILL`) em pontos aleatórios, em 1.000 iterações; retomar e exigir hash correto. Zero arquivos finalizados com hash errado.
- **Testes de propriedade** (`proptest`) do escalonador, do particionamento de segmentos e do work stealing (nenhum byte perdido nem duplicado, cobertura total do arquivo).
- **Concorrência** com `loom` nas partes de estado compartilhado que restarem.
- **Fuzzing** do parser JSON-RPC, de `Content-Range`, `Content-Disposition`, `Retry-After` e Metalink.
- **Snapshots de texto** (`insta`) das mensagens do doctor e dos estados em pt-BR e inglês.
- **Rede instável:** blackhole de 2 min com `iptables`/`netem`; tamanho final exato e hash correto.
- **Sistemas de arquivos:** imagem FAT32 em loop device com arquivo de 20 GB; latência da API p99 < 50 ms durante a alocação.
- **Extensão:** testes ponta a ponta com Playwright em Chromium e Firefox: daemon parado, clique em download, daemon sobe e o download começa; handoff falho devolve ao navegador.
- **GUI:** teste de "nenhum timer ativo quando ocioso"; CPU e despertares da GUI parada por 60 s; 1.000 itens na lista rolando liso.

### 13.3 Benchmarks (registrar em `BENCHMARKS.md`)

- Comparar com `wget`, `curl` e `aria2c` (com padrão e bem configurado) nos cenários do faultbox, com `hyperfine`, várias repetições e mediana.
- Métricas: tempo total, tempo até 90% do platô, CPU por MB baixado, RSS/PSS do daemon parado e com 10 downloads, latência da API.
- **Metas de projeto** (são objetivos, não resultados):
  - daemon ≤ 15 MB RSS parado e ≤ 40 MB com 10 downloads;
  - CPU por MB ≤ aria2 + 20%;
  - rampa até 90% do platô ≤ 3 s no cenário de limite por conexão;
  - latência da API p99 < 50 ms sob carga.
- Regressão acima de 20% em qualquer métrica falha o CI.

---

## 14. Qualidade de código e regras do dia a dia

- `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`, `cargo deny check`, `cargo audit` no CI, em toda PR.
- Sem `unwrap()`/`expect()` fora de testes, salvo invariante documentada.
- Sem `unsafe`, salvo necessidade comprovada, isolada e comentada.
- Funções pequenas; módulos com responsabilidade única; tipos expressivos (newtypes para bytes, offsets, IDs).
- Toda mudança de comportamento vem com teste. Todo bug corrigido vem com teste de regressão (de preferência um cenário do faultbox).
- Logs com `tracing`, com níveis coerentes e **redação automática** de cookies, tokens, cabeçalhos `Authorization` e parâmetros de URL assinada.
- Decisões de arquitetura registradas em `docs/adr/` (um arquivo curto por decisão: contexto, decisão, consequências).
- Manter `BENCHMARKS.md` e `CLAUDE.md` atualizados a cada marco.

---

## 15. Empacotamento, distribuição e segurança da cadeia de suprimentos

- Formatos: binário estático (musl) para a CLI/daemon, `.deb` e `.rpm` em repositório próprio **assinado**, AUR, Flatpak (runtime GNOME) para a GUI no Flathub e `cargo install`.
- **Builds reproduzíveis**, releases assinados (por exemplo, `cosign` ou `minisign`), checksums publicados em mais de um canal e proveniência (SLSA).
- Nenhum instalador com ofertas, nenhum download direto sem assinatura verificável (lição do caso FDM).
- **Nenhuma telemetria.** Se um dia houver relatório de falhas, será opt-in explícito e local.
- `SECURITY.md` com canal de reporte e política de divulgação.
- Licença: proponha MIT/Apache-2.0 (padrão do ecossistema Rust) e **pergunte antes de decidir**; verifique a compatibilidade das dependências com `cargo-deny`.
- Integração systemd: `rajadad.service` e `rajadad.socket` para o usuário (`systemctl --user`).

---

## 16. Ética e aspectos legais

- Respeitar termos de uso: alguns serviços proíbem aceleradores ou limitam conexões por conta. O modo educado e as regras por domínio existem para isso.
- **Não burlar proteções:** nada de quebrar CAPTCHA, trocar de IP para fugir de limites, falsificar cabeçalhos para enganar bloqueios ou integrar com serviços de hospedagem para contornar limites de contas gratuitas.
- Padrão conservador: começar com poucas conexões, teto por host e recuo diante de sinais de sobrecarga.
- A ferramenta é neutra; não promovê-la para pirataria.
- Privacidade: cookies só em memória, por download; nenhum dado sai da máquina; detecção de área de transferência desligada por padrão.
- Nenhuma alteração de sysctl ou do sistema sem consentimento explícito do usuário (e o programa nunca roda como root).

---

## 17. O que NÃO fazer (anti-objetivos)

- Não prometer "X vezes mais rápido".
- Não abrir conexões à toa quando não ajudam.
- Não usar Electron, Tauri ou WebView na GUI.
- Não fazer app que funcione só pela bandeja.
- Não fazer polling nos clientes; não ter spinner infinito; não animar nada invisível.
- Não ter telemetria.
- Não baixar vídeos de sites (HLS/DASH) na extensão.
- Não implementar plugins de serviços de hospedagem, captcha ou reconexão de roteador.
- Não incluir BitTorrent nem HTTP/3 no MVP.
- Não alterar configurações do sistema sem consentimento.
- Não marcar nada como "concluído" sem verificar.
- Não gravar cookies, tokens ou URLs assinadas em disco ou em logs.

---

## 18. Roteiro por fases (com critérios de aceite)

### Fase 0 — Fundação

- Workspace Cargo com os crates da seção 5.2 (vazios ou mínimos), CI (fmt, clippy, test, deny, audit), `CLAUDE.md`, `docs/ESPECIFICACAO.md`, `docs/ARQUITETURA.md`, primeiro ADR.
- `rajada-types`: enum de estados, motivos, `VerificationLevel`, códigos de diagnóstico e esquema inicial de eventos da API (`summary@1Hz`, `rows@4Hz`, `detail(id)@4Hz`, mudanças de estado imediatas).
- `rajada-i18n` com o catálogo inicial (pt-BR e en) dos estados e diagnósticos.
- `faultbox` com os primeiros cenários: arquivo com e sem Range, limite por conexão, latência, queda de conexão, 429 com `Retry-After`, mudança de ETag.
- **Aceite:** `cargo test` verde; `faultbox` serve arquivos determinísticos com hash conhecido; tipos compartilhados compilam para todos os crates.

### Fase 1 — Motor MVP e CLI (versão 0.1)

1. Download com uma conexão, `.part`, rename atômico, journal, retomada com `If-Range`, retry com backoff e `Retry-After`, verificação final (tamanho + hash opcional).
   **Aceite:** interromper e retomar funciona; ETag alterado gera `ResourceChanged`; 1.000 iterações de crash injection sem nenhum arquivo errado.
2. Sondagem dupla de Range, segmentação com N fixo, `pwrite`, pré-alocação sensível ao sistema de arquivos.
   **Aceite:** hash idêntico ao original em todos os cenários do faultbox; benchmark contra wget e aria2 registrado.
3. Controlador adaptativo, segmentos dinâmicos, work stealing e watchdog de stall.
   **Aceite:** converge para 1–2 conexões sem limite por conexão; para cerca de 8 com limite; recua com 429; rampa ≤ 3 s.
4. Doctor v1 (limite por conexão, link saturado, 429, HTML no lugar do arquivo, sem Range) e polimento da CLI (`--limit`, `--sha256`, `--json`, códigos de saída).
5. Publicar binário estático e AUR/.deb; coletar feedback.

### Fase 2 — Daemon, TUI, extensão e confiabilidade avançada (versões 0.2–0.5)

- Daemon com API nativa por socket Unix, eventos por nível, fila e agendamento, serviço systemd com ativação por socket.
- TUI completa.
- `rajada-host` + extensão MV3 (Chromium e Firefox) + `rajada setup-browser` (incluindo investigação de navegadores em Flatpak) + publicação nas lojas.
- Renovação de link expirado (extensão, comando, RPC).
- Espelhos, múltiplos IPs, Metalink e `Repr-Digest`, com contrato de identidade.
- Integração com logind e NetworkManager.
- Adaptador de compatibilidade com aria2 (desligado por padrão), validado com o AriaNg real.
- Builds reproduzíveis e assinados, repositório APT/RPM assinado.
- **Aceite:** AriaNg controla o daemon com token; extensão funciona com o daemon parado (ativação); suspender o notebook no meio e acordar retoma sozinho; link expirado retoma com link novo sem perder progresso.

### Fase 3 — Interface gráfica (pode começar em paralelo, contra um daemon falso)

| Subfase | Entregas | Aceite |
|---|---|---|
| 3.1 Lista | Janela Relm4 + libadwaita, `GtkListView` agrupada por estado, diálogo de adicionar (colar/arrastar), pausar/retomar/remover com desfazer, tokens CSS, números tabulares | 1.000 itens rolando liso; CPU ~0% parada; PSS medido |
| 3.2 Confiança | Painel de detalhe, verificação + selo, painel do doctor, fluxos de link expirado e recurso alterado, notificações via portal | Todo `Failed`/`NeedsUser` tem ação de recuperação; snapshots dos textos |
| 3.3 Presença | Portal Background, SNI via `ksni`, mini-modo, onboarding de 3 passos, status da extensão | Funciona no GNOME sem extensão de bandeja e no KDE com bandeja |
| 3.4 Polimento | Mapa de segmentos, sparkline, animações da seção 12.7, paleta de comandos (Ctrl+K), modo compacto, alto contraste | Auditoria com Orca e só teclado; teste com "reduzir animações" |
| 3.5 Distribuição | Flatpak (runtime GNOME), metainfo com capturas claro/escuro, benchmark de RAM/CPU publicado contra concorrentes | Pronto para o Flathub; números reproduzíveis no README |

### Fase 4 — Avançado (cada item é um projeto separado, com benchmark próprio)

- HTTP/3 opt-in, ativado só quando o doctor detectar perda alta e o servidor anunciar `Alt-Svc`, com volta automática.
- SFTP/FTP segmentado.
- BitTorrent via biblioteca (por exemplo, `librqbit`).
- Doctor com sugestões de sysctl baseadas no BDP medido.
- Modo sequencial para assistir enquanto baixa.
- Sistema de plugins (por exemplo, WASM), se houver demanda real.

---

## 19. Definição de pronto (para cada tarefa)

Uma tarefa só está pronta quando:

1. o código compila sem warnings e passa `fmt`, `clippy`, `test`, `deny` e `audit`;
2. há testes cobrindo o comportamento novo, incluindo pelo menos um cenário de falha;
3. se afeta desempenho, há número no `BENCHMARKS.md`;
4. se afeta textos para o usuário, as traduções pt-BR e en existem e os snapshots foram atualizados;
5. se muda uma decisão, há um ADR e o `CLAUDE.md` foi atualizado;
6. o commit tem mensagem clara e escopo pequeno.

---

## 20. Primeira tarefa (agora)

1. Entre em modo de planejamento.
2. Leia este documento inteiro.
3. Proponha:
   - a estrutura final do workspace (ou ajustes à seção 5.2, com justificativa);
   - as versões dos crates principais que você pretende usar, conferindo a documentação atual de cada um;
   - o conteúdo do `CLAUDE.md` (menos de 200 linhas);
   - o plano detalhado da Fase 0, dividido em tarefas pequenas, com o critério de aceite de cada uma;
   - os riscos que você enxerga e as perguntas que precisa que eu responda (nome, licença, app ID, etc.).
4. **Espere minha aprovação** antes de criar arquivos ou escrever código.
