---------------------------- MODULE FinishIntegration ----------------------------
(* Formal model of wl/src/finish.rs and retire_task() in wl/src/commands.rs
   at commit a0a389f3312c98f267ea88f2ec5f86c651948651.
   Each action represents one indivisible saved state change, Git operation,
   or remote CI event. *)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS
  Tasks,            \* set of task names, e.g. {"a"} or {"a", "b"}
  Gated,            \* TRUE when .github/required-checks.json is non-empty
  Kills,            \* TRUE when a worker process may die at any step
  SuffixCollision,  \* TRUE when task "a" models "x-b" colliding with "b" on ends_with("-b")
  PreemptAccept,    \* TRUE when accept.json write (line 79) and queue create (line 80) are split
  RetireMode,       \* TRUE when finish may run with --retire and user edits/holds may race
  MaxEdits,         \* 1 for single-task edit/fix tests, 0 for 2-task concurrency checks
  MaxAttempts,      \* bound on finish attempts per task
  AllowExternal     \* TRUE when an external push may advance origin/<target> once

None == "none"

CommitId(t, k) == t \o (IF k = 1 THEN "1" ELSE "2")
TaskCommits == { CommitId(t, k) : t \in Tasks, k \in 1..(1 + MaxEdits) }
TargetCommits == TaskCommits \cup {"ext"}

VARIABLES
  nextTs,           \* next monotonic accepted_at_ms timestamp
  head,             \* head[t]: current commit number 1..(1 + MaxEdits) on task slot branch
  dirty,            \* dirty[t]: uncommitted/untracked changes in slot checkout
  hold,             \* hold[t]: holds/<t>.json exists
  retired,          \* retired[t]: tasks/<t>.json has been deleted by retire_task
  finPhase,         \* "idle" | "queuing" (PreemptAccept only)
  accept,           \* accept[t]: record [exists, attempt, commit, ts, retire]
  prog,             \* prog[t]: persisted progress.json [attempt, stage, baseObj, mergedObj]
  queue,            \* set of [ts |-> Nat, task |-> Tasks]
  wAlive,           \* wAlive[t]: worker process holds workers/<t>.lock
  wPc,              \* worker program counter
  wStage,           \* in-memory progress.stage before Store::write_durable
  wBase,            \* in-memory target_base before write_durable
  wMerged,          \* in-memory merged commit before write_durable
  wFailReason,      \* in-memory error detail before withdraw/write_durable
  inWorktree,       \* set of workers currently reading/writing integrate-<target>
  wtSrc,            \* commit currently checked out in integrate-<target> (TaskCommits \cup {None})
  wtMergedValid,    \* TRUE if wtSrc passed validation in integrate-<target>
  validatedOwn,     \* validatedOwn[c]: c passed validation on its own uncorrupted tree
  validT,           \* validT[c]: whether validation in integrate-<target> passes on c
  conflictC,        \* conflictC[c]: whether merge of c onto moved target conflicts
  preflightBad,     \* whether next preflight rejects
  ciTopic,          \* ciTopic[c] \in {"none", "pending", "success", "failure"}
  ciTarget,         \* ciTarget[m] \in {"none", "pending", "success", "failure"} for pushed target commit
  tRemoteTip,       \* current tip object on origin/<target>
  tRemoteCommits,   \* set of task commits (plus "base", "ext") integrated into origin/<target>
  integratedInfo,   \* integratedInfo[t]: [done |-> BOOLEAN, commit |-> ..., merged |-> ...]
  retiredSnap,      \* retiredSnap[t]: snapshot at retirement time for invariants
  pushedWhileHeld,  \* TRUE if target push ran while hold[t] was TRUE
  badQueueRemove,   \* TRUE if leave_queue deleted another active task's queue entry
  extPushed         \* TRUE once ExternalTargetPush has fired

vars == <<nextTs, head, dirty, hold, retired, finPhase, accept, prog, queue,
          wAlive, wPc, wStage, wBase, wMerged, wFailReason, inWorktree, wtSrc,
          wtMergedValid, validatedOwn, validT, conflictC, preflightBad, ciTopic,
          ciTarget, tRemoteTip, tRemoteCommits, integratedInfo, retiredSnap,
          pushedWhileHeld, badQueueRemove, extPushed>>

BaseObj == [kind |-> "base", src |-> None, base |-> None]
ExtObj == [kind |-> "ext", src |-> "ext", base |-> None]
NoAccept == [exists |-> FALSE, attempt |-> 0, commit |-> None, ts |-> 0, retire |-> FALSE]
NoProg == [attempt |-> 0, stage |-> "none", baseObj |-> BaseObj, mergedObj |-> BaseObj]

QueueMatches(t, entryTask) ==
  \/ entryTask = t
  \/ (SuffixCollision /\ t = "b" /\ entryTask = "a")

IsTerminal(stage, ret) ==
  \/ stage \in {"retired", "needs-action"}
  \/ (stage = "integrated" /\ ~ret)

Running(t) ==
  /\ accept[t].exists
  /\ ~(prog[t].attempt = accept[t].attempt /\ IsTerminal(prog[t].stage, accept[t].retire))

MinQueueEntry(q) ==
  CHOOSE e \in q : \A o \in q : e.ts <= o.ts

Published(c) == c \in tRemoteCommits

AllDone ==
  \A t \in Tasks :
    /\ finPhase[t] = "idle"
    /\ ~wAlive[t]
    /\ (retired[t] \/ (accept[t].exists /\ prog[t].attempt = accept[t].attempt
                       /\ prog[t].stage \in {"integrated", "needs-action"}))

Init ==
  /\ nextTs = 1
  /\ head = [t \in Tasks |-> 1]
  /\ dirty = [t \in Tasks |-> FALSE]
  /\ hold = [t \in Tasks |-> FALSE]
  /\ retired = [t \in Tasks |-> FALSE]
  /\ finPhase = [t \in Tasks |-> "idle"]
  /\ accept = [t \in Tasks |-> NoAccept]
  /\ prog = [t \in Tasks |-> NoProg]
  /\ queue = {}
  /\ wAlive = [t \in Tasks |-> FALSE]
  /\ wPc = [t \in Tasks |-> "idle"]
  /\ wStage = [t \in Tasks |-> "none"]
  /\ wBase = [t \in Tasks |-> BaseObj]
  /\ wMerged = [t \in Tasks |-> BaseObj]
  /\ wFailReason = [t \in Tasks |-> None]
  /\ inWorktree = {}
  /\ wtSrc = None
  /\ wtMergedValid = FALSE
  /\ validatedOwn = [c \in TaskCommits |-> FALSE]
  /\ validT \in [TaskCommits -> BOOLEAN]
  /\ conflictC = [c \in TaskCommits |-> FALSE]
  /\ preflightBad = FALSE
  /\ ciTopic = [c \in TaskCommits |-> None]
  /\ ciTarget = [m \in TargetCommits |-> None]
  /\ tRemoteTip = BaseObj
  /\ tRemoteCommits = {"base"}
  /\ integratedInfo = [t \in Tasks |-> [done |-> FALSE, commit |-> None, merged |-> BaseObj]]
  /\ retiredSnap = [t \in Tasks |-> [checked |-> FALSE, clean |-> TRUE, notHeld |-> TRUE, published |-> TRUE]]
  /\ pushedWhileHeld = FALSE
  /\ badQueueRemove = FALSE
  /\ extPushed = FALSE

(* --- Foreground finish acceptance: wl/src/finish.rs lines 24-90 --- *)

FinishWriteAccept(t) ==
  /\ ~retired[t]
  /\ finPhase[t] = "idle"
  /\ ~hold[t]
  /\ ~Running(t)
  /\ accept[t].attempt < MaxAttempts
  /\ LET cur == CommitId(t, head[t])
         retOptions == IF RetireMode THEN BOOLEAN ELSE {FALSE}
     IN \E ret \in retOptions :
          /\ ~(accept[t].exists /\ prog[t].attempt = accept[t].attempt
               /\ prog[t].stage = "integrated" /\ accept[t].commit = cur /\ ~ret)
          /\ accept' = [accept EXCEPT ![t] = [exists |-> TRUE,
                                              attempt |-> accept[t].attempt + 1,
                                              commit |-> cur,
                                              ts |-> nextTs,
                                              retire |-> ret]]
          /\ nextTs' = nextTs + 1
          /\ IF PreemptAccept
             THEN /\ finPhase' = [finPhase EXCEPT ![t] = "queuing"]
                  /\ UNCHANGED queue
             ELSE /\ queue' = queue \cup {[ts |-> nextTs, task |-> t]}
                  /\ UNCHANGED finPhase
  /\ UNCHANGED <<head, dirty, hold, retired, prog, wAlive, wPc, wStage, wBase,
               wMerged, wFailReason, inWorktree, wtSrc, wtMergedValid,
               validatedOwn, validT, conflictC, preflightBad, ciTopic, ciTarget,
               tRemoteTip, tRemoteCommits, integratedInfo, retiredSnap,
               pushedWhileHeld, badQueueRemove, extPushed>>

FinishEnqueue(t) ==
  /\ PreemptAccept
  /\ finPhase[t] = "queuing"
  /\ finPhase' = [finPhase EXCEPT ![t] = "idle"]
  /\ queue' = queue \cup {[ts |-> accept[t].ts, task |-> t]}
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, accept, prog, wAlive, wPc,
               wStage, wBase, wMerged, wFailReason, inWorktree, wtSrc,
               wtMergedValid, validatedOwn, validT, conflictC, preflightBad,
               ciTopic, ciTarget, tRemoteTip, tRemoteCommits, integratedInfo,
               retiredSnap, pushedWhileHeld, badQueueRemove, extPushed>>

(* --- Worker launch / housekeeping restart: wl/src/finish.rs lines 118-124, 152-176 --- *)

WorkerStart(t) ==
  /\ ~retired[t]
  /\ ~wAlive[t]
  /\ Running(t)
  /\ LET a == accept[t]
         hasProg == prog[t].attempt = a.attempt
         st == IF hasProg THEN prog[t].stage ELSE "accepted"
     IN /\ wAlive' = [wAlive EXCEPT ![t] = TRUE]
        /\ wStage' = [wStage EXCEPT ![t] = st]
        /\ wBase' = [wBase EXCEPT ![t] = IF hasProg THEN prog[t].baseObj ELSE BaseObj]
        /\ wMerged' = [wMerged EXCEPT ![t] = IF hasProg THEN prog[t].mergedObj ELSE BaseObj]
        /\ wPc' = [wPc EXCEPT ![t] =
             CASE st = "accepted" -> "stepAccepted"
               [] st = "topic-pushed" -> "waitTurn"
               [] st = "prepared" -> "checkTopicCI"
               [] st = "target-pushed" -> "waitTargetCI"
               [] st = "integrated" -> "doRetire"
               [] OTHER -> "idle"]
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               queue, wFailReason, inWorktree, wtSrc, wtMergedValid,
               validatedOwn, validT, conflictC, preflightBad, ciTopic, ciTarget,
               tRemoteTip, tRemoteCommits, integratedInfo, retiredSnap,
               pushedWhileHeld, badQueueRemove, extPushed>>

(* --- Stage::Accepted -> TopicPushed: finish.rs lines 212-219, 266-271 --- *)

WorkerPushTopic(t) ==
  /\ wAlive[t] /\ wPc[t] = "stepAccepted"
  /\ LET c == accept[t].commit
     IN IF preflightBad
        THEN /\ wPc' = [wPc EXCEPT ![t] = "withdrawErr"]
             /\ wFailReason' = [wFailReason EXCEPT ![t] = "preflight"]
             /\ UNCHANGED <<ciTopic, wStage>>
        ELSE /\ ciTopic' = [ciTopic EXCEPT ![c] =
                              IF Gated /\ ciTopic[c] = None THEN "pending" ELSE ciTopic[c]]
             /\ wStage' = [wStage EXCEPT ![t] = "topic-pushed"]
             /\ wPc' = [wPc EXCEPT ![t] = "writeStage"]
             /\ UNCHANGED wFailReason
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               queue, wAlive, wBase, wMerged, inWorktree, wtSrc, wtMergedValid,
               validatedOwn, validT, conflictC, preflightBad, ciTarget,
               tRemoteTip, tRemoteCommits, integratedInfo, retiredSnap,
               pushedWhileHeld, badQueueRemove, extPushed>>

(* --- wait_for_turn(): finish.rs lines 279-301 --- *)

WorkerWaitTurn(t) ==
  /\ wAlive[t] /\ wPc[t] = "waitTurn"
  /\ IF ~\E e \in queue : QueueMatches(t, e.task)
     THEN /\ queue' = queue \cup {[ts |-> accept[t].ts, task |-> t]}
          /\ UNCHANGED <<wPc, inWorktree>>
     ELSE LET first == MinQueueEntry(queue) IN
          IF QueueMatches(t, first.task)
          THEN /\ wPc' = [wPc EXCEPT ![t] = "prepMerge"]
               /\ inWorktree' = inWorktree \cup {t}
               /\ UNCHANGED queue
          ELSE IF ~Running(first.task)
               THEN /\ queue' = queue \ {first}
                    /\ UNCHANGED <<wPc, inWorktree>>
               ELSE UNCHANGED <<queue, wPc, inWorktree>>
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               wAlive, wStage, wBase, wMerged, wFailReason, wtSrc,
               wtMergedValid, validatedOwn, validT, conflictC, preflightBad,
               ciTopic, ciTarget, tRemoteTip, tRemoteCommits, integratedInfo,
               retiredSnap, pushedWhileHeld, badQueueRemove, extPushed>>

(* --- prepare() lines 337-362: fetch target, fast-forward or merge in integrate-<target> --- *)

WorkerPrepareMerge(t) ==
  /\ wAlive[t] /\ wPc[t] = "prepMerge"
  /\ LET c == accept[t].commit
     IN IF c \in tRemoteCommits
        THEN \* Already in target; validation skipped (finish.rs:345-348)
             /\ wBase' = [wBase EXCEPT ![t] = tRemoteTip]
             /\ wMerged' = [wMerged EXCEPT ![t] = tRemoteTip]
             /\ wStage' = [wStage EXCEPT ![t] = "prepared"]
             /\ wPc' = [wPc EXCEPT ![t] = "writeStage"]
             /\ inWorktree' = inWorktree \ {t}
             /\ UNCHANGED <<wtSrc, wtMergedValid, wFailReason>>
        ELSE IF conflictC[c] /\ tRemoteTip # BaseObj
        THEN \* Merge conflict -> Err -> withdraw()
             /\ wFailReason' = [wFailReason EXCEPT ![t] = "conflict"]
             /\ wPc' = [wPc EXCEPT ![t] = "withdrawErr"]
             /\ inWorktree' = inWorktree \ {t}
             /\ UNCHANGED <<wBase, wMerged, wStage, wtSrc, wtMergedValid>>
        ELSE LET m == IF tRemoteTip = BaseObj
                      THEN [kind |-> "ff", src |-> c, base |-> "base"]
                      ELSE [kind |-> "merge", src |-> c, base |-> tRemoteTip.src]
             IN /\ wBase' = [wBase EXCEPT ![t] = tRemoteTip]
                /\ wMerged' = [wMerged EXCEPT ![t] = m]
                /\ wtSrc' = c
                /\ wtMergedValid' = validT[c]
                /\ wPc' = [wPc EXCEPT ![t] = "prepValidate"]
                /\ UNCHANGED <<wStage, wFailReason, inWorktree>>
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               queue, wAlive, validatedOwn, validT, conflictC, preflightBad,
               ciTopic, ciTarget, tRemoteTip, tRemoteCommits, integratedInfo,
               retiredSnap, pushedWhileHeld, badQueueRemove, extPushed>>

(* --- prepare() lines 363-384: validation runs on whatever is currently in integrate-<target> --- *)

WorkerPrepareValidate(t) ==
  /\ wAlive[t] /\ wPc[t] = "prepValidate"
  /\ LET c == accept[t].commit
     IN IF wtMergedValid
        THEN /\ validatedOwn' = IF wtSrc = c
                                THEN [validatedOwn EXCEPT ![c] = TRUE]
                                ELSE validatedOwn
             /\ wStage' = [wStage EXCEPT ![t] = "prepared"]
             /\ wPc' = [wPc EXCEPT ![t] = "writeStage"]
             /\ inWorktree' = inWorktree \ {t}
             /\ UNCHANGED wFailReason
        ELSE /\ wFailReason' = [wFailReason EXCEPT ![t] = "validation"]
             /\ wPc' = [wPc EXCEPT ![t] = "withdrawErr"]
             /\ inWorktree' = inWorktree \ {t}
             /\ UNCHANGED <<validatedOwn, wStage>>
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               queue, wAlive, wBase, wMerged, wtSrc, wtMergedValid, validT,
               conflictC, preflightBad, ciTopic, ciTarget, tRemoteTip,
               tRemoteCommits, integratedInfo, retiredSnap, pushedWhileHeld,
               badQueueRemove, extPushed>>

(* --- push_target() lines 418-424: wait_for_checks(commit) and HoldRecord check --- *)

WorkerCheckTopicCI(t) ==
  /\ wAlive[t] /\ wPc[t] = "checkTopicCI"
  /\ LET c == accept[t].commit
     IN IF Gated
        THEN CASE ciTopic[c] = "success" ->
                    /\ wPc' = [wPc EXCEPT ![t] = "pushTargetExec"]
                    /\ inWorktree' = inWorktree \cup {t}
                    /\ UNCHANGED wFailReason
               [] ciTopic[c] = "failure" ->
                    /\ wPc' = [wPc EXCEPT ![t] = "withdrawErr"]
                    /\ wFailReason' = [wFailReason EXCEPT ![t] = "topic-check-failed"]
                    /\ UNCHANGED inWorktree
               [] OTHER -> UNCHANGED <<wPc, wFailReason, inWorktree>>
        ELSE /\ wPc' = [wPc EXCEPT ![t] = "pushTargetExec"]
             /\ inWorktree' = inWorktree \cup {t}
             /\ UNCHANGED wFailReason
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               queue, wAlive, wStage, wBase, wMerged, wtSrc, wtMergedValid,
               validatedOwn, validT, conflictC, preflightBad, ciTopic, ciTarget,
               tRemoteTip, tRemoteCommits, integratedInfo, retiredSnap,
               pushedWhileHeld, badQueueRemove, extPushed>>

(* --- push_target() lines 422-445: hold check, fetch, push target, leave_queue BEFORE write_durable --- *)

WorkerPushTarget(t) ==
  /\ wAlive[t] /\ wPc[t] = "pushTargetExec"
  /\ LET c == accept[t].commit
         merged == wMerged[t]
         base == wBase[t]
         removed == { e \in queue : QueueMatches(t, e.task) }
     IN IF hold[t]
        THEN /\ wFailReason' = [wFailReason EXCEPT ![t] = "held"]
             /\ wPc' = [wPc EXCEPT ![t] = "withdrawErr"]
             /\ inWorktree' = inWorktree \ {t}
             /\ UNCHANGED <<queue, wStage, tRemoteTip, tRemoteCommits, ciTarget,
                          pushedWhileHeld, badQueueRemove>>
        ELSE IF tRemoteTip = merged
        THEN \* Already on remote -> leave_queue() then return TargetPushed (line 430-433)
             /\ queue' = queue \ removed
             /\ badQueueRemove' = (badQueueRemove \/ (\E e \in removed : e.task # t /\ Running(e.task)))
             /\ wStage' = [wStage EXCEPT ![t] = "target-pushed"]
             /\ wPc' = [wPc EXCEPT ![t] = "writeStage"]
             /\ inWorktree' = inWorktree \ {t}
             /\ UNCHANGED <<wFailReason, tRemoteTip, tRemoteCommits, ciTarget, pushedWhileHeld>>
        ELSE IF tRemoteTip # base
        THEN \* Remote moved -> return TopicPushed without leave_queue (line 434-436)
             /\ wStage' = [wStage EXCEPT ![t] = "topic-pushed"]
             /\ wPc' = [wPc EXCEPT ![t] = "writeStage"]
             /\ inWorktree' = inWorktree \ {t}
             /\ UNCHANGED <<queue, wFailReason, tRemoteTip, tRemoteCommits, ciTarget,
                          pushedWhileHeld, badQueueRemove>>
        ELSE IF preflightBad
        THEN /\ wFailReason' = [wFailReason EXCEPT ![t] = "preflight"]
             /\ wPc' = [wPc EXCEPT ![t] = "withdrawErr"]
             /\ inWorktree' = inWorktree \ {t}
             /\ UNCHANGED <<queue, wStage, tRemoteTip, tRemoteCommits, ciTarget,
                          pushedWhileHeld, badQueueRemove>>
        ELSE \* Push target, then leave_queue() BEFORE write_durable (lines 441-445)
             /\ tRemoteTip' = merged
             /\ tRemoteCommits' = tRemoteCommits \cup {c}
             /\ ciTarget' = [ciTarget EXCEPT ![c] =
                               IF Gated /\ ciTarget[c] = None THEN "pending" ELSE ciTarget[c]]
             /\ pushedWhileHeld' = (pushedWhileHeld \/ hold[t])
             /\ queue' = queue \ removed
             /\ badQueueRemove' = (badQueueRemove \/ (\E e \in removed : e.task # t /\ Running(e.task)))
             /\ wStage' = [wStage EXCEPT ![t] = "target-pushed"]
             /\ wPc' = [wPc EXCEPT ![t] = "writeStage"]
             /\ inWorktree' = inWorktree \ {t}
             /\ UNCHANGED wFailReason
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               wAlive, wBase, wMerged, wtSrc, wtMergedValid, validatedOwn,
               validT, conflictC, preflightBad, ciTopic, integratedInfo,
               retiredSnap, extPushed>>

(* --- Stage::TargetPushed lines 230-236: wait_for_checks(merged) --- *)

WorkerTargetChecks(t) ==
  /\ wAlive[t] /\ wPc[t] = "waitTargetCI"
  /\ LET mSrc == wMerged[t].src
     IN IF ~Gated \/ (mSrc \in TargetCommits /\ ciTarget[mSrc] = "success")
        THEN /\ wStage' = [wStage EXCEPT ![t] = "integrated"]
             /\ wPc' = [wPc EXCEPT ![t] = "writeStage"]
             /\ UNCHANGED wFailReason
        ELSE IF mSrc \in TargetCommits /\ ciTarget[mSrc] = "failure"
        THEN /\ wFailReason' = [wFailReason EXCEPT ![t] = "target-check-failed"]
             /\ wPc' = [wPc EXCEPT ![t] = "withdrawErr"]
             /\ UNCHANGED wStage
        ELSE UNCHANGED <<wStage, wPc, wFailReason>>
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               queue, wAlive, wBase, wMerged, inWorktree, wtSrc, wtMergedValid,
               validatedOwn, validT, conflictC, preflightBad, ciTopic, ciTarget,
               tRemoteTip, tRemoteCommits, integratedInfo, retiredSnap,
               pushedWhileHeld, badQueueRemove, extPushed>>

(* --- Stage::Integrated (--retire) lines 237-240 & commands.rs retire_task lines 224-275 --- *)

WorkerRetire(t) ==
  /\ wAlive[t] /\ wPc[t] = "doRetire"
  /\ LET cur == CommitId(t, head[t])
     IN IF ~hold[t] /\ ~dirty[t] /\ cur = accept[t].commit /\ Published(cur)
        THEN /\ retired' = [retired EXCEPT ![t] = TRUE]
             /\ retiredSnap' = [retiredSnap EXCEPT ![t] =
                                 [checked |-> TRUE,
                                  clean |-> ~dirty[t],
                                  notHeld |-> ~hold[t],
                                  published |-> Published(cur)]]
             /\ accept' = [accept EXCEPT ![t] = [accept[t] EXCEPT !.exists = FALSE]]
             /\ prog' = [prog EXCEPT ![t] = NoProg]
             /\ wAlive' = [wAlive EXCEPT ![t] = FALSE]
             /\ wPc' = [wPc EXCEPT ![t] = "idle"]
             /\ UNCHANGED wFailReason
        ELSE /\ wFailReason' = [wFailReason EXCEPT ![t] = "retire-refused"]
             /\ wPc' = [wPc EXCEPT ![t] = "withdrawErr"]
             /\ UNCHANGED <<retired, retiredSnap, accept, prog, wAlive>>
  /\ UNCHANGED <<nextTs, head, dirty, hold, finPhase, queue, wStage, wBase,
               wMerged, inWorktree, wtSrc, wtMergedValid, validatedOwn, validT,
               conflictC, preflightBad, ciTopic, ciTarget, tRemoteTip,
               tRemoteCommits, integratedInfo, pushedWhileHeld, badQueueRemove,
               extPushed>>

(* --- Error path withdraw() (finish.rs:182, 498-505) runs BEFORE write_durable (line 187) --- *)

WorkerWithdraw(t) ==
  /\ wAlive[t] /\ wPc[t] = "withdrawErr"
  /\ LET removed == { e \in queue : QueueMatches(t, e.task) }
     IN /\ queue' = queue \ removed
        /\ badQueueRemove' = (badQueueRemove \/ (\E e \in removed : e.task # t /\ Running(e.task)))
        /\ wStage' = [wStage EXCEPT ![t] = "needs-action"]
        /\ wPc' = [wPc EXCEPT ![t] = "writeStage"]
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               wAlive, wBase, wMerged, wFailReason, inWorktree, wtSrc,
               wtMergedValid, validatedOwn, validT, conflictC, preflightBad,
               ciTopic, ciTarget, tRemoteTip, tRemoteCommits, integratedInfo,
               retiredSnap, pushedWhileHeld, extPushed>>

(* --- Store::write_durable(&progress_path, &progress) (finish.rs:187) --- *)

WorkerWriteStage(t) ==
  /\ wAlive[t] /\ wPc[t] = "writeStage"
  /\ prog' = [prog EXCEPT ![t] = [attempt |-> accept[t].attempt,
                                  stage |-> wStage[t],
                                  baseObj |-> wBase[t],
                                  mergedObj |-> wMerged[t]]]
  /\ integratedInfo' =
       IF wStage[t] = "integrated"
       THEN [integratedInfo EXCEPT ![t] = [done |-> TRUE,
                                           commit |-> accept[t].commit,
                                           merged |-> wMerged[t]]]
       ELSE integratedInfo
  /\ IF IsTerminal(wStage[t], accept[t].retire)
     THEN /\ wAlive' = [wAlive EXCEPT ![t] = FALSE]
          /\ wPc' = [wPc EXCEPT ![t] = "idle"]
     ELSE /\ wPc' = [wPc EXCEPT ![t] =
                       CASE wStage[t] = "topic-pushed" -> "waitTurn"
                         [] wStage[t] = "prepared" -> "checkTopicCI"
                         [] wStage[t] = "target-pushed" -> "waitTargetCI"
                         [] wStage[t] = "integrated" -> "doRetire"
                         [] OTHER -> "idle"]
          /\ UNCHANGED wAlive
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, queue,
               wStage, wBase, wMerged, wFailReason, inWorktree, wtSrc,
               wtMergedValid, validatedOwn, validT, conflictC, preflightBad,
               ciTopic, ciTarget, tRemoteTip, tRemoteCommits, retiredSnap,
               pushedWhileHeld, badQueueRemove, extPushed>>

(* --- Process crash/kill: workers/<t>.lock drops, in-memory state lost, disk state stays --- *)

WorkerKill(t) ==
  /\ Kills
  /\ wAlive[t]
  /\ wAlive' = [wAlive EXCEPT ![t] = FALSE]
  /\ wPc' = [wPc EXCEPT ![t] = "idle"]
  /\ inWorktree' = inWorktree \ {t}
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               queue, wStage, wBase, wMerged, wFailReason, wtSrc,
               wtMergedValid, validatedOwn, validT, conflictC, preflightBad,
               ciTopic, ciTarget, tRemoteTip, tRemoteCommits, integratedInfo,
               retiredSnap, pushedWhileHeld, badQueueRemove, extPushed>>

(* --- Environment events: CI completion, user commits/dirty/hold, external target push --- *)

CiTopicCompletes ==
  \E c \in TaskCommits, r \in {"success", "failure"} :
    /\ ciTopic[c] = "pending"
    /\ ciTopic' = [ciTopic EXCEPT ![c] = r]
    /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
                 queue, wAlive, wPc, wStage, wBase, wMerged, wFailReason,
                 inWorktree, wtSrc, wtMergedValid, validatedOwn, validT,
                 conflictC, preflightBad, ciTarget, tRemoteTip, tRemoteCommits,
                 integratedInfo, retiredSnap, pushedWhileHeld, badQueueRemove,
                 extPushed>>

CiTargetCompletes ==
  \E m \in TargetCommits, r \in {"success", "failure"} :
    /\ ciTarget[m] = "pending"
    /\ ciTarget' = [ciTarget EXCEPT ![m] = r]
    /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
                 queue, wAlive, wPc, wStage, wBase, wMerged, wFailReason,
                 inWorktree, wtSrc, wtMergedValid, validatedOwn, validT,
                 conflictC, preflightBad, ciTopic, tRemoteTip, tRemoteCommits,
                 integratedInfo, retiredSnap, pushedWhileHeld, badQueueRemove,
                 extPushed>>

UserCommitFix(t) ==
  /\ MaxEdits > 0
  /\ ~retired[t]
  /\ head[t] <= MaxEdits
  /\ head' = [head EXCEPT ![t] = head[t] + 1]
  /\ dirty' = [dirty EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<nextTs, hold, retired, finPhase, accept, prog, queue, wAlive,
               wPc, wStage, wBase, wMerged, wFailReason, inWorktree, wtSrc,
               wtMergedValid, validatedOwn, validT, conflictC, preflightBad,
               ciTopic, ciTarget, tRemoteTip, tRemoteCommits, integratedInfo,
               retiredSnap, pushedWhileHeld, badQueueRemove, extPushed>>

UserToggleDirty(t) ==
  /\ RetireMode
  /\ ~retired[t]
  /\ ~dirty[t]
  /\ dirty' = [dirty EXCEPT ![t] = TRUE]
  /\ UNCHANGED <<nextTs, head, hold, retired, finPhase, accept, prog, queue,
               wAlive, wPc, wStage, wBase, wMerged, wFailReason, inWorktree,
               wtSrc, wtMergedValid, validatedOwn, validT, conflictC,
               preflightBad, ciTopic, ciTarget, tRemoteTip, tRemoteCommits,
               integratedInfo, retiredSnap, pushedWhileHeld, badQueueRemove,
               extPushed>>

UserHold(t) ==
  /\ RetireMode
  /\ ~retired[t]
  /\ ~hold[t]
  /\ wPc[t] # "pushTargetExec"
  /\ hold' = [hold EXCEPT ![t] = TRUE]
  /\ UNCHANGED <<nextTs, head, dirty, retired, finPhase, accept, prog, queue,
               wAlive, wPc, wStage, wBase, wMerged, wFailReason, inWorktree,
               wtSrc, wtMergedValid, validatedOwn, validT, conflictC,
               preflightBad, ciTopic, ciTarget, tRemoteTip, tRemoteCommits,
               integratedInfo, retiredSnap, pushedWhileHeld, badQueueRemove,
               extPushed>>

UserReleaseHold(t) ==
  /\ RetireMode
  /\ ~retired[t]
  /\ hold[t]
  /\ hold' = [hold EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<nextTs, head, dirty, retired, finPhase, accept, prog, queue,
               wAlive, wPc, wStage, wBase, wMerged, wFailReason, inWorktree,
               wtSrc, wtMergedValid, validatedOwn, validT, conflictC,
               preflightBad, ciTopic, ciTarget, tRemoteTip, tRemoteCommits,
               integratedInfo, retiredSnap, pushedWhileHeld, badQueueRemove,
               extPushed>>

ExternalTargetPush ==
  /\ AllowExternal /\ ~extPushed
  /\ extPushed' = TRUE
  /\ tRemoteTip' = ExtObj
  /\ tRemoteCommits' = tRemoteCommits \cup {"ext"}
  /\ ciTarget' = [ciTarget EXCEPT !["ext"] = IF Gated THEN "pending" ELSE "success"]
  /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
               queue, wAlive, wPc, wStage, wBase, wMerged, wFailReason,
               inWorktree, wtSrc, wtMergedValid, validatedOwn, validT,
               conflictC, preflightBad, ciTopic, integratedInfo, retiredSnap,
               pushedWhileHeld, badQueueRemove>>

Done ==
  /\ AllDone
  /\ UNCHANGED vars

Next ==
  \/ \E t \in Tasks :
       \/ FinishWriteAccept(t)
       \/ FinishEnqueue(t)
       \/ WorkerStart(t)
       \/ WorkerPushTopic(t)
       \/ WorkerWaitTurn(t)
       \/ WorkerPrepareMerge(t)
       \/ WorkerPrepareValidate(t)
       \/ WorkerCheckTopicCI(t)
       \/ WorkerPushTarget(t)
       \/ WorkerTargetChecks(t)
       \/ WorkerRetire(t)
       \/ WorkerWithdraw(t)
       \/ WorkerWriteStage(t)
       \/ WorkerKill(t)
       \/ UserCommitFix(t)
       \/ UserToggleDirty(t)
       \/ UserHold(t)
       \/ UserReleaseHold(t)
  \/ CiTopicCompletes
  \/ CiTargetCompletes
  \/ ExternalTargetPush
  \/ Done

Spec == Init /\ [][Next]_vars

(* --- Invariants (Rules) --- *)

\* At most one worker uses integrate-<target> at any moment.
ExclusiveIntegrationWorktree ==
  Cardinality(inWorktree) <= 1

\* Every task commit pushed to origin/<target> passed validation on its own
\* uncorrupted merged tree and passed topic required checks when Gated.
OnlyCheckedWorkReachesTarget ==
  \A c \in TaskCommits :
    c \in tRemoteCommits =>
      /\ validT[c]
      /\ validatedOwn[c]
      /\ (Gated => ciTopic[c] = "success")

\* A task recorded as integrated (or retired after integration) has its accepted commit on origin/<target>.
IntegratedIsOnRemote ==
  \A t \in Tasks :
    integratedInfo[t].done => integratedInfo[t].commit \in tRemoteCommits

\* When Gated, a task recorded as integrated passed target required checks on its pushed commit.
IntegratedChecksPassed ==
  \A t \in Tasks :
    (Gated /\ integratedInfo[t].done) => ciTarget[integratedInfo[t].merged.src] = "success"

\* A task retired via finish --retire had no uncommitted/untracked files, was not held,
\* and had its accepted commit published on origin/<target>.
NoUnpublishedOrDirtyRetire ==
  \A t \in Tasks :
    retired[t] => (retiredSnap[t].clean /\ retiredSnap[t].notHeld /\ retiredSnap[t].published)

\* No target push happens while the task has an active HoldRecord.
NoTargetPushWhileHeld ==
  ~pushedWhileHeld

\* leave_queue() never deletes another running task's queue entry.
QueueEntryMatchesOwner ==
  ~badQueueRemove
=============================================================================
