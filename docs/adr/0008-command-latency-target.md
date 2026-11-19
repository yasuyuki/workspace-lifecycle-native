# 人が直接打つ command の command latency は、controller の実測で p90 50 ms 以下

対象は `status`、`begin`、`finish` の受付、`hold` で、Windows controller で実測した p90 を 50 ms 以下にする。runner の実測（ADR 0004）から、controller では 25〜30 ms と推定している。50 ms は、その推定に余裕を持たせつつ、人が待ちを感じない水準である。background work の所要時間はこの基準に含めない。最初に切り替える repository は funkot-player と funkot-autodj-for-ui で、そこで受入を通してから管理系の repository へ広げる。
