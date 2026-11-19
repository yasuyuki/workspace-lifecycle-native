# background work は finish ごとの切り離された worker が進める

finish acceptance は worker を一つ起動して即座に戻る。worker は各段階を state に記録する。worker が落ちても、PC を再起動しても、次に呼ばれた command が未完了の finish を見つけて worker を起動し直す。Windows のサービスやタスクスケジューラとして常駐させる案は、更新と退役の管理が要るので採らなかった。command が呼ばれるたびに少しずつ進める案は、人が何もしない間に進まないので採らなかった。結果は、その repository で次に実行した command の出力の先頭と `status` に出し、失敗したときは Windows のトースト通知も出す。Issue へのコメントは option とし、初版では作らない。
