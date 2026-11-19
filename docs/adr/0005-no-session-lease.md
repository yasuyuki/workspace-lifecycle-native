# session lease を廃止し、統合は integration worktree で行う

legacy lifecycle は、task の lease を持つ session が残る間 finish を拒否し、validation の間は state lock を持ち続けた。その結果、session が一つでも開いていれば finish できず（work-records#258 の 2026-11-17 の記録）、同じ repository の別 task は 20 分以上 busy になった（workspace-lifecycle#60）。この道具は lease を持たない。finish acceptance で commit を確定し、統合は integration worktree で行う。task checkout には書かないので、排他が要らない。データと宛先を守る保証（push preflight、所有外 dirty の不混入、ignored collision、統合前の required check、hold）と、中断した finish の再開は残す。recovery copy は、commit されていない内容がある checkout を退役するときだけ作る。
