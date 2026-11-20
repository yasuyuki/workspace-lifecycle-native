----------------------------- MODULE SlotClaims ------------------------------
(* Models slot & claim management across wl/src/commands.rs and wl/src/store.rs
   at commit a0a389f3312c98f267ea88f2ec5f86c651948651:
   - begin()          : commands.rs:109-182, Workspace::claim_slot/claim_new_slot:49-72,
                        ensure_slot_checkout:75-86, switch_slot:185-208
   - retire()         : commands.rs:212-219, retire_task:224-275
   - prepare_slot()   : commands.rs:352-390
   - finish::running(): finish.rs:104-108 (abstracted into finishRunning[t])
   - Store primitives : store.rs:33-132 (create_new, create_new_json, write_json, remove)
*)
EXTENDS Integers, FiniteSets

CONSTANTS
  Tasks,               \* e.g. {"a"} or {"a", "b"}
  Slots,               \* e.g. {1} or {1, 2, 3}
  InitFreeSlot1,       \* BOOLEAN: whether slot 1 starts prepared and free
  MaxCycles,           \* Nat: max begin attempts per begin actor
  AllowPrepare,        \* BOOLEAN: enable prepare_slot() background actor
  AllowRetire,         \* BOOLEAN: enable retire_task()
  AllowRiskStates,     \* BOOLEAN: enable hold/dirty/unpublished/finishRunning toggles
  AllowBaseError,      \* BOOLEAN: model resolve_base() failure after claim_slot()
  AllowPrepareCrash,   \* BOOLEAN: model crash/error while #prepare claim is held
  AllowRetireCrash,    \* BOOLEAN: model crash between remove(task) and remove(claim)
  AllowSameTaskRace,   \* BOOLEAN: two concurrent `wl begin a` invocations
  AllowRetireClaimed   \* BOOLEAN: retire_task() may act on TaskStage::Claimed mid-begin

VARIABLES
  slotJson,            \* [Slots -> BOOLEAN]: slots/<n>.json exists
  slotWorktree,        \* [Slots -> BOOLEAN]: product.slots/<n> checkout exists
  slotBranch,          \* [Slots -> {"none", "detached"} \cup Tasks]
  slotClaim,           \* [Slots -> {"none", "#prepare"} \cup Tasks]
  taskRec,             \* [Tasks -> [exists: BOOLEAN, slot: {0} \cup Slots, stage: {"none", "claimed", "ready"}]]
  hold,                \* [Tasks -> BOOLEAN]
  dirty,               \* [Tasks -> BOOLEAN]
  unpublished,         \* [Tasks -> BOOLEAN]
  finishRunning,       \* [Tasks -> BOOLEAN]
  prepareLock,         \* BOOLEAN: workers/prepare.lock held
  bPc,                 \* [Procs -> {"idle", "scannedClaims", "claimed", "taskWritten", "checkoutReady"}]
  bSlot,               \* [Procs -> {0} \cup Slots]
  bClaimsSnap,         \* [Procs -> SUBSET Slots]
  bRuns,               \* [Procs -> 0..MaxCycles]
  rPc,                 \* [Tasks -> {"idle", "checked", "taskDeleted"}]
  rSlot,               \* [Tasks -> {0} \cup Slots]
  pPc,                 \* "idle" | "claimedExisting" | "claimedNew"
  pSlot,               \* {0} \cup Slots
  pRan,                \* BOOLEAN: prepare_slot() has completed or crashed once
  retiredSnap,         \* [Tasks -> [checked: BOOLEAN, notHeld: BOOLEAN, clean: BOOLEAN, published: BOOLEAN, notFinishing: BOOLEAN]]
  prepareOverwroteSlot \* BOOLEAN: ghost flag set if prepare_slot overwrites a claimed/live slot

vars == <<slotJson, slotWorktree, slotBranch, slotClaim, taskRec,
          hold, dirty, unpublished, finishRunning, prepareLock,
          bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot, pPc, pSlot, pRan,
          retiredSnap, prepareOverwroteSlot>>

Procs ==
  IF AllowSameTaskRace THEN {"a1", "a2"}
  ELSE IF "b" \in Tasks THEN {"a1", "b1"}
  ELSE {"a1"}

OwnerOf(p) == IF p \in {"a1", "a2"} THEN "a" ELSE "b"

SlotOrZero == {0} \cup Slots
NoTaskRec == [exists |-> FALSE, slot |-> 0, stage |-> "none"]
InitSnap == [checked |-> FALSE, notHeld |-> TRUE, clean |-> TRUE, published |-> TRUE, notFinishing |-> TRUE]

MinNat(S) == CHOOSE x \in S : \A y \in S : x <= y

AllProcsIdle ==
  /\ \A p \in Procs : bPc[p] = "idle"
  /\ \A t \in Tasks : rPc[t] = "idle"
  /\ pPc = "idle"

Init ==
  /\ slotJson = [n \in Slots |-> (InitFreeSlot1 /\ n = 1)]
  /\ slotWorktree = [n \in Slots |-> (InitFreeSlot1 /\ n = 1)]
  /\ slotBranch = [n \in Slots |-> IF InitFreeSlot1 /\ n = 1 THEN "detached" ELSE "none"]
  /\ slotClaim = [n \in Slots |-> "none"]
  /\ taskRec = [t \in Tasks |-> NoTaskRec]
  /\ hold = [t \in Tasks |-> FALSE]
  /\ dirty = [t \in Tasks |-> FALSE]
  /\ unpublished = [t \in Tasks |-> FALSE]
  /\ finishRunning = [t \in Tasks |-> FALSE]
  /\ prepareLock = FALSE
  /\ bPc = [p \in Procs |-> "idle"]
  /\ bSlot = [p \in Procs |-> 0]
  /\ bClaimsSnap = [p \in Procs |-> {}]
  /\ bRuns = [p \in Procs |-> 0]
  /\ rPc = [t \in Tasks |-> "idle"]
  /\ rSlot = [t \in Tasks |-> 0]
  /\ pPc = "idle"
  /\ pSlot = 0
  /\ pRan = FALSE
  /\ retiredSnap = [t \in Tasks |-> InitSnap]
  /\ prepareOverwroteSlot = FALSE

(* --- begin(): read task record or scan slots/*.claim (commands.rs:129-133, 49-55) --- *)

BeginStartScan(p) ==
  /\ bPc[p] = "idle"
  /\ bRuns[p] < MaxCycles
  /\ LET t == OwnerOf(p)
         ownClaimed == { n \in Slots : slotClaim[n] = t }
     IN IF taskRec[t].exists
        THEN IF taskRec[t].stage = "ready"
             THEN /\ bRuns' = [bRuns EXCEPT ![p] = bRuns[p] + 1]
                  /\ UNCHANGED <<bPc, bSlot, bClaimsSnap>>
             ELSE /\ bPc' = [bPc EXCEPT ![p] = "taskWritten"]
                  /\ bSlot' = [bSlot EXCEPT ![p] = taskRec[t].slot]
                  /\ bRuns' = [bRuns EXCEPT ![p] = bRuns[p] + 1]
                  /\ UNCHANGED bClaimsSnap
        ELSE IF ownClaimed # {}
             THEN /\ bSlot' = [bSlot EXCEPT ![p] = MinNat(ownClaimed)]
                  /\ bPc' = [bPc EXCEPT ![p] = "claimed"]
                  /\ bRuns' = [bRuns EXCEPT ![p] = bRuns[p] + 1]
                  /\ UNCHANGED bClaimsSnap
             ELSE /\ bClaimsSnap' = [bClaimsSnap EXCEPT ![p] = { n \in Slots : slotClaim[n] # "none" }]
                  /\ bPc' = [bPc EXCEPT ![p] = "scannedClaims"]
                  /\ bRuns' = [bRuns EXCEPT ![p] = bRuns[p] + 1]
                  /\ UNCHANGED bSlot
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, taskRec,
               hold, dirty, unpublished, finishRunning, prepareLock,
               rPc, rSlot, pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot>>

(* --- claim_slot() / claim_new_slot(): atomic Store::create_new (commands.rs:56-72) --- *)

BeginCreateClaim(p) ==
  /\ bPc[p] = "scannedClaims"
  /\ LET t == OwnerOf(p)
         freeExisting == { n \in Slots : slotJson[n] /\ n \notin bClaimsSnap[p] }
         uncreated == { n \in Slots : ~slotJson[n] /\ slotClaim[n] = "none" }
         chosen == IF freeExisting # {}
                   THEN MinNat(freeExisting)
                   ELSE IF uncreated # {}
                   THEN MinNat(uncreated)
                   ELSE 0
     IN /\ chosen # 0
        /\ IF slotClaim[chosen] = "none"
           THEN /\ slotClaim' = [slotClaim EXCEPT ![chosen] = t]
                /\ bSlot' = [bSlot EXCEPT ![p] = chosen]
                /\ bPc' = [bPc EXCEPT ![p] = "claimed"]
                /\ UNCHANGED bClaimsSnap
           ELSE /\ bClaimsSnap' = [bClaimsSnap EXCEPT ![p] = bClaimsSnap[p] \cup {chosen}]
                /\ UNCHANGED <<slotClaim, bSlot, bPc>>
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, taskRec,
               hold, dirty, unpublished, finishRunning, prepareLock,
               bRuns, rPc, rSlot, pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot>>

(* --- begin(): resolve_base() + Store::create_new_json(task_path) (commands.rs:135-159) --- *)

BeginWriteTask(p) ==
  /\ bPc[p] = "claimed"
  /\ LET t == OwnerOf(p)
         n == bSlot[p]
         fresh == [exists |-> TRUE, slot |-> n, stage |-> "claimed"]
     IN IF ~taskRec[t].exists
        THEN /\ taskRec' = [taskRec EXCEPT ![t] = fresh]
             /\ bPc' = [bPc EXCEPT ![p] = "taskWritten"]
             /\ UNCHANGED bSlot
        ELSE \* create_new_json returns false (Another begin created tasks/<t>.json)
             IF taskRec[t].stage = "ready"
             THEN /\ bPc' = [bPc EXCEPT ![p] = "idle"]
                  /\ bSlot' = [bSlot EXCEPT ![p] = 0]
                  /\ UNCHANGED taskRec
             ELSE /\ bSlot' = [bSlot EXCEPT ![p] = taskRec[t].slot]
                  /\ bPc' = [bPc EXCEPT ![p] = "taskWritten"]
                  /\ UNCHANGED taskRec
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim,
               hold, dirty, unpublished, finishRunning, prepareLock,
               bClaimsSnap, bRuns, rPc, rSlot, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot>>

(* resolve_base() fails at commands.rs:138 or 142 AFTER claim_slot() at line 133 *)
BeginBaseError(p) ==
  /\ AllowBaseError
  /\ bPc[p] = "claimed"
  /\ bPc' = [bPc EXCEPT ![p] = "idle"]
  /\ bSlot' = [bSlot EXCEPT ![p] = 0]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, taskRec,
               hold, dirty, unpublished, finishRunning, prepareLock,
               bClaimsSnap, bRuns, rPc, rSlot, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot>>

(* --- ensure_slot_checkout() (commands.rs:168, 75-86) --- *)

BeginEnsureCheckout(p) ==
  /\ bPc[p] = "taskWritten"
  /\ LET n == bSlot[p]
     IN IF slotJson[n]
        THEN UNCHANGED <<slotWorktree, slotBranch, slotJson>>
        ELSE /\ slotWorktree' = [slotWorktree EXCEPT ![n] = TRUE]
             /\ slotBranch' = [slotBranch EXCEPT ![n] =
                                 IF slotWorktree[n] THEN slotBranch[n] ELSE "detached"]
             /\ slotJson' = [slotJson EXCEPT ![n] = TRUE]
  /\ bPc' = [bPc EXCEPT ![p] = "checkoutReady"]
  /\ UNCHANGED <<slotClaim, taskRec, hold, dirty, unpublished, finishRunning,
               prepareLock, bSlot, bClaimsSnap, bRuns, rPc, rSlot,
               pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot>>

(* --- switch_slot() + write_json(Ready) (commands.rs:173-175, 185-208) --- *)

BeginSwitchAndReady(p) ==
  /\ bPc[p] = "checkoutReady"
  /\ LET t == OwnerOf(p)
         n == bSlot[p]
     IN /\ ~dirty[t]
        /\ IF TRUE \* Mutant hook: skip switch_slot before writing Ready
           THEN slotBranch' = [slotBranch EXCEPT ![n] = t]
           ELSE UNCHANGED slotBranch
        /\ taskRec' = [taskRec EXCEPT ![t] = [exists |-> TRUE, slot |-> n, stage |-> "ready"]]
        /\ bPc' = [bPc EXCEPT ![p] = "idle"]
        /\ bSlot' = [bSlot EXCEPT ![p] = 0]
  /\ UNCHANGED <<slotJson, slotWorktree, slotClaim, hold, dirty, unpublished,
               finishRunning, prepareLock, bClaimsSnap, bRuns, rPc, rSlot,
               pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot>>

(* --- retire_task(): safety checks + branch delete (commands.rs:224-268) --- *)

RetireCheckAndClean(t) ==
  /\ AllowRetire
  /\ rPc[t] = "idle"
  /\ taskRec[t].exists
  /\ (AllowRetireClaimed \/ taskRec[t].stage = "ready")
  /\ ~hold[t]
  /\ ~finishRunning[t]
  /\ ~dirty[t]
  /\ ~unpublished[t]
  /\ LET n == taskRec[t].slot
     IN /\ retiredSnap' = [retiredSnap EXCEPT ![t] =
                            [checked |-> TRUE,
                             notHeld |-> ~hold[t],
                             clean |-> ~dirty[t],
                             published |-> ~unpublished[t],
                             notFinishing |-> ~finishRunning[t]]]
        /\ slotBranch' = [slotBranch EXCEPT ![n] = IF slotWorktree[n] THEN "detached" ELSE slotBranch[n]]
        /\ rSlot' = [rSlot EXCEPT ![t] = n]
        /\ rPc' = [rPc EXCEPT ![t] = "checked"]
        /\ hold' = [hold EXCEPT ![t] = FALSE]
        /\ dirty' = [dirty EXCEPT ![t] = FALSE]
        /\ unpublished' = [unpublished EXCEPT ![t] = FALSE]
        /\ finishRunning' = [finishRunning EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<slotJson, slotWorktree, slotClaim, taskRec, prepareLock,
               bPc, bSlot, bClaimsSnap, bRuns, pPc, pSlot, pRan,
               prepareOverwroteSlot>>

(* --- retire_task() line 269 then lines 270-272 --- *)

RetireRemoveTask(t) ==
  /\ rPc[t] = "checked"
  /\ IF TRUE \* Mutant hook: swap order so claim is removed BEFORE task record
     THEN /\ taskRec' = [taskRec EXCEPT ![t] = NoTaskRec]
          /\ rPc' = [rPc EXCEPT ![t] = "taskDeleted"]
          /\ UNCHANGED slotClaim
     ELSE /\ slotClaim' = [slotClaim EXCEPT ![rSlot[t]] =
                             IF slotClaim[rSlot[t]] = t THEN "none" ELSE slotClaim[rSlot[t]]]
          /\ rPc' = [rPc EXCEPT ![t] = "taskDeleted"]
          /\ UNCHANGED taskRec
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, hold, dirty, unpublished,
               finishRunning, prepareLock, bPc, bSlot, bClaimsSnap, bRuns,
               rSlot, pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot>>

RetireRemoveClaim(t) ==
  /\ rPc[t] = "taskDeleted"
  /\ LET n == rSlot[t]
     IN slotClaim' = [slotClaim EXCEPT ![n] = IF slotClaim[n] = t THEN "none" ELSE slotClaim[n]]
  /\ rPc' = [rPc EXCEPT ![t] = "idle"]
  /\ rSlot' = [rSlot EXCEPT ![t] = 0]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, taskRec,
               hold, dirty, unpublished, finishRunning, prepareLock,
               bPc, bSlot, bClaimsSnap, bRuns, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot>>

(* Crash after Store::remove(&task_path) at line 269 before Store::remove(&claim(n)) at line 271 *)
RetireCrashAfterTaskDelete(t) ==
  /\ AllowRetireCrash
  /\ rPc[t] = "taskDeleted"
  /\ rPc' = [rPc EXCEPT ![t] = "idle"]
  /\ rSlot' = [rSlot EXCEPT ![t] = 0]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, taskRec,
               hold, dirty, unpublished, finishRunning, prepareLock,
               bPc, bSlot, bClaimsSnap, bRuns, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot>>

(* --- prepare_slot() (commands.rs:352-390) --- *)

PrepareClaimSlot ==
  /\ AllowPrepare
  /\ pPc = "idle"
  /\ ~prepareLock
  /\ ~pRan
  /\ LET freeExisting == { n \in Slots : slotJson[n] /\ slotClaim[n] = "none" }
         uncreated == { n \in Slots : ~slotJson[n] /\ slotClaim[n] = "none" }
     IN IF freeExisting # {}
        THEN LET n == MinNat(freeExisting)
             IN IF TRUE \* Mutant hook: skip #prepare claim on free slot
                THEN /\ slotClaim[n] = "none"
                     /\ slotClaim' = [slotClaim EXCEPT ![n] = "#prepare"]
                     /\ prepareLock' = TRUE
                     /\ pSlot' = n
                     /\ pPc' = "claimedExisting"
                ELSE /\ prepareLock' = TRUE
                     /\ pSlot' = n
                     /\ pPc' = "claimedExisting"
                     /\ UNCHANGED slotClaim
        ELSE /\ uncreated # {}
             /\ LET n == MinNat(uncreated)
                IN /\ slotClaim' = [slotClaim EXCEPT ![n] = "#prepare"]
                   /\ prepareLock' = TRUE
                   /\ pSlot' = n
                   /\ pPc' = "claimedNew"
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, taskRec,
               hold, dirty, unpublished, finishRunning, bPc, bSlot, bClaimsSnap,
               bRuns, rPc, rSlot, pRan, retiredSnap, prepareOverwroteSlot>>

PrepareFinish ==
  /\ pPc \in {"claimedExisting", "claimedNew"}
  /\ prepareOverwroteSlot' = (prepareOverwroteSlot \/ (slotClaim[pSlot] # "#prepare" /\ slotClaim[pSlot] # "none"))
  /\ IF pPc = "claimedExisting"
     THEN /\ slotBranch' = [slotBranch EXCEPT ![pSlot] = "detached"]
          /\ UNCHANGED <<slotWorktree, slotJson>>
     ELSE /\ slotWorktree' = [slotWorktree EXCEPT ![pSlot] = TRUE]
          /\ slotJson' = [slotJson EXCEPT ![pSlot] = TRUE]
          /\ slotBranch' = [slotBranch EXCEPT ![pSlot] = "detached"]
  /\ slotClaim' = [slotClaim EXCEPT ![pSlot] = IF slotClaim[pSlot] = "#prepare" THEN "none" ELSE slotClaim[pSlot]]
  /\ prepareLock' = FALSE
  /\ pPc' = "idle"
  /\ pSlot' = 0
  /\ pRan' = TRUE
  /\ UNCHANGED <<taskRec, hold, dirty, unpublished, finishRunning,
               bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot, retiredSnap>>

(* Crash or error while holding #prepare claim (commands.rs:372-388) *)
PrepareCrash ==
  /\ AllowPrepareCrash
  /\ pPc \in {"claimedExisting", "claimedNew"}
  /\ prepareLock' = FALSE
  /\ pPc' = "idle"
  /\ pSlot' = 0
  /\ pRan' = TRUE
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, taskRec,
               hold, dirty, unpublished, finishRunning, bPc, bSlot, bClaimsSnap,
               bRuns, rPc, rSlot, retiredSnap, prepareOverwroteSlot>>

(* --- Risk-state toggles when a task is Ready and not mid-retire --- *)

CanToggleRisk(t) ==
  /\ AllowRiskStates
  /\ taskRec[t].exists
  /\ taskRec[t].stage = "ready"
  /\ rPc[t] = "idle"

ToggleHold(t) ==
  /\ CanToggleRisk(t)
  /\ hold' = [hold EXCEPT ![t] = ~hold[t]]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, taskRec,
               dirty, unpublished, finishRunning, prepareLock, bPc, bSlot,
               bClaimsSnap, bRuns, rPc, rSlot, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot>>

ToggleDirty(t) ==
  /\ CanToggleRisk(t)
  /\ dirty' = [dirty EXCEPT ![t] = ~dirty[t]]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, taskRec,
               hold, unpublished, finishRunning, prepareLock, bPc, bSlot,
               bClaimsSnap, bRuns, rPc, rSlot, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot>>

ToggleUnpublished(t) ==
  /\ CanToggleRisk(t)
  /\ unpublished' = [unpublished EXCEPT ![t] = ~unpublished[t]]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, taskRec,
               hold, dirty, finishRunning, prepareLock, bPc, bSlot,
               bClaimsSnap, bRuns, rPc, rSlot, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot>>

ToggleFinishRunning(t) ==
  /\ CanToggleRisk(t)
  /\ finishRunning' = [finishRunning EXCEPT ![t] = ~finishRunning[t]]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, taskRec,
               hold, dirty, unpublished, prepareLock, bPc, bSlot,
               bClaimsSnap, bRuns, rPc, rSlot, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot>>

(* --- Terminal stuttering state --- *)

Done ==
  /\ AllProcsIdle
  /\ \A p \in Procs : bRuns[p] = MaxCycles
  /\ \A t \in Tasks :
       IF AllowRetire THEN ~taskRec[t].exists
       ELSE IF AllowBaseError THEN TRUE
       ELSE (taskRec[t].exists /\ taskRec[t].stage = "ready")
  /\ (~AllowPrepare \/ pRan \/ (\A n \in Slots : slotClaim[n] # "none"))
  /\ UNCHANGED vars

Next ==
  \/ \E p \in Procs :
       \/ BeginStartScan(p)
       \/ BeginCreateClaim(p)
       \/ BeginWriteTask(p)
       \/ BeginBaseError(p)
       \/ BeginEnsureCheckout(p)
       \/ BeginSwitchAndReady(p)
  \/ \E t \in Tasks :
       \/ RetireCheckAndClean(t)
       \/ RetireRemoveTask(t)
       \/ RetireRemoveClaim(t)
       \/ RetireCrashAfterTaskDelete(t)
       \/ ToggleHold(t)
       \/ ToggleDirty(t)
       \/ ToggleUnpublished(t)
       \/ ToggleFinishRunning(t)
  \/ PrepareClaimSlot
  \/ PrepareFinish
  \/ PrepareCrash
  \/ Done

Spec == Init /\ [][Next]_vars

(* --- Invariants (Rules) --- *)

\* No two live tasks share a slot, and prepare_slot never acts on a claimed slot.
ExclusiveSlotOwnership ==
  /\ ~prepareOverwroteSlot
  /\ \A t1, t2 \in Tasks :
       (taskRec[t1].exists /\ taskRec[t2].exists /\ t1 # t2)
       => taskRec[t1].slot # taskRec[t2].slot

\* Every live tasks/<t>.json points to a slot whose .claim is t.
TaskSlotMatchesClaim ==
  \A t \in Tasks :
    taskRec[t].exists => slotClaim[taskRec[t].slot] = t

\* No task holds more than one slots/<n>.claim simultaneously.
AtMostOneClaimPerTask ==
  \A t \in Tasks :
    Cardinality({ n \in Slots : slotClaim[n] = t }) <= 1

\* Whenever prepare_slot is not running, no #prepare claim remains on disk.
NoStalePrepareClaimWhenIdle ==
  ~prepareLock => \A n \in Slots : slotClaim[n] # "#prepare"

\* Whenever all begin/retire/prepare actors are idle, every .claim belongs to a live task at that slot.
NoOrphanClaimWhenIdle ==
  AllProcsIdle =>
    \A n \in Slots :
      slotClaim[n] # "none" =>
        \E t \in Tasks : slotClaim[n] = t /\ taskRec[t].exists /\ taskRec[t].slot = n

\* retire_task() never proceeds on a held, dirty, unpublished, or finishing task.
NoRetireUnsafeTask ==
  \A t \in Tasks :
    retiredSnap[t].checked =>
      /\ retiredSnap[t].notHeld
      /\ retiredSnap[t].clean
      /\ retiredSnap[t].published
      /\ retiredSnap[t].notFinishing

\* Every Ready task (not mid-retire) has its slot checkout on its own branch.
SlotBranchMatchesReadyTask ==
  \A t \in Tasks :
    (taskRec[t].exists /\ taskRec[t].stage = "ready" /\ rPc[t] = "idle")
    => slotBranch[taskRec[t].slot] = t
=============================================================================
