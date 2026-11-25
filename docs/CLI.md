# CLI `idb` — contrato

Os textos seguem o idioma do sistema (`LANG`, `LC_ALL`, `LC_MESSAGES`): pt-BR ou inglês.

## `idb get URL`
| Opção | Efeito |
|---|---|
| `-d, --dir PASTA` | Pasta de destino (padrão: `XDG_DOWNLOAD_DIR`, ou `~/Downloads`) |
| `-c, --connections auto\|N` | `auto` (padrão): o IDB mede e decide, até 8 conexões. `N` (1–8, ou `fixed:N`): número fixo |
| `--limit TAXA` | Teto de velocidade para todas as conexões juntas: `2M`, `500k`, `1.5MiB`, `800KB/s` (múltiplos de 1024) |
| `--sha256 HEX` | Só conclui se o hash bater (64 caracteres hexadecimais) |
| `-q, --quiet` | Sem barra de progresso |
| `--json` | Uma linha JSON no stdout com o resultado (abaixo) |

Rodar o mesmo comando de novo retoma de onde parou. Se o arquivo final já existe e foi baixado do mesmo endereço, ele é
verificado de novo e o download não se repete.

## `idb doctor URL`
Faz uma sondagem curta (padrão: até 5 s, `--seconds N`), **sem gravar nada**, e explica o que limita a velocidade:
suporte a partes, se é possível retomar, limite por conexão, link saturado, servidor pedindo para esperar, página HTML
no lugar do arquivo. Aceita `-c` e `--json`.

## Códigos de saída
| Código | Significado |
|---|---|
| 0 | Concluído e verificado (`get`) / nada impede o download (`doctor`) |
| 1 | Falhou (a mensagem diz o próximo passo) |
| 2 | Precisa de você: link expirou, login, página no lugar do arquivo, arquivo mudou, disco cheio... |
| 3 | Erro interno |
| 64 | Uso incorreto (`-c` fora de 1–8, `--sha256` ou `--limit` inválidos); nada é baixado |

## `--json` (estável dentro da `api_version` 1)
`idb get`:
```json
{"id":1,"state":{"state":"Complete","data":{"level":"SizeAndValidator"}},"path":"/home/ana/Downloads/f.iso",
 "connections":8,"ramp_ms":2501,"peak_rate":16000000,
 "diagnostics":[{"code":{"code":"ServerPerConnectionLimit","params":{...}},"severity":"Info","text":"O servidor limita..."}]}
```
`idb doctor`:
```json
{"final_url":"...","size":268435456,"ranges":true,"validator":true,"recommended_connections":8,
 "peak_rate":16000000,"blocking":null,"diagnostics":[...]}
```
- `state`, `blocking` e `code` usam os mesmos tipos de `idb-types` da API (`docs/API.md`).
- `text` é a frase já traduzida, a mesma mostrada na TUI, na GUI e no app de celular.
- Campos novos podem aparecer; os existentes não mudam de significado sem trocar a `api_version`.
