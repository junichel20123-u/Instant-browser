# ADR-0004: Nome definitivo — Instant Download Browser (IDB)

- **Status:** aceito
- **Contexto:** a especificação usava o nome provisório "Rajada". O dono do projeto escolheu
  **Instant Download Browser**, abreviado **IDB**.
- **Decisão:** trocar tudo, usando o prefixo `idb`:

| Antes | Depois |
|---|---|
| crates `rajada-*` | `idb-*` |
| binários `rajada`, `rajadad`, `rajada-tui`, `rajada-gtk`, `rajada-host` | `idb`, `idbd`, `idb-tui`, `idb-gtk`, `idb-host` |
| variáveis `RAJADA_*` | `IDB_*` |
| `$XDG_STATE_HOME/rajada`, `$XDG_RUNTIME_DIR/rajada/rajada.sock` | `.../idb`, `.../idb/idb.sock` |
| User-Agent `rajada/<versão>` | `idb/<versão>` |

- **Consequências / riscos conhecidos:**
  - Já existe um CLI chamado `idb` (iOS Development Bridge, do Meta), e "IDB" também é a sigla comum de IndexedDB.
    Isso pode gerar colisão de nome em pacotes de distro (AUR, deb, rpm) ou em buscas.
  - Se houver colisão no empacotamento, renomear **só o pacote** (por exemplo, `instant-download-browser`) e manter
    os binários; se o binário `idb` colidir no `PATH` do usuário, oferecer o alias `instant-dl`. Nada disso muda o código.
  - O app ID do Flatpak/D-Bus ainda está em aberto.
  - Journals criados em `~/.local/state/rajada` antes da troca não são migrados (projeto ainda sem usuários).
