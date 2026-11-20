export const title = "wl finish と統合の遷移";

const stageLabel = (stage, pc, wStage) => {
  if (pc === "withdrawErr" || (pc === "writeStage" && wStage === "needs-action")) {
    return "失敗処理中 (queue 解放)";
  }
  if (stage === "none") return "作業中";
  if (stage === "accepted") return "受付済み (Accepted)";
  if (stage === "topic-pushed") {
    if (pc === "prepMerge" || pc === "prepValidate" || (pc === "writeStage" && wStage === "prepared")) {
      return "統合先で merge と検証中";
    }
    return "topic push 済みと順番待ち";
  }
  if (stage === "prepared") {
    if (pc === "pushTargetExec" || pc === "writeStage") return "統合先へ push 中";
    return "統合先で検証済み (Prepared)";
  }
  if (stage === "target-pushed") return "統合先へ push 済み (CI 待ち)";
  if (stage === "integrated") return "統合済み";
  if (stage === "needs-action") return "要対応 (needs-action)";
  return stage;
};

export const state = (v) => {
  const tasks = Object.keys(v.accept || {});
  if (tasks.length > 1) {
    const label = (t) => {
      if (v.retired[t]) return "退役済み";
      if (!v.accept[t].exists) return v.finPhase[t] === "queuing" ? "受付書込済みとqueue未登録" : "未finish";
      const st = v.prog[t].attempt === v.accept[t].attempt ? v.prog[t].stage : "accepted";
      const suffix = (v.inWorktree || []).includes(t) ? "[作業領域使用中]" : "";
      return `${stageLabel(st, v.wPc[t], v.wStage[t])}${suffix}`;
    };
    return `A:${label("a")} | B:${label("b")}`;
  }
  const t = tasks[0] ?? "a";
  if (v.retired[t]) return "退役済み";
  if (!v.accept[t].exists) return "作業中";
  const st = v.prog[t].attempt === v.accept[t].attempt ? v.prog[t].stage : "accepted";
  return stageLabel(st, v.wPc[t], v.wStage[t]);
};

export const group = (v) => {
  const tasks = Object.keys(v.accept || {});
  if (tasks.length > 1) {
    if ((v.inWorktree || []).length > 1 || v.badQueueRemove) return "attention";
    return "work";
  }
  const t = tasks[0] ?? "a";
  if (v.retired[t]) return "done";
  if (!v.accept[t].exists) return "work";
  if (v.wPc?.[t] === "withdrawErr" || (v.wPc?.[t] === "writeStage" && v.wStage?.[t] === "needs-action")) {
    return "attention";
  }
  const st = v.prog[t].attempt === v.accept[t].attempt ? v.prog[t].stage : "accepted";
  if (st === "integrated") return "done";
  if (st === "needs-action") return "attention";
  if (st === "none") return "work";
  return "integrating";
};

export const colors = {
  work: "blue",
  integrating: "purple",
  attention: "orange",
  done: "teal",
};

const changedTask = (from, to) => {
  for (const t of Object.keys(to.accept || {})) {
    if (
      from.wPc?.[t] !== to.wPc?.[t] ||
      from.wStage?.[t] !== to.wStage?.[t] ||
      from.prog?.[t]?.stage !== to.prog?.[t]?.stage ||
      from.accept?.[t]?.attempt !== to.accept?.[t]?.attempt ||
      from.finPhase?.[t] !== to.finPhase?.[t] ||
      from.retired?.[t] !== to.retired?.[t]
    ) {
      return t;
    }
  }
  return Object.keys(to.accept || {})[0] ?? "a";
};

export const step = (action, from, to) => {
  const t = changedTask(from, to);
  switch (action) {
    case "FinishWriteAccept":
      return Object.keys(from.accept).length > 1
        ? `${t.toUpperCase()} の wl finish が accept.json を書込む`
        : "wl finish";
    case "FinishEnqueue":
      return `${t.toUpperCase()} を queue へ遅れて登録`;
    case "WorkerStart":
      return Object.keys(from.accept).length > 1 ? `${t.toUpperCase()} の worker を起動または再起動` : null;
    case "WorkerPushTopic":
      return "topic へ push";
    case "WorkerWaitTurn": {
      const lenDiff = (to.inWorktree || []).length - (from.inWorktree || []).length;
      const prefix = Object.keys(from.accept).length > 1 ? `${t.toUpperCase()} が` : "";
      if (lenDiff > 0) return `${prefix}順番を獲得し統合先へ入る`;
      return "queue を確認し待機";
    }
    case "WorkerPrepareMerge":
      return to.wPc?.[t] === "withdrawErr" ? "merge 衝突" : "統合先で merge";
    case "WorkerPrepareValidate":
      return to.wPc?.[t] === "withdrawErr" ? "検証失敗" : "統合先で検証成功";
    case "WorkerCheckTopicCI":
      if (to.wPc?.[t] === "withdrawErr") return "topic check 失敗";
      if (to.wPc?.[t] === "pushTargetExec") return "topic check 成功";
      return null;
    case "WorkerPushTarget":
      if (to.wStage?.[t] === "topic-pushed") return "統合先が進み再準備";
      if (to.wStage?.[t] === "target-pushed") return "統合先へ push (queue 削除)";
      if (to.wPc?.[t] === "withdrawErr") return "保留または判定拒否";
      return null;
    case "WorkerTargetChecks":
      return to.wStage?.[t] === "integrated" ? "統合先 check 成功" : "統合先 check 失敗";
    case "WorkerRetire":
      return to.retired?.[t] ? "自動退役 (--retire)" : "退役拒否";
    case "WorkerWithdraw":
      return Object.keys(from.accept).length > 1
        ? `${t.toUpperCase()} の withdraw が queue 削除`
        : "queue 削除 (withdraw)";
    case "WorkerWriteStage": {
      const s = to.prog?.[t]?.stage;
      if (s === "needs-action") return "要対応 (needs-action) を記録";
      if (s === "integrated") return "統合済みを記録";
      if (s === "topic-pushed") return "進捗 TopicPushed を記録";
      if (s === "prepared") return "進捗 Prepared を記録";
      if (s === "target-pushed") return "進捗 TargetPushed を記録";
      return "進捗を保存";
    }
    case "WorkerKill":
      return `${t.toUpperCase()} の worker が強制終了 (lock 解放)`;
    case "UserCommitFix":
      return "修正 commit";
    case "CiTopicCompletes":
    case "CiTargetCompletes":
    case "ExternalTargetPush":
    case "Done":
      return null;
    default:
      return action;
  }
};
