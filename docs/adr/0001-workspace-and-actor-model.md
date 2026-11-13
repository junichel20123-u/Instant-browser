# ADR-0001: Workspace Cargo e modelo de atores

- **Status:** aceito (Fase 0)
- **Contexto:** vários binários (daemon, CLI, TUI, GUI, native host) precisam dos mesmos estados e textos,
  e o motor precisa de concorrência sem locks espalhados.
- **Decisão:** um workspace com o layout da §5.2 da especificação, edição 2024, dependências centralizadas em
  `[workspace.dependencies]` e lints centralizados (`unsafe_code = forbid`, `unwrap_used`/`expect_used = deny`).
  O motor usará o modelo de atores: uma tarefa tokio por download, comunicação por canais com limite e um
  escalonador como única fonte de verdade das conexões. `rajada-gtk` fica fora de `default-members` até a Fase 3,
  porque exige GTK4/libadwaita no sistema.
- **Consequências:** builds e CI rápidos na Fase 0; cada crate novo herda as regras. Para compilar a GUI
  vai ser preciso `-p rajada-gtk` explícito e um runner com GTK.
