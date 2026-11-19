# Native workspace lifecycle

Windows で開発 session を動かす人と agent のために、task の開始から統合・退役までを扱う道具の語彙。

## Language

**Command latency**:
command を起動してから、呼び手に制御が戻るまでの時間。人が意識しない水準に保つ対象である。
_Avoid_: 処理時間、所要時間

**Background work**:
command が始めるが、呼び手を待たせない仕事。validation、required check の待ち、統合、それらの lock 待ちを含む。
_Avoid_: 待ち、非同期処理、後処理

**WSL offload**:
platform に依らない background work を、Windows の道具が人の操作なしに WSL の distro で実行すること。session は Windows に残る。
_Avoid_: 引継ぎ、Linux への移行、Linux session

**Environment handoff**:
人が作業を別の環境の session へ移すこと。人がその必要を検出しなければならないので、この道具はこれを要求しない。
_Avoid_: offload、委譲

**Legacy lifecycle**:
置き換える前の Python の workspace-lifecycle の install。切替の前に始めた task だけを、完了か退役まで扱う。
_Avoid_: 旧版、互換層
