# ADR-0003: Retomada segura e `get` idempotente

- **Status:** aceito (Fase 1.1)
- **Contexto:** o teste de crash injection (SIGKILL em momentos aleatórios) mostrou duas janelas que geravam um
  arquivo duplicado (`file (1)`), embora nunca um arquivo errado: (a) entre o `rename` e a gravação de `Complete`;
  (b) depois de `Complete` gravado, antes de o processo sair. Além disso, com checkpoint só por bytes (4 MiB), arquivos
  pequenos ou links lentos perdiam todo o progresso a cada interrupção.
- **Decisão:**
  1. Gravar `final_path` no journal antes do `rename`. Se, ao retomar, o `.part` não existe e o destino existe,
     verificar o destino de novo em vez de baixar.
  2. `get` do mesmo `(url, dir)` já concluído, com o arquivo final presente, verifica de novo e devolve `Complete`
     (com `--sha256`, o arquivo existente é conferido contra esse hash). Para baixar de novo, apague o arquivo.
  3. Checkpoint durável por bytes **ou** por tempo (padrão: 4 MiB ou 1 s).
  4. Ao retomar, 200 com o **mesmo** validador significa que o servidor ignora Range: recomeçar do zero com o corpo
     recebido. Validador diferente significa que o recurso mudou: `NeedsUser(ResourceChanged)`.
- **Consequências:** 1.000+ SIGKILLs sem nenhum arquivo errado nem duplicado. Um `fdatasync` por segundo, no
  máximo, durante o download (custo a medir na Fase 1.2, no `BENCHMARKS.md`).
