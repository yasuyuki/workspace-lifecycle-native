# state は task ごとの file にし、Git の共通 dir に置く

state は task ごとに 1 file とし、repository の Git 共通 dir に置く。更新は一時 file への書込みと rename で行う。legacy lifecycle は全 task を共有の state.json にまとめていたので、Windows で 1 回の書直しに 70〜125 s かかった（workspace-lifecycle#66）。そのうえ、別の task の worker どうしが同じ file を奪い合った。task ごとの file なら、一つの file に書く主体は常に一つなので、lock が要らない。
