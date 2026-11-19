# 統合後の退役は `finish --retire` のときだけ、checkout が変わっていなければ行う

`finish --retire` を指定すると、worker は統合の後で task checkout を確かめる。次の 3 つを満たすときだけ、退役、回収、topic branch の削除を行う。HEAD が確定した commit と同じであること。commit されていない変更が無いこと。確定の後に積まれた commit が無いこと。どれかを満たさなければ needs-action にする。lease が無いので（ADR 0005）、統合の間も同じ task の編集が続いている場合があり、無条件に退役するとその作業を消してしまう。
