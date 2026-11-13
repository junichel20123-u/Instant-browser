# CLAUDE.md — Instant Download Browser (IDB)

O Instant Download Browser (IDB) é um gerenciador e acelerador de downloads para Linux, em Rust.
Ele descobre sozinho a forma mais rápida **e educada** de baixar, nunca entrega um arquivo errado
e explica em português claro por que um download está lento. Daemon leve + CLI, TUI, extensão e GUI GTK4.

Especificação completa (fonte de verdade): `docs/ESPECIFICACAO.md`.
Arquitetura: `docs/ARQUITETURA.md` · API: `docs/API.md` · Decisões: `docs/adr/`.

## Princípios inegociáveis (o de cima vence)
1. Nunca entregar um arquivo errado (`.part` → verificação → `rename` atômico).
2. Seguro por padrão (socket Unix 0600, sem porta TCP por padrão, sem telemetria, segredos fora de argv/logs).
3. Educado com servidores (teto por host, `Retry-After`, recuo em 429/503).
4. Honesto sobre velocidade (mede, decide, explica; nada de "até 10×").
5. Leve (sem polling, sem trabalho quando nada muda).
6. Uma verdade só (estados, códigos e textos em `idb-types` + `idb-i18n`).
7. Explicável (toda falha tem um próximo passo concreto).

## Workspace
| Crate | Papel | Estado |
|---|---|---|
| `crates/idb-types` | estados, motivos, `VerificationLevel`, diagnósticos, eventos, `API_VERSION` | Fase 0 ✅ |
| `crates/idb-i18n` | catálogo `.po` (pt-BR, en) + formatação por locale | Fase 0 ✅ |
| `crates/faultbox` | servidor HTTP de teste com injeção de falhas (lib + bin) | Fase 0 ✅ |
| `crates/idb-engine` | motor de download (biblioteca) | Fase 1.1 ✅ (1 conexão) |
| `crates/idb-store` | journal SQLite (WAL) | Fase 1.1 ✅ |
| `crates/idb-daemon` | bin `idbd` | esqueleto |
| `crates/idb-cli` | bin `idb` (`idb get`, usa o motor direto) | Fase 1.1 ✅ |
| `crates/idb-tui` | bin `idb-tui` | esqueleto |
| `crates/idb-gtk` | bin `idb-gtk` (fora de `default-members`) | esqueleto |
| `crates/idb-host` | bin `idb-host` (native messaging) | esqueleto |
| `xtask` | automação (`cargo xtask ci`) | Fase 0 ✅ |

## Comandos
```sh
cargo xtask ci                      # fmt --check + clippy -D warnings + test
cargo test -p faultbox              # testes do servidor de falhas
cargo run -p faultbox -- --port 8080
curl -r 0-99 'http://127.0.0.1:8080/file?size=1048576&seed=1'
INSTA_UPDATE=always cargo test      # regravar snapshots (revise o diff!)
IDB_CRASH_ITERS=1000 cargo test --release -p idb-cli   # aceite de crash injection
```
Cenários do faultbox: `no_ranges`, `per_conn_rate=B`, `latency_ms=M`, `drop_conn=N`,
`429_above=N&retry_after=S`, `etag_change_at=P`, `fail_first=N&status=C`, `html` (ver `crates/faultbox/src/lib.rs`).

## Invariantes do motor (não quebre)
- `journal.downloaded` só avança **depois** do `fdatasync` do `.part`; na retomada, offset = min(tamanho do `.part`, journal).
- Retomada sempre com `If-Range` (ETag forte, senão `Last-Modified`). 200 com validador diferente → `NeedsUser(ResourceChanged)`, nunca mistura versões.
- Destino gravado no journal **antes** do `rename`; `get` repetido com o arquivo final presente → verifica de novo, não duplica (ADR-0003).

## Regras de código
- Código, identificadores, comentários e commits em **inglês**. Textos para o usuário em pt-BR **e** en,
  sempre via `idb-i18n` (nunca strings soltas nos clientes).
- Sem `unwrap()`/`expect()` fora de testes (lint `deny` no workspace); sem `unsafe` (`forbid`).
- `thiserror` nas bibliotecas, `anyhow` só nos binários.
- Newtypes para bytes, offsets e IDs (`ByteCount`, `ByteOffset`, `DownloadId`).
- Modelo de atores: cada download é uma tarefa tokio dona do seu estado; canais com limite.
- Toda mudança de comportamento vem com teste; todo bug vem com teste de regressão (de preferência um cenário do faultbox).
- Nenhuma afirmação de desempenho sem número em `BENCHMARKS.md`.
- Logs com `tracing`, com redação de cookies, tokens, `Authorization` e URLs assinadas.

## Pergunte antes de
adicionar dependência pesada ou com licença incomum · mudar uma decisão de arquitetura · publicar qualquer coisa ·
escolher nome/licença definitivos · rodar comandos como root ou que alterem o sistema.

## Definição de pronto
1. `fmt`, `clippy`, `test`, `deny` e `audit` passam, sem warnings.
2. Testes cobrem o comportamento novo, incluindo pelo menos um cenário de falha.
3. Se afeta desempenho: número em `BENCHMARKS.md`.
4. Se afeta textos: pt-BR + en + snapshots atualizados.
5. Se muda uma decisão: ADR + este arquivo atualizado.
6. Commit pequeno, com mensagem clara.

## Decisões provisórias (pendentes de confirmação)
- Licença `MIT OR Apache-2.0`; edição 2024. (Nome definido: Instant Download Browser — ADR-0004.)
- i18n: `.po` parseado em Rust puro em vez de `gettext-rs` (ADR-0002).
- `Retry-After` só em segundos (data HTTP ainda não é interpretada; usa backoff).
