# Política de segurança

## Como reportar
Não abra issue pública para vulnerabilidades. Use o recurso **"Report a vulnerability"** (GitHub Security Advisories) deste repositório.
Respondemos em até 7 dias e combinamos uma data de divulgação coordenada (padrão: 90 dias ou quando a correção sair).

## Escopo
Daemon (`idbd`), API (socket Unix e TCP opt-in), native host, extensão, CLI, GUI e o empacotamento.

## Garantias de projeto
- Nenhuma porta TCP aberta por padrão; socket Unix com permissão 0600.
- TCP só em loopback, opt-in, com token de 256 bits e verificação de `Origin`/`Host`.
- Escrita restrita a `allowed_dirs`; nomes vindos do servidor são sanitizados.
- Segredos nunca em argv nem em logs. Sem telemetria.
