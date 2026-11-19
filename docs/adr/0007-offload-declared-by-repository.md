# WSL offload の対象は repository が宣言し、宣言の無い手順は Windows で走らせる

validation の手順のうち、どれを WSL offload で走らせるかは、repository が手順ごとに `windows` か `any` で宣言する。宣言が無ければ `windows` として扱う。道具が内容から自動で判定する案は採らない。誤判定すると、Windows でしか出ない失敗を見逃すからである。`any` の手順は、確定した commit を WSL 内の file system に展開してから実行する。`/mnt/c` を経由する読み書きは遅いので使わない。
