# IDB no celular

Duas formas de uso, com a **mesma** base: os estados, os textos e o protocolo do desktop.

## 1. Controle remoto do computador (prioridade)
O celular comanda o IDB que roda no Linux: adicionar um link, ver o progresso, pausar, retomar. O arquivo fica no PC.

| Peça | Situação |
|---|---|
| Protocolo JSON-RPC v1 independente de transporte (`idb-types::api`, `docs/API.md`) | ✅ pronto, com snapshots |
| Pareamento: oferta `idb://pair`, código de uso único, tokens com hash, revogação (`pairing.rs`, ADR-0007) | ✅ pronto |
| `idb devices list|revoke` | ✅ pronto |
| Daemon `idbd` com o `Manager` | Fase 2 (o `Manager` já existe no motor) |
| Listener WebSocket + TLS na LAN, certificado, QR, `idb pair` | Fase 2 |
| App (Kotlin/Compose): ler QR, fixar o certificado, falar o protocolo v1 | Depois da Fase 2 |

## 2. Baixar no próprio celular
O motor roda dentro do app.

| Peça | Situação |
|---|---|
| Motor portável (Linux/Android/iOS) | ✅ `fallocate` só onde existe |
| Fachada `idb-mobile` com UniFFI (ADR-0008) | ✅ pronta e testada no Linux |
| Bindings Kotlin e Swift (`cargo xtask mobile-bindings`) | ✅ gerados em `target/bindings/` |
| Check de compilação para Android no CI (`cargo ndk`) | ✅ job criado (roda só no GitHub) |
| Serviço em primeiro plano (Android) / download em segundo plano (iOS) | A fazer |
| Velocidade instantânea no `MobileDownload` | A fazer |

## Exemplo (Kotlin, com os bindings gerados)
```kotlin
val engine = Engine(context.filesDir.path, "pt-BR")
engine.setListener(object : ProgressListener {
    override fun onUpdate(download: MobileDownload) {
        // Chega de uma thread de fundo: poste para a UI.
        mainHandler.post { adapter.update(download) } // download.statusText = "Baixando"
    }
})
val id = engine.start("https://exemplo.org/arquivo.iso", downloadsDir.path)
engine.pause(id)
engine.resume(id)
```

## Ordem sugerida
1. **Fase 2:** `idbd` + listener LAN + `idb pair` (QR).
2. **App Android mínimo:** parear, listar, adicionar e pausar pelo protocolo v1.
3. **Download local no app:** `idb-mobile` + serviço em primeiro plano.
4. **iOS:** os mesmos passos, com os bindings Swift.
