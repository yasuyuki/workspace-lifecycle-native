# 新しい state を持ち、既存の task は legacy lifecycle で完了させる

この道具は、Python の lifecycle の state.json を読まず、変換もしない。切替の前に始めた task は、legacy lifecycle で完了か退役まで進め、その後で legacy の install を退役する。既存の state を読む案は、旧形式とその不変条件をすべて抱えることになる。一括変換する案は、使い捨ての code と、変換に失敗したときの復旧経路を要する。
