# 開発 session は Windows に置き、Linux は WSL offload にだけ使う

開発の目標は音の device に依存する Windows アプリなので、session を Windows から外せない。work-records#258 は session を通常 Linux へ移し、Windows で行う確認を GitHub runner に任せた。しかしその方式では、作業を環境間で移す必要を人がリアルタイムで検出しなければならず、人の時間を奪う。そこで #258 の session 配置を覆し、すべての session を Windows に置くことにした。Linux は、Windows の道具が人の操作なしに起動する WSL offload にだけ使う。
