# Rust の単一実行 file にし、Git の読取りはプロセス内で行う

Windows では git の起動 1 回に 29〜55 ms かかる。Python の lifecycle の時間の大半は、この起動の繰り返しが占める（workspace-lifecycle#37、work-records#233）。道具は runtime を持たない Rust の単一実行 file にする。HEAD、ref、config、worktree、status は gix でプロセス内から読む。git CLI は push と merge のときだけ起動し、hook と credential helper の挙動を変えない。C++ と Zig は性能が同等だが、state の安全性の保証と既存の toolchain で劣る。Go は GC があるうえ、Git library が未成熟である。libgit2 だけで push する案は、認証の挙動が git CLI とずれる。
