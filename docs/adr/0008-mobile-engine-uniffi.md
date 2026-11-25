# ADR-0008: Motor no celular com UniFFI

- **Status:** aceito
- **Contexto:** além de controlar o PC, o app de celular poderá baixar no próprio aparelho. O motor é Rust e o
  workspace proíbe `unsafe`. Escrever a ponte C à mão exigiria `unsafe` e duas camadas de código manual (JNI e Swift).
- **Decisão:**
  1. **Novo crate `idb-mobile`** (`cdylib` + `staticlib`), fora do build padrão. Usa UniFFI 0.33 com proc-macros e
     gera Kotlin e Swift a partir da biblioteca compilada (`cargo xtask mobile-bindings` → `target/bindings/`).
  2. **API pequena e síncrona para o app:** `Engine(dataDir, language)`, `start`, `pause`, `resume`, `list`,
     `setListener(ProgressListener)`, `apiVersion`. Um runtime tokio interno (2 threads) roda o motor.
  3. **Manager compartilhado:** `idb-engine::manager::Manager` (iniciar, pausar, retomar, listar) é o mesmo que o
     daemon vai usar. Pausar = abortar a tarefa, seguro pelos mesmos invariantes que o teste de SIGKILL verifica.
  4. **Uma verdade só:** o app recebe o nome estável do estado (para ícones) e o texto já traduzido pelo `idb-i18n`.
  5. **Portabilidade:** `fallocate`/`fstatfs` só em Linux e Android (incluindo f2fs); no iOS e no macOS só `set_len`.
  6. **Licença:** o UniFFI é MPL-2.0, aprovado pelo dono do projeto. O `deny.toml` abre exceção **só para os crates
     `uniffi*`**, então qualquer outra dependência MPL continua barrada.
  7. **Lints:** o `idb-mobile` não herda `forbid(unsafe_code)` do workspace, porque o scaffolding gerado pelo UniFFI é
     código FFI. O código escrito à mão nesse crate continua sem `unsafe`, e os lints do clippy do workspace valem.
- **Verificação:** o teste do `idb-mobile` baixa pelo `Engine` exportado e confere o status traduzido. O job `mobile`
  do CI faz `cargo ndk check` para `aarch64-linux-android`. Esse check precisa do NDK e só roda no GitHub, não no
  ambiente local.
- **Limites atuais:** a velocidade instantânea não aparece no `MobileDownload` (o `Progress` do motor ainda não a
  carrega). Ainda não há serviço em primeiro plano no Android, nem `URLSession` em segundo plano no iOS: no celular, o
  download para quando o sistema suspende o app. A retomada usa o journal.
