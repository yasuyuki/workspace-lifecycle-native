# 新しい state を持ち、既存の task は legacy lifecycle で完了させる

この道具は、Python の lifecycle の state.json を読まず、変換もしない。切替の前に始めた task は、legacy lifecycle で完了か退役まで進める。legacy install の撤去条件は [ADR 0016](0016-legacy-install-dependencies.md) で置き換えた。既存の state を読む案は、旧形式とその不変条件をすべて抱えることになる。一括変換する案は、使い捨ての code と、変換に失敗したときの復旧経路を要する。
