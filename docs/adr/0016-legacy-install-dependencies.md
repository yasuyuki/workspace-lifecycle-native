# legacy install は依存がなくなってから撤去する

[ADR 0003](0003-new-state-legacy-tasks-drain.md) の撤去条件を置き換える。すべてのリポジトリで次の条件がそろうまで legacy install を保持する。

1. legacy が管理する未完了 task がない。
2. push 判定の実装が移行済みで、`wl` の設定と commit 規約が新しい正本を指している。
3. agent-runtime の workspace binding が legacy install を参照していない。

workspace binding は外す。`wl` は [session lease を持たない](0005-no-session-lease.md) ため、起動を監督する `resolve-run` は不要である。task の切替だけで install を撤去すると、push 判定と session 起動が止まる。

[横断モデル](../cross-tool/CrossTool.tla) で撤去条件と起動を確認できる。
