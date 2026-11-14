# Benchmarks

Toda afirmação de desempenho precisa de uma linha aqui, com comando reproduzível
(`hyperfine`, várias repetições, mediana), máquina e versão.

## Metas de projeto (objetivos, não resultados)
| Métrica | Meta |
|---|---|
| RSS do daemon parado | ≤ 15 MB |
| RSS do daemon com 10 downloads | ≤ 40 MB |
| CPU por MB | ≤ aria2 + 20% |
| Rampa até 90% do platô (limite por conexão) | ≤ 3 s |
| Latência da API p99 sob carga | < 50 ms |

Regressão > 20% em qualquer métrica falha o CI (a partir da Fase 1).

## Resultados

### 2026-11-14 — Fase 1.2 (download segmentado com N fixo)

**Como reproduzir:** `cargo xtask bench` (precisa de `aria2c` e `hyperfine` no `PATH`). O comando sobe o `faultbox` em
release, em `127.0.0.1`, e mede com `hyperfine` o download de 32 MiB por cada ferramenta. Tempo, CPU e RSS máximo são
medianas medidas pelo próprio hyperfine.

**Aviso honesto:** tudo roda em loopback, no mesmo container de nuvem (4 vCPUs compartilhadas). O cenário A mede o
efeito de um servidor que **limita a velocidade por conexão**, que é onde várias conexões ajudam. O cenário B mede só o
custo da própria ferramenta. Nenhum dos dois representa uma conexão real de internet.

Máquina: 4 CPUs, Linux 6.18.44-fc-v470

Versões: idb 0.0.1 · GNU Wget 1.21.4 built on linux-gnu. · aria2 version 1.37.0 · hyperfine 2.0.0

curl: curl 8.5.0 (x86_64-pc-linux-gnu) libcurl/8.5.0 OpenSSL/3.0.13 zlib/1.3 brotli/1.1.0 zstd/1.5.5 libidn2/2.3.7 libpsl/0.21.2 (+libidn2/2.3.7) libssh/0.10.6/openssl/zlib nghttp2/1.59.0 librtmp/2.3 OpenLDAP/2.6.10

### A — servidor limita 2 MiB/s por conexão (32 MiB)

| Ferramenta | Tempo, mediana (s) | Desvio (s) | Vazão (MiB/s) | CPU (ms/MiB) | RSS máx. (MiB) |
|---|---:|---:|---:|---:|---:|
| `idb -c1` | 16.71 | 0.00 | 1.9 | 5.82 | 9.8 |
| `idb -c4` | 4.20 | 0.00 | 7.6 | 2.43 | 11.6 |
| `idb -c8` | 2.11 | 0.00 | 15.1 | 2.42 | 13.3 |
| `wget` | 16.70 | 0.02 | 1.9 | 2.34 | 4.4 |
| `curl` | 16.72 | 0.02 | 1.9 | 2.91 | 10.2 |
| `aria2c -x1` | 16.76 | 0.01 | 1.9 | 2.63 | 17.6 |
| `aria2c -x8 -s8` | 2.15 | 0.00 | 14.9 | 1.96 | 23.9 |

3 execuções por ferramenta (+1 de aquecimento), mediana.

### B — sem limite, loopback (32 MiB)

| Ferramenta | Tempo, mediana (s) | Desvio (s) | Vazão (MiB/s) | CPU (ms/MiB) | RSS máx. (MiB) |
|---|---:|---:|---:|---:|---:|
| `idb -c1` | 0.10 | 0.01 | 309.4 | 1.44 | 12.1 |
| `idb -c4` | 0.09 | 0.01 | 359.3 | 1.41 | 16.9 |
| `idb -c8` | 0.08 | 0.00 | 389.9 | 1.55 | 19.5 |
| `wget` | 0.07 | 0.01 | 478.4 | 0.57 | 4.5 |
| `curl` | 0.07 | 0.01 | 430.4 | 0.79 | 10.2 |
| `aria2c -x1` | 0.16 | 0.03 | 197.5 | 1.23 | 17.6 |
| `aria2c -x8 -s8` | 0.13 | 0.02 | 237.5 | 1.85 | 21.9 |

5 execuções por ferramenta (+1 de aquecimento), mediana.

#### Leitura dos números
- **Cenário A (limite de 2 MiB/s por conexão):** com 4 conexões, o IDB baixa 4,0× mais rápido que com uma (4,20 s
  contra 16,71 s); com 8, 7,9× (2,11 s). Isso empata com o `aria2c -x8 -s8` (2,15 s). Com uma conexão, todas as
  ferramentas ficam no teto do servidor (~1,9 MiB/s), como esperado.
- **Cenário B (sem limite):** no loopback, o `wget` (0,07 s) e o `curl` (0,07 s) são mais rápidos que o `idb -c1`
  (0,10 s). O IDB paga a abertura do journal SQLite, o `fdatasync` dos checkpoints e a verificação final. Mesmo assim,
  ele é mais rápido que o `aria2c` nos dois modos.
- **Memória:** o IDB usa de 9,8 a 19,5 MiB de RSS, contra 17,6 a 23,9 MiB do `aria2c`. O `wget` (4,5 MiB) é bem menor.
- **CPU (meta: ≤ aria2 + 20%):**
  - ✅ No cenário B, o `idb -c1` gasta 1,44 ms/MiB contra 1,23 do `aria2c -x1` (+17%), e o `idb -c4` gasta 1,41 contra
    1,85 do `aria2c -x8`.
  - ❌ No cenário A, o `idb -c1` gasta **5,82 ms/MiB** contra 2,63 do `aria2c -x1` (+121%). No caminho de uma conexão,
    o download lento chega em pedaços pequenos, e cada pedaço vira uma escrita assíncrona separada (`tokio::fs`), que
    passa pela thread pool. Correção prevista na Fase 1.3: usar o mesmo escritor com buffer de 256 KiB do modo
    segmentado também para uma conexão.

#### Metas de projeto nesta medição
| Meta | Situação |
|---|---|
| Rampa até 90% do platô ≤ 3 s (limite por conexão) | Ainda não medido (N fixo; o controlador adaptativo é da Fase 1.3) |
| CPU por MB ≤ aria2 + 20% | ✅ no cenário B; ❌ no cenário A com `-c1` (ver acima) |
| RSS do daemon | Ainda sem daemon (Fase 2); a CLI usa de 10 a 20 MiB |
