# ADR-0005: Download segmentado, `pwrite`, pré-alocação e `rustix`

- **Status:** aceito (Fase 1.2)
- **Contexto:** a §7.2 e a §7.4 da especificação pedem sondagem dupla de Range, N conexões em paralelo, escrita
  posicional sem lock global e pré-alocação que não trave em FAT/exFAT/NTFS-3G. O workspace proíbe `unsafe`, então as
  chamadas `statvfs`/`fstatfs`/`fallocate` precisam de um wrapper seguro.
- **Decisão:**
  1. **Sondagem:** a resposta 206 ao primeiro `GET bytes=0-` prova suporte a Range. Uma resposta 200 com tamanho
     conhecido leva a uma segunda sonda de 1 byte no meio do arquivo; só um 206 exato ativa a segmentação. Sem isso:
     uma conexão e o diagnóstico `RangeNotSupported`. O corpo da primeira resposta vira o segmento 0 (nada se perde).
  2. **Segmentos:** `segments::plan(total, N, min_segment)` (padrão: N=4, segmento mínimo de 1 MiB, teto educado de
     8 conexões por host). Um ator tokio por segmento, cada um com seus próprios retries, `If-Range` e `Retry-After`.
     Um 200 ou um `Content-Range` com outro total vira `NeedsUser(ResourceChanged)` e cancela todos.
  3. **Escrita:** `FileExt::write_all_at` (`pwrite`) em `spawn_blocking`, em blocos de 256 KiB por segmento.
  4. **Durabilidade:** só o coordenador faz checkpoint: tira um retrato dos contadores, chama `fdatasync` e então grava
     todos os segmentos no journal (tabela `segments`) numa transação. O journal nunca afirma bytes que não estão no disco.
  5. **Disco:** `statvfs` antes de criar o `.part` (`NeedsUser(DiskFull)` cedo). `fstatfs` escolhe a pré-alocação:
     `fallocate` em ext4/xfs/btrfs/tmpfs, em segundo plano; só `set_len` nos demais sistemas de arquivos.
  6. **Dependência:** `rustix` 1.1 (feature `fs`; Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT), já usada
     indiretamente por muitos crates, e sem código de sistema.
- **Consequências:** N=1, ou servidores sem Range, continuam no caminho sequencial da Fase 1.1. O N ainda é fixo: o
  controlador adaptativo (§7.6), o work stealing e o watchdog ficam para a Fase 1.3.
