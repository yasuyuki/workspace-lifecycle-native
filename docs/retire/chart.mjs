export const title = "wl retire and retire --abandon";
export const state = (v) => {
  if (v.phase === "retired") return v.abandoned ? "統合せず退役済み" : "退役済み";
  const parts = [];
  if (v.held) parts.push("保留中");
  if (v.dirty) parts.push("未 commit の変更あり");
  else if (v.inTarget) parts.push("すべて統合先にある");
  else if (v.onTopic) parts.push("統合先に無い commit が remote topic にある");
  else parts.push("統合先に無い commit が手元だけにある");
  return parts.join(" / ");
};
export const step = (a, from, to) => ({
  YouEdit: "あなたが file を変える",
  YouCommit: "あなたが commit する",
  YouPushTopic: "あなたが topic を push する",
  FinishIntegrates: "wl finish が統合する",
  YouHold: "wl hold",
  YouReleaseHold: "wl release-hold",
  Retire: "wl retire",
  RetireAbandon: "wl retire --abandon",
  Done: null,
}[a] ?? a);
export const group = (v) => (v.phase === "retired" ? "done" : v.held || v.dirty ? "attention" : "work");
export const colors = { work: "blue", attention: "orange", done: "teal" };
