---------------------------- MODULE RetireAbandon  ----------------------------
(* One task in one slot. What wl retire and wl retire --abandon may end. *)
VARIABLES phase, dirty, held, inTarget, onTopic, abandoned

vars == <<phase, dirty, held, inTarget, onTopic, abandoned>>

Init ==
  /\ phase = "working"
  /\ dirty = FALSE /\ held = FALSE
  /\ inTarget = TRUE /\ onTopic = TRUE
  /\ abandoned = FALSE

Working == phase = "working"

YouCommit ==
  /\ Working /\ dirty
  /\ dirty' = FALSE /\ inTarget' = FALSE /\ onTopic' = FALSE
  /\ UNCHANGED <<phase, held, abandoned>>

YouEdit ==
  /\ Working /\ ~dirty
  /\ dirty' = TRUE
  /\ UNCHANGED <<phase, held, inTarget, onTopic, abandoned>>

YouPushTopic ==
  /\ Working /\ ~onTopic
  /\ onTopic' = TRUE
  /\ UNCHANGED <<phase, dirty, held, inTarget, abandoned>>

FinishIntegrates ==
  /\ Working /\ ~inTarget /\ ~dirty /\ ~held
  /\ inTarget' = TRUE /\ onTopic' = TRUE
  /\ UNCHANGED <<phase, dirty, held, abandoned>>

YouHold ==
  /\ Working /\ ~held
  /\ held' = TRUE
  /\ UNCHANGED <<phase, dirty, inTarget, onTopic, abandoned>>

YouReleaseHold ==
  /\ Working /\ held
  /\ held' = FALSE
  /\ UNCHANGED <<phase, dirty, inTarget, onTopic, abandoned>>

Retire ==
  /\ Working /\ ~dirty /\ ~held /\ inTarget
  /\ phase' = "retired"
  /\ UNCHANGED <<dirty, held, inTarget, onTopic, abandoned>>

RetireAbandon ==
  /\ Working /\ ~dirty /\ ~held /\ onTopic
  /\ phase' = "retired" /\ abandoned' = TRUE
  /\ UNCHANGED <<dirty, held, inTarget, onTopic>>

Done == phase = "retired" /\ UNCHANGED vars

Next == YouEdit \/ YouCommit \/ YouPushTopic \/ FinishIntegrates \/ YouHold
     \/ YouReleaseHold \/ Retire \/ RetireAbandon \/ Done

Spec == Init /\ [][Next]_vars

\* Retirement never loses a commit: it is on the target or on the remote topic.
NoCommitLost == phase = "retired" => (inTarget \/ onTopic)
\* Retirement never ends uncommitted work or a held task.
NoDirtyOrHeldRetire == phase = "retired" => (~dirty /\ ~held)
\* Only an abandonment ends a task whose commits are not on the target.
OnlyAbandonSkipsTarget == (phase = "retired" /\ ~inTarget) => abandoned
=============================================================================
