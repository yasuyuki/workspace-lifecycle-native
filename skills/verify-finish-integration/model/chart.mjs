export const title = "wl finish と統合の遷移";

const stageLabel = (stage, pc, wStage) => {
  if (stage === "none") return "作業中";
  if (stage === "accepted") return "受付済み (Accepted)";
  if (stage === "topic-pushed") {
    if (pc === "prepMerge" || pc === "prepValidate" || (pc === "writeStage" && wStage === "prepared")) {
      return "統合先で merge と検証中";
    }
    if (pc === "writeStage" && wStage === "needs-action") {
      return "要対応を記録中";
    }
    return "topic push 済みと順番待ち";
  }
  if (stage === "prepared") {
    if (pc === "pushTargetExec" || (pc === "writeStage" && wStage === "target-pushed")) {
      return "統合先へ push 中";
    }
    if (pc === "writeStage" && wStage === "topic-pushed") {
      return "再準備を記録中";
    }
    return "統合先で検証済み (Prepared)";
  }
  if (stage === "target-pushed") {
    if (pc === "leaveQueue") return "統合先へ push 済み (queue 解放中)";
    return "統合先へ push 済み (CI 待ち)";
  }
  if (stage === "integrated") return "統合済み";
  if (stage === "needs-action") {
    if (pc === "leaveQueue") return "要対応記録済み (queue 解放中)";
    return "要対応 (needs-action)";
  }
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
  const st = v.prog[t].attempt === v.accept[t].attempt ? v.prog[t].stage : "accepted";
  if (st === "integrated") return "done";
  if (st === "needs-action") return "attention";
  if (v.wPc?.[t] === "writeStage" && v.wStage?.[t] === "needs-action") return "attention";
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
  const multi = Object.keys(from.accept || {}).length > 1;
  switch (action) {
    case "FinishWriteAccept":
      return multi ? `${t.toUpperCase()} の wl finish が accept.json を書込む` : "wl finish";
    case "FinishEnqueue":
      return `${t.toUpperCase()} を queue へ登録`;
    case "WorkerStart":
      return multi ? `${t.toUpperCase()} の worker を起動または再起動` : null;
    case "WorkerPushTopic":
      return "topic へ push";
    case "WorkerWaitTurn":
      return "queue 確認 (順番待ち)";
    case "WorkerPrepareAcquire":
      return multi ? `${t.toUpperCase()} が統合ロックを取得` : "順番と統合ロックを取得";
    case "WorkerPrepareMerge":
      return to.wPc?.[t] === "writeStage" && to.wStage?.[t] === "needs-action" ? "merge 衝突" : "統合先で merge";
    case "WorkerPrepareValidate":
      return to.wStage?.[t] === "needs-action" ? "検証失敗" : "統合先で検証成功";
    case "WorkerCheckTopicCI":
      if (to.wStage?.[t] === "needs-action") return "topic check 失敗";
      if (to.wPc?.[t] === "acquirePushLock") return "topic check 成功";
      return null;
    case "WorkerPushAcquire":
      return multi ? `${t.toUpperCase()} が push 用統合ロック取得` : "push 用統合ロックを取得";
    case "WorkerPushTarget":
      if (to.wStage?.[t] === "topic-pushed") return "統合先が進み再準備";
      if (to.wStage?.[t] === "target-pushed") return "統合先へ push";
      if (to.wStage?.[t] === "needs-action") return "保留または判定拒否";
      return null;
    case "WorkerTargetChecks":
      return to.wStage?.[t] === "integrated" ? "統合先 check 成功" : "統合先 check 失敗";
    case "WorkerRetire":
      return to.retired?.[t] ? "自動退役 (--retire)" : "退役拒否";
    case "WorkerWriteStage": {
      const s = to.prog?.[t]?.stage;
      if (s === "needs-action") return "要対応 (needs-action) を記録";
      if (s === "integrated") return "統合済みを記録";
      if (s === "topic-pushed") return "進捗 TopicPushed を記録";
      if (s === "prepared") return "進捗 Prepared を記録";
      if (s === "target-pushed") return "進捗 TargetPushed を記録";
      return "進捗を保存";
    }
    case "WorkerLeaveQueue":
      return multi ? `${t.toUpperCase()} の queue を削除` : "queue を削除";
    case "WorkerKill":
      return `${t.toUpperCase()} の worker が強制終了 (全ロック解放)`;
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
