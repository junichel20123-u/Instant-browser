# ADR-0002: Catálogo `.po` lido em Rust puro (em vez do `gettext-rs`)

- **Status:** aceito provisoriamente (Fase 0), sujeito a revisão.
- **Contexto:** a especificação pede gettext. O `gettext-rs` 0.8 depende de libintl/msgfmt do sistema ou compila o
  gettext do C, o que torna o build e o CI mais lentos e complica o build estático (musl). Na Fase 0 só precisamos de
  um catálogo pequeno, compartilhado por todos os clientes.
- **Decisão:** manter os catálogos no formato gettext `.po` (`crates/idb-i18n/po/`), embutidos com `include_str!` e
  lidos por um parser mínimo em Rust (strings simples, continuações e escapes `\n \t \" \\`). As chaves são msgids
  estáveis (`state.Queued`) e os parâmetros usam marcadores `{nome}`.
- **Consequências:** zero dependências de sistema; os arquivos continuam editáveis por ferramentas de tradução
  (Poedit, Weblate). Ainda não há suporte a plurais (`msgid_plural`) nem a contextos (`msgctxt`): se forem
  necessários, estender o parser ou migrar para o `gettext-rs` sem mudar os arquivos `.po`.
