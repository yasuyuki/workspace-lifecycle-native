export const title = "wl slot と claim の状態遷移";

const isMultiTask = (v) => Object.keys(v.taskRec || {}).length > 1;
const hasTwoClaimsA = (v) =>
  Object.values(v.slotClaim || {}).filter((c) => c === "a").length > 1;
const hasTwoTasksOneSlot = (v) => {
  const slots = Object.values(v.taskRec || {})
    .filter((r) => r.exists)
    .map((r) => r.slot);
  return slots.length > 1 && new Set(slots).size < slots.length;
};
const hasTaskWithoutClaim = (v) =>
  Object.entries(v.taskRec || {}).some(
    ([t, r]) => r.exists && v.slotClaim?.[String(r.slot)] !== t
  );
const hasOrphanClaim = (v) => {
  const idle =
    Object.values(v.bPc || {}).every((pc) => pc === "idle") &&
    Object.values(v.rPc || {}).every((pc) => pc === "idle") &&
    v.pPc === "idle";
  if (!idle) return false;
  return Object.entries(v.slotClaim || {}).some(
    ([n, c]) =>
      c !== "none" &&
      (!v.taskRec?.[c]?.exists || String(v.taskRec[c].slot) !== String(n))
  );
};
const hasStalePrepare = (v) =>
  !v.prepareLock && Object.values(v.slotClaim || {}).includes("#prepare");

export const state = (v) => {
  const slotDesc = (n) => {
    const k = String(n);
    const c = v.slotClaim?.[k] ?? "none";
    const json = v.slotJson?.[k];
    const br = v.slotBranch?.[k] ?? "none";
    const claimText =
      c === "none" ? "claim無" : c === "#prepare" ? "prepare" : `claim:${c.toUpperCase()}`;
    const wtText = !json ? "未作成" : br === "detached" ? "detached" : `branch:${br}`;
    return `S${n}[${claimText}/${wtText}]`;
  };
  const taskDesc = (t) => {
    const r = v.taskRec?.[t];
    if (!r || !r.exists) {
      if (v.rPc?.[t] === "taskDeleted") return `${t.toUpperCase()}:task削除済`;
      if (Object.values(v.slotClaim || {}).includes(t)) return `${t.toUpperCase()}:claim済(task無)`;
      return `${t.toUpperCase()}:未開始`;
    }
    if (v.rPc?.[t] === "checked" || v.rPc?.[t] === "orderRead") return `${t.toUpperCase()}:退役検証済(S${r.slot})`;
    return `${t.toUpperCase()}:${r.stage}(S${r.slot})`;
  };

  if (!isMultiTask(v) && Object.keys(v.slotClaim || {}).length === 1 && !(v.bPc && "a2" in v.bPc)) {
    const c = v.slotClaim["1"];
    const r = v.taskRec["a"];
    if (r.exists) {
      if (v.rPc["a"] === "checked" || v.rPc["a"] === "orderRead") return "退役処理中 (branch 削除済)";
      if (r.stage === "ready") return "作業可能 (Ready)";
      if (v.bPc["a1"] === "checkoutReady") return "checkout 準備済";
      return "開始済 (Claimed)";
    }
    if (v.rPc["a"] === "taskDeleted") return "退役処理中 (Task 削除済)";
    if (c === "#prepare") return v.prepareLock ? "準備中 (prepare claim)" : "stale prepare (回収待ち)";
    if (c === "a") return "claim 済 (Task 未記録)";
    if (v.taskLock?.["a"] !== "none") return "begin ロック取得 (base 確認中)";
    return "空き (準備済)";
  }

  const slots = Object.keys(v.slotClaim || {}).sort().map(slotDesc).join(" ");
  const tasks = Object.keys(v.taskRec || {}).sort().map(taskDesc).join(" ");
  return `${tasks} | ${slots}`;
};

export const group = (v) => {
  if (
    hasTwoClaimsA(v) ||
    hasTwoTasksOneSlot(v) ||
    hasTaskWithoutClaim(v) ||
    hasOrphanClaim(v) ||
    hasStalePrepare(v) ||
    v.prepareOverwroteSlot ||
    v.badHousekeeping
  ) {
    return "attention";
  }
  if (!isMultiTask(v) && Object.keys(v.slotClaim || {}).length === 1 && !(v.bPc && "a2" in v.bPc)) {
    const c = v.slotClaim["1"];
    const r = v.taskRec["a"];
    if (r.exists && r.stage === "ready") return "work";
    if (r.exists || c === "a" || c === "#prepare" || v.rPc["a"] !== "idle" || v.taskLock?.["a"] !== "none") {
      return "busy";
    }
    return "done";
  }
  return "work";
};

export const colors = {
  done: "teal",
  busy: "purple",
  work: "blue",
  attention: "orange",
};

const actorLabel = (from, to) => {
  for (const p of Object.keys(to.bPc || {})) {
    if (
      from.bPc?.[p] !== to.bPc?.[p] ||
      from.bRuns?.[p] !== to.bRuns?.[p] ||
      (from.bClaimsSnap?.[p] || []).length !== (to.bClaimsSnap?.[p] || []).length
    ) {
      return Object.keys(to.bPc).length > 1 ? `${p}: ` : "";
    }
  }
  for (const t of Object.keys(to.rPc || {})) {
    if (from.rPc?.[t] !== to.rPc?.[t]) {
      return Object.keys(to.taskRec || {}).length > 1 ? `${t.toUpperCase()}: ` : "";
    }
  }
  return "";
};

export const step = (action, from, to) => {
  const prefix = actorLabel(from, to);
  switch (action) {
    case "BeginAcquireLock":
    case "ChartAcquireLock":
      return `${prefix}begin が task_lock 取得`;
    case "BeginResolveAndScan":
      return `${prefix}base 確認と claim 走査`;
    case "BeginBaseError":
      return `${prefix}resolve_base 失敗 (claim 前に解放)`;
    case "BeginCreateClaim":
      return `${prefix}claim 作成`;
    case "BeginWriteTask":
      return `${prefix}Task(Claimed) 書込`;
    case "BeginEnsureCheckout":
      return `${prefix}slot checkout 確認`;
    case "BeginSwitchAndReady":
      return `${prefix}branch 切替と Ready 書込`;
    case "RetireCheckAndClean":
      return `${prefix}retire 安全性検査と clean`;
    case "RetireReadOrder":
      return null;
    case "RetireRemoveTask":
      return `${prefix}tasks/t.json 削除`;
    case "RetireRemoveClaim":
      return `${prefix}slots/n.claim 削除`;
    case "RetireCrashAfterTaskDelete":
      return `${prefix}claim 削除前に強制終了`;
    case "PrepareClaimSlot":
    case "ChartPrepareClaim":
      return "prepare_slot が prepare claim 書込";
    case "PrepareFinish":
      return "prepare_slot 完了 (claim 解放)";
    case "PrepareCrash":
      return "prepare_slot が claim 解放前に終了";
    case "HousekeepingReclaim":
      return "housekeeping が孤立 claim 回収";
    case "Done":
    case "ChartDone":
      return null;
    default:
      return `${prefix}${action}`;
  }
};
