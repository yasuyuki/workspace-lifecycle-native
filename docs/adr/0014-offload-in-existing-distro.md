# WSL offload は Windows 側の Ubuntu-26.04 distro で行う

offload 専用の user と directory を、Windows に既にある `Ubuntu-26.04` distro の中に設ける。session は Windows に置くので（ADR 0001）、この distro で開発 session とメモリを奪い合うことは無い。offload 専用の distro を作る案は、distro の導入と保守がもう 1 系統要るので採らない。

当初は `ubuntu-26-development` に置くつもりだった。しかしその distro は Windows との間を遮断した通常 Linux の環境であり、その遮断は変えない。build 用の package は `Ubuntu-26.04` にあるが、cargo は無かった。そのため offload user の home に rustup で Rust を入れ、`~/.profile` から読む。offload の手順は commit の内容だけを入力にするので、toolchain の場所はこの user の中で完結する。
