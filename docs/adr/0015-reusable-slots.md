# task の checkout は、path が固定された slot を再利用する

Windows runner で `git worktree add` を直接計測すると、148 file で p50 120 ms、400 file で p50 206 ms（p90 262 ms）だった（run 44538036798）。funkot-player は 376 file あり、task ごとに checkout を作ると begin だけが ADR 0008 の 50 ms を超える。さらに cargo の build-dir は checkout の path ごとに分かれるので、新しい path では依存 crate の build をやり直すことになる。そこで、repository ごとに path が固定された slot をいくつか持つ。begin は空いている slot で branch を切り替え、base との差分だけを書く。退役は slot を空きに戻す。repository が宣言した slot cache だけを残し、それ以外の ignored data は消す。checkout の file を background で書く案は、file が揃う前に読み書きされると壊れるので採らない。command の名前は `wl` とする。
