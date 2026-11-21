----------------------------- MODULE SlotClaims ------------------------------
(* Models slot & claim management across wl/src/commands.rs, wl/src/store.rs,
   and wl/src/finish.rs (housekeeping) at commit 0149b853f6f8611cb12b4f06cb83215fbc8e45d7:
   - begin()          : commands.rs:170-237, lock_task:48-50, resolve_base before claim_slot:194-196,
                        claim_slot/claim_new_slot:100-132, ensure_slot_checkout:135-147, switch_slot:240-269
   - retire()         : commands.rs:279-362 (lock_task + retirement_lock + retirement_order + remove order)
   - prepare_slot()   : commands.rs:449-490 (reclaim stale/orphan claims + error-safe #prepare cleanup)
   - housekeeping()   : finish.rs:213-225 -> reclaim_orphan_claims:commands.rs:85-97
   - Store primitives : store.rs:23,108-156 (retirement_lock, create_new, create_new_json, write_json, remove, lock_exclusive, try_lock_exclusive)
*)
EXTENDS Integers, FiniteSets

CONSTANTS
  Tasks,               \* e.g. {"a"} or {"a", "b"}
  Slots,               \* e.g. {1, 2} or {1, 2, 3}
  InitFreeSlot1,       \* BOOLEAN: whether slot 1 starts prepared and free
  MaxCycles,           \* Nat: max begin attempts per begin actor
  AllowPrepare,        \* BOOLEAN: enable prepare_slot() background actor
  AllowRetire,         \* BOOLEAN: enable retire_task()
  AllowRiskStates,     \* BOOLEAN: enable hold/dirty/unpublished/finishRunning toggles
  AllowBaseError,      \* BOOLEAN: model resolve_base() failure during begin()
  AllowPrepareCrash,   \* BOOLEAN: model crash/error while #prepare claim is held
  AllowRetireCrash,    \* BOOLEAN: model crash between remove(task) and remove(claim)
  AllowSameTaskRace,   \* BOOLEAN: two concurrent `wl begin a` invocations
  AllowRetireClaimed   \* BOOLEAN: retire_task() may run concurrently with begin() (serialized by task_lock)

VARIABLES
  slotJson,            \* [Slots -> BOOLEAN]: slots/<n>.json exists
  slotWorktree,        \* [Slots -> BOOLEAN]: product.slots/<n> checkout exists
  slotBranch,          \* [Slots -> {"none", "detached"} \cup Tasks]
  slotClaim,           \* [Slots -> {"none", "#prepare"} \cup Tasks]
  retireOrder,         \* [Slots -> Nat]: SlotRecord.retirement_order (0 = None)
  taskRec,             \* [Tasks -> [exists: BOOLEAN, slot: {0} \cup Slots, stage: {"none", "claimed", "ready"}]]
  hold,                \* [Tasks -> BOOLEAN]
  dirty,               \* [Tasks -> BOOLEAN]
  unpublished,         \* [Tasks -> BOOLEAN]
  finishRunning,       \* [Tasks -> BOOLEAN]
  prepareLock,         \* BOOLEAN: workers/prepare.lock held
  retireLock,          \* {"none"} \cup {"a1", "a2", "b1", "a", "b"}: slots/retirement.lock owner
  taskLock,            \* [Tasks -> {"none"} \cup {"a1", "a2", "b1", "retire"}]
  bPc,                 \* [Procs -> {"idle", "locked", "scannedClaims", "claimed", "taskWritten", "checkoutReady"}]
  bSlot,               \* [Procs -> {0} \cup Slots]
  bClaimsSnap,         \* [Procs -> SUBSET Slots]
  bRuns,               \* [Procs -> 0..MaxCycles]
  rPc,                 \* [Tasks -> {"idle", "checked", "orderRead", "taskDeleted"}]
  rSlot,               \* [Tasks -> {0} \cup Slots]
  rLastOrder,          \* [Tasks -> Nat]
  pPc,                 \* "idle" | "claimedExisting" | "claimedNew"
  pSlot,               \* {0} \cup Slots
  pRan,                \* BOOLEAN: prepare_slot() has completed or crashed once
  retiredSnap,         \* [Tasks -> [checked: BOOLEAN, notHeld: BOOLEAN, clean: BOOLEAN, published: BOOLEAN, notFinishing: BOOLEAN]]
  prepareOverwroteSlot,\* BOOLEAN: ghost flag set if prepare_slot overwrites a claimed/live slot
  badHousekeeping,     \* BOOLEAN: ghost flag if housekeeping() failed to reclaim an idle orphan/#prepare claim
  badReuseOrder        \* BOOLEAN: ghost flag if claim_slot() skipped earliest retired slot

vars == <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder, taskRec,
          hold, dirty, unpublished, finishRunning, prepareLock, retireLock,
          taskLock, bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot, rLastOrder,
          pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot,
          badHousekeeping, badReuseOrder>>

Procs ==
  IF AllowSameTaskRace THEN {"a1", "a2"}
  ELSE IF "b" \in Tasks THEN {"a1", "b1"}
  ELSE {"a1"}

OwnerOf(p) == IF p \in {"a1", "a2"} THEN "a" ELSE "b"

SlotOrZero == {0} \cup Slots
NoTaskRec == [exists |-> FALSE, slot |-> 0, stage |-> "none"]
InitSnap == [checked |-> FALSE, notHeld |-> TRUE, clean |-> TRUE, published |-> TRUE, notFinishing |-> TRUE]

MinNat(S) == CHOOSE x \in S : \A y \in S : x <= y

MaxOrder ==
  IF \A s \in Slots : retireOrder[s] = 0
  THEN 0
  ELSE CHOOSE m \in { retireOrder[s] : s \in Slots } : \A s \in Slots : retireOrder[s] <= m

SlotBetter(a, b) ==
  IF (retireOrder[a] = 0) # (retireOrder[b] = 0)
  THEN retireOrder[a] > 0
  ELSE IF retireOrder[a] # retireOrder[b]
  THEN retireOrder[a] < retireOrder[b]
  ELSE a < b

BestFreeSlot(S) == CHOOSE x \in S : \A y \in S : (y = x) \/ SlotBetter(x, y)

AllProcsIdle ==
  /\ \A p \in Procs : bPc[p] = "idle"
  /\ \A t \in Tasks : rPc[t] = "idle"
  /\ pPc = "idle"

CandidateTaskSlots ==
  { n \in Slots : slotClaim[n] \in Tasks /\ ~taskRec[slotClaim[n]].exists }

ReclaimableTaskSlots ==
  { n \in CandidateTaskSlots : taskLock[slotClaim[n]] = "none" }

ReclaimablePrepareSlots ==
  { n \in Slots : ~prepareLock /\ slotClaim[n] = "#prepare" }

HasReclaimable == ReclaimableTaskSlots # {} \/ ReclaimablePrepareSlots # {}

Init ==
  /\ slotJson = [n \in Slots |-> (InitFreeSlot1 /\ n = 1)]
  /\ slotWorktree = [n \in Slots |-> (InitFreeSlot1 /\ n = 1)]
  /\ slotBranch = [n \in Slots |-> IF InitFreeSlot1 /\ n = 1 THEN "detached" ELSE "none"]
  /\ slotClaim = [n \in Slots |-> "none"]
  /\ retireOrder = [n \in Slots |-> 0]
  /\ taskRec = [t \in Tasks |-> NoTaskRec]
  /\ hold = [t \in Tasks |-> FALSE]
  /\ dirty = [t \in Tasks |-> FALSE]
  /\ unpublished = [t \in Tasks |-> FALSE]
  /\ finishRunning = [t \in Tasks |-> FALSE]
  /\ prepareLock = FALSE
  /\ retireLock = "none"
  /\ taskLock = [t \in Tasks |-> "none"]
  /\ bPc = [p \in Procs |-> "idle"]
  /\ bSlot = [p \in Procs |-> 0]
  /\ bClaimsSnap = [p \in Procs |-> {}]
  /\ bRuns = [p \in Procs |-> 0]
  /\ rPc = [t \in Tasks |-> "idle"]
  /\ rSlot = [t \in Tasks |-> 0]
  /\ rLastOrder = [t \in Tasks |-> 0]
  /\ pPc = "idle"
  /\ pSlot = 0
  /\ pRan = FALSE
  /\ retiredSnap = [t \in Tasks |-> InitSnap]
  /\ prepareOverwroteSlot = FALSE
  /\ badHousekeeping = FALSE
  /\ badReuseOrder = FALSE

(* --- begin() acquires ws.lock_task(&task) (commands.rs:188, 48-50) --- *)

BeginAcquireLock(p) ==
  /\ bPc[p] = "idle"
  /\ bRuns[p] < MaxCycles
  /\ LET t == OwnerOf(p)
     IN /\ IF TRUE \* Mutant hook: begin() acquires task_lock before checking task record
           THEN /\ taskLock[t] = "none"
                /\ taskLock' = [taskLock EXCEPT ![t] = p]
           ELSE UNCHANGED taskLock
        /\ bPc' = [bPc EXCEPT ![p] = "locked"]
        /\ bRuns' = [bRuns EXCEPT ![p] = bRuns[p] + 1]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               taskRec, hold, dirty, unpublished, finishRunning, prepareLock,
               retireLock, bSlot, bClaimsSnap, rPc, rSlot, rLastOrder,
               pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot,
               badHousekeeping, badReuseOrder>>

(* --- begin(): resolve_base() BEFORE claim_slot(), then read task or scan claims (commands.rs:190-200) --- *)

BeginResolveAndScan(p) ==
  /\ bPc[p] = "locked"
  /\ LET t == OwnerOf(p)
         ownClaimed == { n \in Slots : slotClaim[n] = t }
     IN IF taskRec[t].exists
        THEN IF taskRec[t].stage = "ready"
             THEN /\ bPc' = [bPc EXCEPT ![p] = "idle"]
                  /\ taskLock' = [taskLock EXCEPT ![t] = IF taskLock[t] = p THEN "none" ELSE taskLock[t]]
                  /\ UNCHANGED <<bSlot, bClaimsSnap, retireLock>>
             ELSE /\ bPc' = [bPc EXCEPT ![p] = "taskWritten"]
                  /\ bSlot' = [bSlot EXCEPT ![p] = taskRec[t].slot]
                  /\ UNCHANGED <<bClaimsSnap, taskLock, retireLock>>
        ELSE /\ IF TRUE \* Mutant hook: claim_slot() acquires retirement_lock
                THEN retireLock = "none"
                ELSE TRUE
             /\ IF ownClaimed # {}
                THEN /\ bSlot' = [bSlot EXCEPT ![p] = MinNat(ownClaimed)]
                     /\ bPc' = [bPc EXCEPT ![p] = "claimed"]
                     /\ UNCHANGED <<bClaimsSnap, taskLock, retireLock>>
                ELSE /\ bClaimsSnap' = [bClaimsSnap EXCEPT ![p] = { n \in Slots : slotClaim[n] # "none" }]
                     /\ bPc' = [bPc EXCEPT ![p] = "scannedClaims"]
                     /\ retireLock' = IF retireLock = "none" THEN p ELSE retireLock
                     /\ UNCHANGED <<bSlot, taskLock>>
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               taskRec, hold, dirty, unpublished, finishRunning, prepareLock,
               bRuns, rPc, rSlot, rLastOrder, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot, badHousekeeping, badReuseOrder>>

(* resolve_base() at commands.rs:194 fails BEFORE claim_slot() at line 196 *)
BeginBaseError(p) ==
  /\ AllowBaseError
  /\ bPc[p] = "locked"
  /\ ~taskRec[OwnerOf(p)].exists
  /\ LET t == OwnerOf(p)
         freeExisting == { n \in Slots : slotJson[n] /\ slotClaim[n] = "none" }
         uncreated == { n \in Slots : ~slotJson[n] /\ slotClaim[n] = "none" }
     IN IF TRUE \* Mutant hook: resolve_base() fails BEFORE calling claim_slot()
        THEN UNCHANGED slotClaim
        ELSE LET chosen == IF freeExisting # {} THEN BestFreeSlot(freeExisting) ELSE MinNat(uncreated)
             IN slotClaim' = [slotClaim EXCEPT ![chosen] = t]
  /\ taskLock' = [taskLock EXCEPT ![OwnerOf(p)] = IF taskLock[OwnerOf(p)] = p THEN "none" ELSE taskLock[OwnerOf(p)]]
  /\ bPc' = [bPc EXCEPT ![p] = "idle"]
  /\ bSlot' = [bSlot EXCEPT ![p] = 0]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, retireOrder, taskRec,
               hold, dirty, unpublished, finishRunning, prepareLock,
               retireLock, bClaimsSnap, bRuns, rPc, rSlot, rLastOrder,
               pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot,
               badHousekeeping, badReuseOrder>>

(* --- claim_slot() / claim_new_slot(): atomic Store::create_new (commands.rs:100-132) --- *)

BeginCreateClaim(p) ==
  /\ bPc[p] = "scannedClaims"
  /\ LET t == OwnerOf(p)
         freeExisting == { n \in Slots : slotJson[n] /\ n \notin bClaimsSnap[p] }
         uncreated == { n \in Slots : ~slotJson[n] /\ slotClaim[n] = "none" }
         bestFree == IF freeExisting # {} THEN BestFreeSlot(freeExisting) ELSE 0
         pickedFree == IF freeExisting # {}
                       THEN IF TRUE \* Mutant hook: claim_slot() picks earliest retirement_order first
                            THEN bestFree
                            ELSE MinNat(freeExisting)
                       ELSE 0
         chosen == IF freeExisting # {}
                   THEN pickedFree
                   ELSE IF uncreated # {}
                   THEN MinNat(uncreated)
                   ELSE 0
     IN /\ chosen # 0
        /\ IF slotClaim[chosen] = "none"
           THEN /\ slotClaim' = [slotClaim EXCEPT ![chosen] = t]
                /\ bSlot' = [bSlot EXCEPT ![p] = chosen]
                /\ bPc' = [bPc EXCEPT ![p] = "claimed"]
                /\ badReuseOrder' = (badReuseOrder \/ (freeExisting # {} /\ chosen # bestFree))
                /\ retireLock' = IF retireLock = p THEN "none" ELSE retireLock
                /\ UNCHANGED bClaimsSnap
           ELSE /\ bClaimsSnap' = [bClaimsSnap EXCEPT ![p] = bClaimsSnap[p] \cup {chosen}]
                /\ UNCHANGED <<slotClaim, bSlot, bPc, retireLock, badReuseOrder>>
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, retireOrder, taskRec,
               hold, dirty, unpublished, finishRunning, prepareLock, taskLock,
               bRuns, rPc, rSlot, rLastOrder, pPc, pSlot, pRan, retiredSnap,
               prepareOverwroteSlot, badHousekeeping>>

(* --- begin(): Store::create_new_json(task_path) (commands.rs:198-216) --- *)

BeginWriteTask(p) ==
  /\ bPc[p] = "claimed"
  /\ LET t == OwnerOf(p)
         n == bSlot[p]
         fresh == [exists |-> TRUE, slot |-> n, stage |-> "claimed"]
     IN IF ~taskRec[t].exists
        THEN /\ taskRec' = [taskRec EXCEPT ![t] = fresh]
             /\ bPc' = [bPc EXCEPT ![p] = "taskWritten"]
             /\ UNCHANGED <<bSlot, taskLock>>
        ELSE IF taskRec[t].stage = "ready"
             THEN /\ bPc' = [bPc EXCEPT ![p] = "idle"]
                  /\ bSlot' = [bSlot EXCEPT ![p] = 0]
                  /\ taskLock' = [taskLock EXCEPT ![t] = IF taskLock[t] = p THEN "none" ELSE taskLock[t]]
                  /\ UNCHANGED taskRec
             ELSE /\ bSlot' = [bSlot EXCEPT ![p] = taskRec[t].slot]
                  /\ bPc' = [bPc EXCEPT ![p] = "taskWritten"]
                  /\ UNCHANGED <<taskRec, taskLock>>
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               hold, dirty, unpublished, finishRunning, prepareLock, retireLock,
               bClaimsSnap, bRuns, rPc, rSlot, rLastOrder, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot, badHousekeeping, badReuseOrder>>

(* --- ensure_slot_checkout() (commands.rs:226, 135-147) --- *)

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
  /\ UNCHANGED <<slotClaim, retireOrder, taskRec, hold, dirty, unpublished,
               finishRunning, prepareLock, retireLock, taskLock, bSlot,
               bClaimsSnap, bRuns, rPc, rSlot, rLastOrder, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot, badHousekeeping, badReuseOrder>>

(* --- switch_slot() + write_json(Ready), then drop _task_lock (commands.rs:228-236) --- *)

BeginSwitchAndReady(p) ==
  /\ bPc[p] = "checkoutReady"
  /\ LET t == OwnerOf(p)
         n == bSlot[p]
     IN /\ ~dirty[t]
        /\ IF TRUE \* Mutant hook: skip switch_slot before writing Ready
           THEN slotBranch' = [slotBranch EXCEPT ![n] = t]
           ELSE UNCHANGED slotBranch
        /\ taskRec' = [taskRec EXCEPT ![t] = [exists |-> TRUE, slot |-> n, stage |-> "ready"]]
        /\ taskLock' = [taskLock EXCEPT ![t] = IF taskLock[t] = p THEN "none" ELSE taskLock[t]]
        /\ bPc' = [bPc EXCEPT ![p] = "idle"]
        /\ bSlot' = [bSlot EXCEPT ![p] = 0]
  /\ UNCHANGED <<slotJson, slotWorktree, slotClaim, retireOrder, hold, dirty,
               unpublished, finishRunning, prepareLock, retireLock,
               bClaimsSnap, bRuns, rPc, rSlot, rLastOrder, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot, badHousekeeping, badReuseOrder>>

(* --- retire_task() acquires _task_lock, checks & cleans (commands.rs:279-333) --- *)

RetireCheckAndClean(t) ==
  /\ AllowRetire
  /\ rPc[t] = "idle"
  /\ IF TRUE \* Mutant hook: retire_task() acquires task_lock
     THEN taskLock[t] = "none"
     ELSE TRUE
  /\ IF ~taskRec[t].exists
     THEN \* release_claims_for(t) when task_path is absent (commands.rs:282-292)
          /\ \E n \in Slots : slotClaim[n] = t
          /\ slotClaim' = [n \in Slots |-> IF slotClaim[n] = t THEN "none" ELSE slotClaim[n]]
          /\ UNCHANGED <<taskLock, retiredSnap, slotBranch, rSlot, rPc,
                       hold, dirty, unpublished, finishRunning>>
     ELSE /\ (AllowRetireClaimed \/ taskRec[t].stage = "ready")
          /\ ~hold[t]
          /\ ~finishRunning[t]
          /\ ~dirty[t]
          /\ ~unpublished[t]
          /\ LET n == taskRec[t].slot
             IN /\ taskLock' = [taskLock EXCEPT ![t] = "retire"]
                /\ retiredSnap' = [retiredSnap EXCEPT ![t] =
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
                /\ UNCHANGED slotClaim
  /\ UNCHANGED <<slotJson, slotWorktree, retireOrder, taskRec, prepareLock,
               retireLock, bPc, bSlot, bClaimsSnap, bRuns, rLastOrder,
               pPc, pSlot, pRan, prepareOverwroteSlot, badHousekeeping, badReuseOrder>>

(* --- retire_task() acquires retirement_lock and scans existing retirement_order (commands.rs:337-348) --- *)

RetireReadOrder(t) ==
  /\ rPc[t] = "checked"
  /\ IF TRUE \* Mutant hook: retire_task() acquires retirement_lock
     THEN /\ retireLock = "none"
          /\ retireLock' = t
     ELSE UNCHANGED retireLock
  /\ rLastOrder' = [rLastOrder EXCEPT ![t] = MaxOrder]
  /\ rPc' = [rPc EXCEPT ![t] = "orderRead"]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               taskRec, hold, dirty, unpublished, finishRunning, prepareLock,
               taskLock, bPc, bSlot, bClaimsSnap, bRuns, rSlot, pPc, pSlot,
               pRan, retiredSnap, prepareOverwroteSlot, badHousekeeping,
               badReuseOrder>>

(* --- retire_task() writes retirement_order (349), removes task_path (351) then claim (352-354) --- *)

RetireRemoveTask(t) ==
  /\ rPc[t] = "orderRead"
  /\ LET n == rSlot[t]
     IN /\ retireOrder' = IF slotJson[n]
                          THEN [retireOrder EXCEPT ![n] = rLastOrder[t] + 1]
                          ELSE retireOrder
        /\ IF TRUE \* Mutant hook: swap order so claim is removed BEFORE task record
           THEN /\ taskRec' = [taskRec EXCEPT ![t] = NoTaskRec]
                /\ rPc' = [rPc EXCEPT ![t] = "taskDeleted"]
                /\ UNCHANGED slotClaim
           ELSE /\ slotClaim' = [slotClaim EXCEPT ![n] =
                                   IF slotClaim[n] = t THEN "none" ELSE slotClaim[n]]
                /\ rPc' = [rPc EXCEPT ![t] = "taskDeleted"]
                /\ UNCHANGED taskRec
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, hold, dirty, unpublished,
               finishRunning, prepareLock, retireLock, taskLock, bPc, bSlot,
               bClaimsSnap, bRuns, rSlot, rLastOrder, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot, badHousekeeping, badReuseOrder>>

RetireRemoveClaim(t) ==
  /\ rPc[t] = "taskDeleted"
  /\ LET n == rSlot[t]
     IN slotClaim' = [slotClaim EXCEPT ![n] = IF slotClaim[n] = t THEN "none" ELSE slotClaim[n]]
  /\ retireLock' = IF retireLock = t THEN "none" ELSE retireLock
  /\ taskLock' = [taskLock EXCEPT ![t] = IF taskLock[t] = "retire" THEN "none" ELSE taskLock[t]]
  /\ rPc' = [rPc EXCEPT ![t] = "idle"]
  /\ rSlot' = [rSlot EXCEPT ![t] = 0]
  /\ rLastOrder' = [rLastOrder EXCEPT ![t] = 0]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, retireOrder, taskRec,
               hold, dirty, unpublished, finishRunning, prepareLock,
               bPc, bSlot, bClaimsSnap, bRuns, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot, badHousekeeping, badReuseOrder>>

(* Crash after Store::remove(&task_path) line 351 before Store::remove(&claim(n)) line 353; OS drops taskLock & retireLock *)
RetireCrashAfterTaskDelete(t) ==
  /\ AllowRetireCrash
  /\ rPc[t] = "taskDeleted"
  /\ retireLock' = IF retireLock = t THEN "none" ELSE retireLock
  /\ taskLock' = [taskLock EXCEPT ![t] = IF taskLock[t] = "retire" THEN "none" ELSE taskLock[t]]
  /\ rPc' = [rPc EXCEPT ![t] = "idle"]
  /\ rSlot' = [rSlot EXCEPT ![t] = 0]
  /\ rLastOrder' = [rLastOrder EXCEPT ![t] = 0]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               taskRec, hold, dirty, unpublished, finishRunning, prepareLock,
               bPc, bSlot, bClaimsSnap, bRuns, pPc, pSlot, pRan,
               retiredSnap, prepareOverwroteSlot, badHousekeeping, badReuseOrder>>

(* --- prepare_slot() (commands.rs:449-490) --- *)

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
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, retireOrder, taskRec,
               hold, dirty, unpublished, finishRunning, retireLock, taskLock,
               bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot, rLastOrder, pRan,
               retiredSnap, prepareOverwroteSlot, badHousekeeping, badReuseOrder>>

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
  /\ UNCHANGED <<retireOrder, taskRec, hold, dirty, unpublished, finishRunning,
               retireLock, taskLock, bPc, bSlot, bClaimsSnap, bRuns, rPc,
               rSlot, rLastOrder, retiredSnap, badHousekeeping, badReuseOrder>>

(* Crash or error while holding #prepare claim (prepare.lock drops, #prepare stays until housekeeping/prepare_slot) *)
PrepareCrash ==
  /\ AllowPrepareCrash
  /\ pPc \in {"claimedExisting", "claimedNew"}
  /\ prepareLock' = FALSE
  /\ pPc' = "idle"
  /\ pSlot' = 0
  /\ pRan' = TRUE
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               taskRec, hold, dirty, unpublished, finishRunning, retireLock,
               taskLock, bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot,
               rLastOrder, retiredSnap, prepareOverwroteSlot,
               badHousekeeping, badReuseOrder>>

(* --- housekeeping() -> reclaim_orphan_claims() (finish.rs:214, commands.rs:65-97) --- *)

HousekeepingReclaim ==
  /\ (CandidateTaskSlots # {} \/ ReclaimablePrepareSlots # {})
  /\ LET clearTask == IF TRUE \* Mutant hook: check task_lock before reclaiming orphan task claim
                      THEN ReclaimableTaskSlots
                      ELSE { n \in Slots : slotClaim[n] \in Tasks /\ ~taskRec[slotClaim[n]].exists }
         clearPrep == IF TRUE \* Mutant hook: reclaim stale #prepare claims when prepare_lock is free
                      THEN ReclaimablePrepareSlots
                      ELSE {}
         nextClaims == [n \in Slots |->
                          IF n \in (clearTask \cup clearPrep) THEN "none" ELSE slotClaim[n]]
         stillTask == \E n \in Slots : nextClaims[n] \in Tasks /\ ~taskRec[nextClaims[n]].exists /\ taskLock[nextClaims[n]] = "none"
         stillPrep == \E n \in Slots : nextClaims[n] = "#prepare" /\ ~prepareLock
     IN /\ slotClaim' = nextClaims
        /\ badHousekeeping' = (badHousekeeping \/ stillTask \/ stillPrep)
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, retireOrder, taskRec,
               hold, dirty, unpublished, finishRunning, prepareLock, retireLock,
               taskLock, bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot, rLastOrder,
               pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot, badReuseOrder>>

(* --- Risk-state toggles when a task is Ready and not mid-retire --- *)

CanToggleRisk(t) ==
  /\ AllowRiskStates
  /\ taskRec[t].exists
  /\ taskRec[t].stage = "ready"
  /\ rPc[t] = "idle"

ToggleHold(t) ==
  /\ CanToggleRisk(t)
  /\ hold' = [hold EXCEPT ![t] = ~hold[t]]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               taskRec, dirty, unpublished, finishRunning, prepareLock,
               retireLock, taskLock, bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot,
               rLastOrder, pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot,
               badHousekeeping, badReuseOrder>>

ToggleDirty(t) ==
  /\ CanToggleRisk(t)
  /\ dirty' = [dirty EXCEPT ![t] = ~dirty[t]]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               taskRec, hold, unpublished, finishRunning, prepareLock,
               retireLock, taskLock, bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot,
               rLastOrder, pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot,
               badHousekeeping, badReuseOrder>>

ToggleUnpublished(t) ==
  /\ CanToggleRisk(t)
  /\ unpublished' = [unpublished EXCEPT ![t] = ~unpublished[t]]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               taskRec, hold, dirty, finishRunning, prepareLock, retireLock,
               taskLock, bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot, rLastOrder,
               pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot,
               badHousekeeping, badReuseOrder>>

ToggleFinishRunning(t) ==
  /\ CanToggleRisk(t)
  /\ finishRunning' = [finishRunning EXCEPT ![t] = ~finishRunning[t]]
  /\ UNCHANGED <<slotJson, slotWorktree, slotBranch, slotClaim, retireOrder,
               taskRec, hold, dirty, unpublished, prepareLock, retireLock,
               taskLock, bPc, bSlot, bClaimsSnap, bRuns, rPc, rSlot, rLastOrder,
               pPc, pSlot, pRan, retiredSnap, prepareOverwroteSlot,
               badHousekeeping, badReuseOrder>>

(* --- Terminal stuttering state --- *)

Done ==
  /\ AllProcsIdle
  /\ ~HasReclaimable
  /\ retireLock = "none"
  /\ \A t \in Tasks : taskLock[t] = "none"
  /\ \A p \in Procs : bRuns[p] = MaxCycles
  /\ \A t \in Tasks :
       IF AllowRetire THEN ~taskRec[t].exists
       ELSE IF AllowBaseError THEN TRUE
       ELSE (taskRec[t].exists /\ taskRec[t].stage = "ready")
  /\ (~AllowPrepare \/ pRan \/ (\A n \in Slots : slotClaim[n] # "none"))
  /\ UNCHANGED vars

Next ==
  \/ \E p \in Procs :
       \/ BeginAcquireLock(p)
       \/ BeginResolveAndScan(p)
       \/ BeginBaseError(p)
       \/ BeginCreateClaim(p)
       \/ BeginWriteTask(p)
       \/ BeginEnsureCheckout(p)
       \/ BeginSwitchAndReady(p)
  \/ \E t \in Tasks :
       \/ RetireCheckAndClean(t)
       \/ RetireReadOrder(t)
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
  \/ HousekeepingReclaim
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

\* After crash recovery runs housekeeping(), no un-reclaimed orphan or #prepare claim stays on disk.
NoOrphanClaimAfterHousekeeping ==
  /\ ~badHousekeeping
  /\ (AllProcsIdle =>
        \A n \in Slots :
          (slotClaim[n] = "#prepare" => prepareLock) \/ HasReclaimable)

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

\* No two retired slots share the same non-zero retirement_order.
UniqueRetirementOrder ==
  \A s1, s2 \in Slots :
    (s1 # s2 /\ retireOrder[s1] > 0) => retireOrder[s1] # retireOrder[s2]

\* claim_slot() chooses the free slot with earliest retirement_order (preferring retired slots over never-retired ones).
OldestRetiredSlotReusedFirst == ~badReuseOrder
=============================================================================
