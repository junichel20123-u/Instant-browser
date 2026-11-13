# Rajada

Gerenciador e acelerador de downloads para Linux, escrito em Rust. **Em desenvolvimento (Fase 0: fundação).**

- Acelera **quando é possível**, com múltiplas conexões HTTP Range ajustadas automaticamente.
- **Não promete o impossível:** nenhum programa passa da velocidade contratada da sua internet.
- **Nunca entrega um arquivo errado:** tudo é verificado antes de receber o nome final.
- **Seguro por padrão**, **educado com servidores** e **sem telemetria**.

## Estado atual
- `rajada-types`: estados, motivos, níveis de verificação, diagnósticos e eventos da API.
- `rajada-i18n`: textos em pt-BR e inglês.
- `faultbox`: servidor de teste com injeção de falhas.

## Desenvolvimento
```sh
cargo xtask ci
cargo run -p faultbox -- --port 8080
```
Leia `CLAUDE.md` e `docs/ESPECIFICACAO.md`.

## Licença
MIT OR Apache-2.0 (provisória).
