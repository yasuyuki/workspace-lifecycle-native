# 常駐プロセスを置かず、command ごとに起動する

Windows runner で計測した（2026-11-19、`probe` workflow、run 44532516295、44533201127、および 24b9326 の run）。Rust の実行 file は 1 回の起動が約 5.5 ms だった。gix を既定の設定で開くと、新しい process の初回だけ、open に約 31 ms、dirty の判定に約 30 ms が余分にかかった。環境の設定を探さない `Options::isolated()` では、open が 1.5 ms、dirty の判定が 6.7 ms だった。これは小さな repository の値で、同じ process 内で 2 回目に読んだ値とほぼ同じである。初回だけの費用は設定の探索から来ており、読む設定を限定すれば消える。したがって、新しい process で起動してから読むまでを 20 ms 前後に収められる。常駐 process を置くと、更新、障害時の回復、古い版の退役が要るが、それで縮むのは数 ms にすぎない。比較のため、Python の lifecycle の `--version` は同じ runner で 85〜108 ms、git の起動は 1 回 19〜27 ms だった。
