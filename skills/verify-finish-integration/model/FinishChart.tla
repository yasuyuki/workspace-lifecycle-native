---------------------------- MODULE FinishChart ----------------------------
(* Smaller single-task instance of FinishIntegration for drawing the
   finish-integration statechart under the 500-state chart budget:
   shows validation failure -> fix commit -> external push re-prepare ->
   gated checks -> integrated -> retired. *)
EXTENDS FinishIntegration

ChartInit ==
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
  /\ validT = [c \in TaskCommits |-> (c # "a1")]
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

ChartCiTopicSucceeds ==
  \E c \in TaskCommits :
    /\ ciTopic[c] = "pending"
    /\ wPc["a"] = "checkTopicCI"
    /\ ciTopic' = [ciTopic EXCEPT ![c] = "success"]
    /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
                 queue, wAlive, wPc, wStage, wBase, wMerged, wFailReason,
                 inWorktree, wtSrc, wtMergedValid, validatedOwn, validT,
                 conflictC, preflightBad, ciTarget, tRemoteTip, tRemoteCommits,
                 integratedInfo, retiredSnap, pushedWhileHeld, badQueueRemove,
                 extPushed>>

ChartCiTargetSucceeds ==
  \E m \in TargetCommits :
    /\ ciTarget[m] = "pending"
    /\ wPc["a"] = "waitTargetCI"
    /\ ciTarget' = [ciTarget EXCEPT ![m] = "success"]
    /\ UNCHANGED <<nextTs, head, dirty, hold, retired, finPhase, accept, prog,
                 queue, wAlive, wPc, wStage, wBase, wMerged, wFailReason,
                 inWorktree, wtSrc, wtMergedValid, validatedOwn, validT,
                 conflictC, preflightBad, ciTopic, tRemoteTip, tRemoteCommits,
                 integratedInfo, retiredSnap, pushedWhileHeld, badQueueRemove,
                 extPushed>>

ChartExternalPush ==
  /\ wPc["a"] = "checkTopicCI"
  /\ head["a"] = 2
  /\ ExternalTargetPush

ChartUserCommitFix ==
  /\ ~Running("a")
  /\ UserCommitFix("a")

ChartNext ==
  \/ FinishWriteAccept("a")
  \/ WorkerStart("a")
  \/ WorkerPushTopic("a")
  \/ WorkerWaitTurn("a")
  \/ WorkerPrepareMerge("a")
  \/ WorkerPrepareValidate("a")
  \/ WorkerCheckTopicCI("a")
  \/ WorkerPushTarget("a")
  \/ WorkerTargetChecks("a")
  \/ WorkerRetire("a")
  \/ WorkerWithdraw("a")
  \/ WorkerWriteStage("a")
  \/ ChartUserCommitFix
  \/ ChartCiTopicSucceeds
  \/ ChartCiTargetSucceeds
  \/ ChartExternalPush
  \/ Done

ChartSpec == ChartInit /\ [][ChartNext]_vars
=============================================================================
