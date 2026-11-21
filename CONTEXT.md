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
置き換える前の Python の workspace-lifecycle の install。切替の前に始めた task を完了か退役まで扱う。push 判定と agent-runtime の workspace binding が参照する間も install を保持する。
_Avoid_: 旧版、互換層

**Task hold**:
`wl hold` で task を保留すること。`wl release-hold` に判断の参照を渡して解除するまで、finish や退役を止める。

**Push decision: hold**:
push preflight が、評価した push を見送ると判断した結果。

**Owner hold**:
本人が Issue に記録する、作業または判断の保留。

**Target integration**:
受入済みの成果が task の統合先の remote branch に届き、必要な検証を終えたこと。

**Default integration**:
受入済みの成果が repository の remote default branch に届いたこと。Issue を閉じる前に確認する。統合先が feature branch のときは、その成果を default branch へ届ける工程が別にある。

**Finish acceptance**:
`finish` が、呼ばれた時点の commit を task の統合対象として確定し、background work を始めて戻ること。確定した後に積まれた commit は、次の finish の対象になる。
_Avoid_: finish の完了、統合

**Worker**:
一つの finish acceptance について background work を進める、切り離された process。常駐しない。止まっても、次に呼ばれた command が同じ地点から再開させる。
_Avoid_: daemon、service、常駐 process

**Integration worktree**:
worker が統合だけに使う、task checkout とは別の作業領域。task checkout を変えないので、他の session は finish の間も同じ task を編集し続けられる。
_Avoid_: task checkout、lease

**Offload declaration**:
repository が validation の各手順に付ける、実行場所の宣言。`windows` か `any` のどちらかで、宣言が無い手順は `windows` として扱う。`any` の手順は WSL offload で走る。
_Avoid_: 自動判定、platform 推定

**Validation declaration**:
repository が管理下の file に書く validation の手順と、各手順の offload declaration。worker は、確定した commit に含まれる版を使う。task はこれを差し替えられない。
_Avoid_: validation argv、task の validator

**Needs-action**:
worker が自力で進められなくなった finish の状態。統合先を元に戻し、理由を記録して止まる。修正 commit を積んで finish を呼び直すと、そこから再開する。
_Avoid_: 失敗、待機、pending

**Slot**:
repository ごとに持つ、path が固定された再利用の checkout。begin は空いている slot を一つの task に割り当て、退役はその slot を空きに戻す。退役時点が分かる空き slot は、古く退役した順に割り当てる。task より長く残る。
_Avoid_: task checkout の作成、worktree の新規作成

**Slot cache**:
slot に残す ignored data のうち、repository が cache として宣言した path（cargo の target など）。slot を空きに戻しても消さず、次の task が使う。宣言の無い ignored data は、空きに戻すときに消す。
_Avoid_: target ignored data、作業の残り
