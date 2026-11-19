# worker は衝突も失敗も解決せず、needs-action で止まる

merge の衝突、validation の失敗、required check の失敗のどれでも、worker は統合先を元に戻し、理由を記録して終了する。修正は task checkout で行い、次の finish acceptance で取り込む。衝突の解決を待つ作業領域を用意する案は採らない。待つ worker は事実上の常駐 process になるからである。同じ統合先への finish は受付順に 1 件ずつ統合する。先行する統合で統合先が進んだときは、merge commit を作り直して validation をやり直す。複数をまとめて統合する merge train は、失敗の原因を切り分ける仕組みが要るので、初版では作らない。
