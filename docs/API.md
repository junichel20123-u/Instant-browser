# API nativa (rascunho, `api_version = 1`)

## Transporte
- Socket Unix em `$XDG_RUNTIME_DIR/idb/idb.sock`, permissão 0600.
- **JSON-RPC 2.0 delimitado por linha**: cada mensagem é um objeto JSON numa linha, terminada por `\n`.
  Escolhido por ser fácil de depurar (`socat`), simples de parsear em streaming e suficiente, já que o JSON
  serializado não contém quebras de linha literais. Tamanho máximo por mensagem: 1 MiB (acima disso, a conexão fecha).
- Todos os tipos ficam em `idb-types`.

## Métodos (planejados)
| Método | Parâmetros |
|---|---|
| `download.add` | `url, dir?, filename?, headers?, cookies?, referer?, user_agent?, expected_hash?, limit?, connections?` |
| `download.pause` / `download.resume` | `id` |
| `download.remove` | `id, delete_file?` |
| `download.list` | `filter?` |
| `download.get` | `id` |
| `download.changeSource` | `id, url` |
| `download.verify` | `id` |
| `doctor.diagnose` | `id` ou `url` |
| `settings.get` / `settings.set` | — |
| `events.subscribe` / `events.unsubscribe` | `level` (`EventLevel`) |

## Eventos (notificações JSON-RPC `event`)
Formato de `idb_types::Event`, por exemplo:
```json
{"jsonrpc":"2.0","method":"event","params":{"event":"StateChanged","data":{"id":7,"state":{"state":"Paused"}}}}
```
- `Summary` a no máximo 1 Hz; `Rows` (só as linhas que mudaram) e `Detail(id)` a no máximo 4 Hz.
- `StateChanged` e `Diagnostic` são enviados na hora.
