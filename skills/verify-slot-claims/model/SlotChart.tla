----------------------------- MODULE SlotChart -------------------------------
(* Clean single-task single-slot instance of SlotClaims for drawing the
   nominal slot and claim lifecycle chart under TLC's chart budget. *)
EXTENDS SlotClaims

ChartDone ==
  /\ AllProcsIdle
  /\ bRuns["a1"] = 1
  /\ ~taskRec["a"].exists
  /\ pRan
  /\ UNCHANGED vars

ChartAcquireLock ==
  /\ ~prepareLock
  /\ BeginAcquireLock("a1")

ChartPrepareClaim ==
  /\ bPc["a1"] = "idle"
  /\ PrepareClaimSlot

ChartNext ==
  \/ ChartAcquireLock
  \/ BeginResolveAndScan("a1")
  \/ BeginCreateClaim("a1")
  \/ BeginWriteTask("a1")
  \/ BeginEnsureCheckout("a1")
  \/ BeginSwitchAndReady("a1")
  \/ RetireCheckAndClean("a")
  \/ RetireRemoveTask("a")
  \/ RetireRemoveClaim("a")
  \/ ChartPrepareClaim
  \/ PrepareFinish
  \/ ChartDone

ChartSpec == Init /\ [][ChartNext]_vars
=============================================================================
