export const title = "wl と周辺ツールの状態遷移";

const phase = {
  accepted: "finish 受付済み",
  topicPushed: "topic を push 済み",
  prepared: "統合先で検証済み",
  targetPushed: "統合先へ push 済み",
  needsAction: "要対応",
};

export function state(v) {
  if (v.issue === "closed") return "Issue を close 済み";
  if (v.owner === "legacy") {
    if (v.phase === "working") return v.inDefault ? "legacy task の成果が default に届いた" : "切替前の legacy task が作業中";
    if (v.legacy === "removed") return "legacy install 撤去済み";
    return v.phase === "stranded" ? "legacy の入口で終えられない task が残る" : "legacy task が完了";
  }
  if (v.owner === "none") {
    if (!v.switched) return "legacy が入口";
    if (v.legacy === "removed") return "legacy install 撤去済み";
    if (v.bindingRemoved && v.pushMoved) return "push 判定を移行し binding を解除済み";
    if (v.pushMoved) return "push 判定を移行済み";
    if (v.bindingRemoved) return "workspace binding を解除済み";
    return "wl が入口";
  }
  if (v.phase === "working") {
    if (v.session === "blocked") return "agent-runtime が起動を拒否";
    return v.session === "running" ? "wl task の session が作業中" : "wl task を開始済み";
  }
  if (v.phase === "integrated") {
    if (v.session === "running") return "Target integration 済み、session は作業中";
    return v.inDefault ? "Default integration 済み" : "Target integration 済み";
  }
  if (v.phase === "retired") return v.inDefault ? "default 到達後に退役済み" : "slot は空き、default に未到達";
  if (v.phase === "reused") return v.inDefault ? "default 到達済みの slot を再利用中" : "slot 再利用中、default に未到達";
  return phase[v.phase];
}

const steps = {
  RulesSwitch: "agent-rules が入口を wl に切り替える",
  LegacyBegin: "legacy task を開始する",
  LegacyFinish: "legacy task を統合して退役する",
  LegacyWorkReachesDefault: "別の task が legacy task の成果を default に届ける",
  LegacyStranded: "legacy の入口では終えられないまま残る",
  WlBegin: "wl begin",
  MovePushDecision: "新しい push 判定に切り替える",
  RemoveWorkspaceBinding: "agent-runtime の binding を外す",
  LegacyUninstall: "作業を残す legacy task が無く、二つの移行も済んで撤去する",
  Launch: "agent-runtime で session を起動する",
  SessionEnds: "session が task の作業を終了する",
  PreflightPush: "push 判定が許可し topic を push する",
  PreflightStops: "push 判定が通らず停止する",
  TopicAndValidationPass: "topic check と検証が通る",
  TopicOrValidationFails: "topic check または検証が失敗する",
  TargetPush: "統合先へ push する",
  TargetCheckPasses: "統合先の check が通る",
  TargetCheckFails: "統合先の check が失敗する",
  WlRetire: "作業終了後に slot を退役する",
  NextTaskTakesSlot: "次の wl begin が同じ slot を使う",
  FeatureReachesDefault: "feature branch の成果が default に届く",
  CloseIssue: "Default integration を確認し Issue を close する",
};

export function step(action, from, to) {
  if (action === "WlFinish") {
    return from.session === "running" && to.session === "ended"
      ? "wl finish --retire を呼び task の作業を終了する"
      : "wl finish";
  }
  return steps[action];
}
