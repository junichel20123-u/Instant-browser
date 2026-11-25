# API nativa — contrato v1 (`api_version = 1`)

Os tipos ficam em `crates/idb-types/src/api.rs` e são a **fonte de verdade**. Snapshots em
`crates/idb-types/tests/snapshots/api__*.snap` travam o formato na rede: uma mudança nele quebra o teste de propósito.

## Um protocolo, três transportes
| Transporte | Quem usa | Quando |
|---|---|---|
| Socket Unix `$XDG_RUNTIME_DIR/idb/idb.sock` (0600), JSON delimitado por linha | CLI, TUI, GUI, native host | Fase 2 |
| WebSocket + TLS na rede local, com pareamento (ADR-0007) | App de celular como controle remoto | Fase 2 |
| Chamada direta (`idb-mobile`, UniFFI) | App de celular com o motor embutido | Ponte pronta (ADR-0008) |

Os transportes só levam as mensagens; nenhum inventa métodos próprios. Mensagens acima de 1 MiB fecham a conexão.

## Sessão
1. O cliente envia `session.hello {api_version, client}`.
2. O servidor responde `{api_version, server}`, ou `UnsupportedVersion` (-32002).
3. Pela rede local, o token do dispositivo vai no handshake do WebSocket, nunca em URL nem em log. Um token ausente ou
   revogado dá `Unauthorized` (-32001).

## Métodos
| Método | Parâmetros | Resultado |
|---|---|---|
| `session.hello` | `api_version`, `client` | `{api_version, server}` |
| `download.add` | `url`, `dir?`, `filename?`, `expected_sha256?`, `limit?` (bytes/s), `connections?` (`"auto"` \| `{"fixed":N}`), `referer?` | `{id}` |
| `download.pause` / `download.resume` | `id` | `{}` |
| `download.remove` | `id`, `delete_file?` | `{}` |
| `download.list` | `filter?` (`StateKind`) | `{downloads: [DownloadSummary]}` |
| `download.get` | `id` | `DownloadSummary` |
| `download.changeSource` | `id`, `url` | `{}` |
| `doctor.diagnose` | `url` | relatório do doctor (o mesmo de `idb doctor --json`) |
| `events.subscribe` / `events.unsubscribe` | `level`: `{"level":"Summary"}` \| `{"level":"Rows"}` \| `{"level":"Detail","id":N}` | `{}` |

`DownloadSummary`: `id, url, filename, state, done, total, speed, eta_secs`. Tudo já vem calculado pelo motor: os
clientes (inclusive o celular) só traduzem e exibem.

## Eventos
Notificações `{"jsonrpc":"2.0","method":"event","params":Event}`:
- `Summary` no máximo 1 Hz; `Rows` (só as linhas que mudaram) e `Detail(id)` no máximo 4 Hz;
- `StateChanged` e `Diagnostic` são enviados na hora.

No celular, o app assina só `Summary` quando está em segundo plano (economia de bateria e rede).

## Erros
`{"code":N,"message":"texto técnico em inglês","data":{"kind":"..."}}`. O texto para o usuário vem do `idb-i18n`, a
partir do `kind`.

| kind | code |
|---|---|
| ParseError / InvalidRequest / MethodNotFound / InvalidParams / Internal | -32700 / -32600 / -32601 / -32602 / -32603 |
| Unauthorized | -32001 |
| UnsupportedVersion | -32002 |
| NotFound | -32003 |
| DirNotAllowed | -32004 |

## Compatibilidade
Campos novos e opcionais podem entrar sem trocar a versão. Remover ou mudar o significado de um campo exige
`api_version = 2`, e o servidor continua aceitando a versão anterior enquanto houver clientes (incluindo apps de
celular desatualizados).
