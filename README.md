# Instant Download Browser (IDB)

O **Instant Download Browser (IDB)** é um gerenciador e acelerador de downloads para Linux, escrito em Rust. **Em desenvolvimento (Fase 1.2: download segmentado).**

- Acelera **quando é possível**, com múltiplas conexões HTTP Range ajustadas automaticamente.
- **Não promete o impossível:** nenhum programa passa da velocidade contratada da sua internet.
- **Nunca entrega um arquivo errado:** tudo é verificado antes de receber o nome final.
- **Seguro por padrão**, **educado com servidores** e **sem telemetria**.

## Estado atual
- `idb-types`: estados, motivos, níveis de verificação, diagnósticos e eventos da API.
- `idb-i18n`: textos em pt-BR e inglês.
- `faultbox`: servidor de teste com injeção de falhas.
- `idb-engine` + `idb-store` + `idb get`: download com uma conexão, retomada, journal e verificação.

## Uso (Fase 1.1)
```sh
idb get URL [-d PASTA] [-c CONEXÕES] [--sha256 HEX] [--json]
```
Quando o servidor permite baixar em partes, o IDB usa até `-c` conexões (padrão 4; teto educado de 8 por servidor).
Se o servidor não permite, usa uma só e explica o motivo. Rodar de novo o mesmo comando retoma de onde parou. O arquivo é baixado como `.part` e só recebe o nome final depois
de verificado.

| Código de saída | Significado |
|---|---|
| 0 | Concluído e verificado |
| 1 | Falhou (a mensagem diz o próximo passo) |
| 2 | Precisa de você (link expirou, login, arquivo mudou, disco cheio...) |
| 3 | Erro interno |
| 64 | Uso incorreto (por exemplo, `-c` acima do teto) |

## Desenvolvimento
```sh
cargo xtask ci
cargo run -p faultbox -- --port 8080
```
Leia `CLAUDE.md` e `docs/ESPECIFICACAO.md`.

## Licença
MIT OR Apache-2.0 (provisória).
