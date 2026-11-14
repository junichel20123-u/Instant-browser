# ADR-0006: Controlador adaptativo, work stealing e watchdog

- **Status:** aceito (Fase 1.3)
- **Contexto:** a §7.6 e a §7.7 da especificação pedem que o programa descubra sozinho quantas conexões ajudam, que
  conexões ociosas peguem trabalho das lentas e que uma conexão congelada seja substituída.
- **Decisão:**
  1. **Mapa de trabalho (`workmap.rs`):** o download é uma lista de faixas (*leases*). Cada faixa tem o próprio lock
     pequeno, com `received` (bytes aceitos do socket) e `written` (bytes no disco). Uma conexão ociosa pega uma faixa
     livre ou **rouba** a metade final da faixa com mais bytes ainda não recebidos. O corte é feito depois de
     `received`, então bytes ainda em buffer nunca ficam além do novo fim. Estacionar uma conexão = cortar a faixa no
     que ela já recebeu; o resto volta livre. Um proptest garante: sem buraco, sem sobreposição, `written ≤ received`.
  2. **Controlador (`controller.rs`):** mede os bytes **recebidos do socket** em janelas de 500 ms e ignora a
     primeira janela (partida). Começa com 1 conexão e acrescenta +1, +2, +4… enquanto o ganho da última adição for
     > 10% e o RTT (aproximado pelo tempo até o primeiro byte) não subir mais de 50% **e** mais de 50 ms. Quando a
     adição não ajuda, volta ao último número que ajudou e para. 429/503 cortam pela metade. Teto: 8 por host.
     `-c N` (= `fixed:N`) desliga o controlador.
  3. **Workers curtos:** cada tarefa faz um único pedido para uma faixa. Retries, `Retry-After` (`PoliteWait`),
     backoff e o limite de tentativas sem progresso ficam com o coordenador, que é o único dono do mapa.
  4. **Watchdog:** a cada janela, uma faixa sem bytes há mais de `stall_timeout` (padrão 20 s) tem a tarefa abortada
     e volta livre, com o diagnóstico `StalledConnectionReassigned`.
  5. **Journal:** o checkpoint grava o mapa inteiro (`checkpoint_map`) numa transação, depois do `fdatasync`.
  6. **Diagnósticos:** cada decisão medida gera `ConnectionsAdjusted`. Ao estabilizar, `ServerPerConnectionLimit`
     (> 2 conexões) ou `LinkSaturated` (≤ 2), com a vazão medida naquela janela.
- **Desvio da especificação:** o tamanho de segmento não segue o alvo "vazão × 4 s". Ele surge das divisões ao meio
  (piso: `min_segment`, 1 MiB), o que dá pedaços grandes no começo e pequenos no fim, onde o roubo mais importa. A
  sondagem periódica para *aumentar* de novo depois de estabilizar também ficou de fora: depois de um 429, o
  controlador fica no número reduzido até o fim do download (escolha educada).
- **Consequências:** `-c 1` com Range também usa o escritor segmentado (lotes de 256 KiB). O caminho sequencial
  (sem Range ou sem tamanho) passou a usar um `BufWriter` de 256 KiB.
