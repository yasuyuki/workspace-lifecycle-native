# legacy install は依存がなくなってから撤去する

[ADR 0003](0003-new-state-legacy-tasks-drain.md) の撤去条件を置き換える。すべてのリポジトリで次の条件がそろうまで legacy install を保持する。

1. legacy が管理する task のうち、default branch の外に作業を持つものがない。legacy の入口で終えられる task は終える。
2. push 判定の実装が移行済みで、`wl` の設定と commit 規約が新しい正本を指している。
3. agent-runtime の workspace binding が legacy install を参照していない。

条件 1 は、当初は「legacy が管理する未完了 task がない」だった。legacy の入口では終えられない task があるため、2026-11-23 に本人の判断で改めた（#8）。legacy は primary checkout の task を退役させない。また、owner receipt を登録した task の `--abandon` を拒否する。その receipt の completion を動かす旧版の interpreter は、すでに撤去されている。これらの task の成果は default branch にあり、出力は削除済みである。state の記録だけが残り、install を撤去した後にそれを動かすものはない。

workspace binding は外す。`wl` は [session lease を持たない](0005-no-session-lease.md) ため、起動を監督する `resolve-run` は不要である。task の切替だけで install を撤去すると、push 判定と session 起動が止まる。

[横断モデル](../cross-tool/CrossTool.tla) で撤去条件と起動を確認できる。
