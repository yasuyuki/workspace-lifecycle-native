# push の判定を `wl` に移し、正本を `wl preflight` にする

[ADR 0012](0012-reuse-push-preflight.md) を置き換える。legacy install を撤去するには、push 判定の実装が移行済みであることが要る（[ADR 0016](0016-legacy-install-dependencies.md) の条件 2）。そこで、`workspace_lifecycle.push`（workspace-lifecycle 0.8.27）の判定表を、変えずに `wl` へ移す。

- 判定の入口は `wl preflight [REPO] [--user-intent auto|push|hold] [--temporary] [--oss auto|yes|no] [--policy PATH]` とする。出力の JSON（`decision`、`reason`、`remote`、`destination`、push のときだけ `push_argv`）、key の順序、非 ASCII の escape、exit status（push 0、hold 10、ask 11、usage 2）は、legacy と同じにする。規約と人の手順は、入口を差し替えるだけで済む。
- worker は Python を起動せず、process 内で同じ判定をする。user 設定には argv ではなく、private policy の path を `push_policy` として置く。外部の argv を置く `preflight` は、test が判定を差し替えるために残す。両方を置くと拒否する。
- GitHub の metadata は、required check と同じく process 内の HTTP で読む。`gh` は token を得るためにだけ起動する。legacy は `gh api` を起動していた。token が得られないとき、または API を読めないときは、legacy の `gh api` の失敗と同じく metadata を不明として ask にする。
- 一致は 2 つの方法で確かめた。legacy の test 14 件は、同じ fixture と期待値で Rust の test にした。controller では、実 repository 23 件と、ask と hold の各経路に届く fixture の repository 21 件で、6 通りの入力と 4 通りの policy について両者を実行した。642 組の stdout と exit status は、すべて一致した（workspace-lifecycle-native#8）。
- legacy の判定と違う点は 1 つだけである。`[` と `]` の対応が壊れた remote URL では、legacy は例外で止まる。`wl` は ask（`INVALID_REMOTE_URL`）を返す。
