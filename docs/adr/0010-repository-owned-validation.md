# validation の手順は repository が宣言し、task は選ばない

validation の手順は、repository が管理下の file に宣言する。worker は、確定した commit に含まれる版を使う。legacy lifecycle は begin で task ごとに argv を受け取った。その結果、推奨を決めた後も、agent が重い test module を登録した task が 25 件中 8 件あった（work-records#233）。
