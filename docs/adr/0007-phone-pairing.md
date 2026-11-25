# ADR-0007: Pareamento do celular (controle remoto na rede local)

- **Status:** aceito (base pronta; o listener entra com o daemon, na Fase 2)
- **Contexto:** o usuário quer controlar o IDB do computador pelo celular. A especificação proíbe porta aberta por
  padrão e RPC sem autenticação (princípio 2, §8.3).
- **Decisão:**
  1. **Opt-in e só na rede local:** o listener WebSocket + TLS fica desligado até o usuário pedir o pareamento.
     Ele escuta só em interfaces de LAN, nunca em `0.0.0.0` público, e verifica `Origin` e `Host`.
  2. **Confiança pelo QR:** a oferta `idb://pair?v=1&h=HOST&p=PORTA&fp=SHA256_DO_CERT&c=CÓDIGO` vira um QR. O
     celular fixa o certificado autoassinado pelo `fp` (sem autoridade certificadora e sem confiar na rede).
  3. **Código de uso único:** 128 bits, válido por 5 minutos. Ele vira um token de dispositivo de 256 bits.
  4. **Só hashes no disco:** o journal guarda o SHA-256 do código e do token (tabelas `pairing_codes` e `devices`).
     O token aparece em claro uma única vez, na resposta ao celular.
  5. **Revogação imediata:** `idb devices revoke ID`. Um token revogado recebe `Unauthorized` (-32001).
  6. **Mesmas regras da API local:** o celular usa o mesmo protocolo (`docs/API.md`) e os mesmos `allowed_dirs`.
     Não existe API "especial" para o celular.
- **Já implementado:** `crates/idb-engine/src/pairing.rs` (oferta, troca do código, autenticação, parse da URI, que
  o app também usa), as tabelas no `idb-store`, `idb devices list|revoke` e os testes de uso único, expiração e
  revogação.
- **Fica para a Fase 2:** o listener, a geração e o armazenamento do certificado TLS, o QR na CLI/GUI e o comando
  `idb pair`. Ele não foi feito agora porque, sem listener e sem certificado, só imprimiria um QR que nenhum celular
  conseguiria usar.
