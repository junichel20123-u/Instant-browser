# Arquitetura

Visão geral na seção 5 de `ESPECIFICACAO.md`. Este arquivo registra o que **já existe** e como as peças se ligam.

## Camadas
```
clientes (rajada, rajada-tui, rajada-gtk, rajada-host)
    │  JSON-RPC 2.0 sobre socket Unix (docs/API.md)
rajadad ── rajada-engine ── rajada-store (SQLite WAL)
    └── todos dependem de rajada-types (+ rajada-i18n nos clientes)
```

## rajada-types (Fase 0)
- `DownloadState` (máquina da §7.1) com `kind()` sem dados, `recovery()` e `is_terminal()`.
- `NeedsUserReason` e `FailReason`: cada motivo tem **exatamente uma** `RecoveryAction` (garantido por teste).
- `VerificationLevel`: `HashMatched{algo,source}`, `SizeAndValidator`, `SizeOnly`, `Unverified`.
- `DiagnosticCode` + `Severity` + `Diagnostic`: só dados; o texto vem do `rajada-i18n`.
- `Event` e `EventLevel` (`Summary@1Hz`, `Rows@4Hz`, `Detail(id)@4Hz`; `StateChanged` é imediato).
- Serialização JSON com tags explícitas (`{"state":"NeedsUser","data":"UrlExpired"}`); snapshots em `tests/snapshots`.

## rajada-i18n (Fase 0)
- Catálogos `po/pt_BR.po` e `po/en.po` embutidos com `include_str!` e lidos por um parser `.po` mínimo (ADR-0002).
- msgids estáveis (`state.Queued`, `diag.LinkSaturated`...) com marcadores `{nome}`.
- `describe_state`, `render_diagnostic`, `action_label` e formatação de bytes, velocidade e tempo por locale.
- Testes: os catálogos têm os mesmos msgids; nenhum texto vaza chave ou marcador; snapshots nos dois idiomas.

## faultbox (Fase 0)
- `GET /file?size=N&seed=S`: conteúdo determinístico e acessível por posição (splitmix64 por bloco de 8 bytes);
  `expected_sha256(seed, size)` dá o hash esperado.
- Range simples (`a-b`, `a-`, `-n`), 416 com `Content-Range: bytes */N`, ETag forte, `Last-Modified`, `If-Range`.
- Cenários por query string; o estado compartilhado (respostas em andamento, bytes servidos, versão) fica por instância
  do servidor, então cada teste chama `faultbox::spawn()` e recebe um servidor isolado numa porta efêmera.
