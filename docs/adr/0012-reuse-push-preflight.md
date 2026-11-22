# push の判定は既存の workspace_lifecycle.push を worker から呼ぶ

[ADR 0017](0017-push-decision-in-wl.md) で置き換えた。

worker は push の前に、既存の `workspace_lifecycle.push` を指定された private policy つきで呼ぶ。Python の起動は約 100 ms だが、background で起きるので command latency には入らない。規約は push の判定をこの入口に委ねており、判定表を別の場所で再実装しないよう定めている。Rust へ移すのは、legacy lifecycle を退役するときに、push 判定の正本ごと移す別の作業として扱う。
