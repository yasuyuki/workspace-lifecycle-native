# WSL offload は既存の ubuntu-26-development distro で行う

offload 専用の user と directory を、既存の `ubuntu-26-development` distro の中に設ける。cargo と Python はすでに入っている。session は Windows に置くので（ADR 0001）、この distro で開発 session とメモリを奪い合うことは無くなる。offload 専用の distro を作る案は、toolchain の導入と保守がもう 1 系統要るので採らない。
