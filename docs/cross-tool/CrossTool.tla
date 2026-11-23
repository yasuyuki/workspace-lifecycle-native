---- MODULE CrossTool ----
\* One task in a repository that agent-rules moves from the legacy
\* workspace-lifecycle to wl, seen across the tools that touch it.
VARIABLES switched, owner, phase, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved

vars == <<switched, owner, phase, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

Active == {"working", "accepted", "topicPushed", "prepared", "targetPushed", "integrated", "needsAction"}

Init ==
  /\ switched = FALSE
  /\ owner = "none"
  /\ phase = "none"
  /\ target = "none"
  /\ inTarget = FALSE
  /\ inDefault = FALSE
  /\ session = "none"
  /\ issue = "open"
  /\ legacy = "installed"
  /\ pushMoved = FALSE
  /\ bindingRemoved = FALSE

\* agent-rules: the repository's entry becomes wl
RulesSwitch ==
  /\ ~switched
  /\ switched' = TRUE
  /\ UNCHANGED <<owner, phase, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

LegacyBegin ==
  /\ owner = "none" /\ ~switched
  /\ owner' = "legacy" /\ phase' = "working" /\ target' = "default"
  /\ UNCHANGED <<switched, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

LegacyFinish ==
  /\ owner = "legacy" /\ phase = "working"
  /\ phase' = "retired" /\ inTarget' = TRUE /\ inDefault' = TRUE
  /\ UNCHANGED <<switched, owner, target, session, issue, legacy, pushMoved, bindingRemoved>>

WlBegin ==
  /\ owner = "none" /\ switched
  /\ owner' = "wl" /\ phase' = "working"
  /\ target' \in {"default", "feature"}
  /\ UNCHANGED <<switched, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

\* Another task, such as a wl task merging the legacy topic, brings the
\* legacy task's work to the default branch.
LegacyWorkReachesDefault ==
  /\ owner = "legacy" /\ phase = "working" /\ ~inDefault
  /\ inTarget' = TRUE /\ inDefault' = TRUE
  /\ UNCHANGED <<switched, owner, phase, target, session, issue, legacy, pushMoved, bindingRemoved>>

\* No legacy entry ends the task (a primary checkout, or an owner receipt
\* whose completion can no longer run); it stays recorded with no work of its own.
LegacyStranded ==
  /\ owner = "legacy" /\ phase = "working" /\ inDefault
  /\ phase' = "stranded"
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

\* The one modeled task represents the legacy drain check.
LegacyTaskActive == owner = "legacy" /\ phase \in Active

MovePushDecision ==
  /\ switched /\ ~pushMoved
  /\ pushMoved' = TRUE
  /\ UNCHANGED <<switched, owner, phase, target, inTarget, inDefault, session, issue, legacy, bindingRemoved>>

RemoveWorkspaceBinding ==
  /\ switched /\ ~bindingRemoved
  /\ bindingRemoved' = TRUE
  /\ UNCHANGED <<switched, owner, phase, target, inTarget, inDefault, session, issue, legacy, pushMoved>>

LegacyUninstall ==
  /\ legacy = "installed" /\ switched
  /\ ~LegacyTaskActive /\ pushMoved /\ bindingRemoved
  /\ legacy' = "removed"
  /\ UNCHANGED <<switched, owner, phase, target, inTarget, inDefault, session, issue, pushMoved, bindingRemoved>>

Launch ==
  /\ session = "none" /\ phase = "working"
  /\ session' = IF bindingRemoved \/ legacy = "installed" THEN "running" ELSE "blocked"
  /\ UNCHANGED <<switched, owner, phase, target, inTarget, inDefault, issue, legacy, pushMoved, bindingRemoved>>

SessionEnds ==
  /\ session = "running"
  /\ session' = "ended"
  /\ UNCHANGED <<switched, owner, phase, target, inTarget, inDefault, issue, legacy, pushMoved, bindingRemoved>>

WlFinish ==
  /\ owner = "wl" /\ phase \in {"working", "needsAction"}
  /\ phase' = "accepted"
  \* A session that calls finish --retire stops task work when it submits finish.
  /\ session' \in IF session = "running" THEN {"running", "ended"} ELSE {"ended"}
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, issue, legacy, pushMoved, bindingRemoved>>

PreflightPush ==
  /\ phase = "accepted" /\ (pushMoved \/ legacy = "installed")
  /\ phase' = "topicPushed"
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

PreflightStops ==
  /\ phase \in {"accepted", "prepared"}
  /\ phase' = "needsAction"
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

TopicAndValidationPass ==
  /\ phase = "topicPushed"
  /\ phase' = "prepared"
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

TopicOrValidationFails ==
  /\ phase = "topicPushed"
  /\ phase' = "needsAction"
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

TargetPush ==
  /\ phase = "prepared" /\ (pushMoved \/ legacy = "installed")
  /\ phase' = "targetPushed" /\ inTarget' = TRUE
  /\ inDefault' = (inDefault \/ target = "default")
  /\ UNCHANGED <<switched, owner, target, session, issue, legacy, pushMoved, bindingRemoved>>

TargetCheckPasses ==
  /\ phase = "targetPushed"
  /\ phase' = "integrated"
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

TargetCheckFails ==
  /\ phase = "targetPushed"
  /\ phase' = "needsAction"
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

\* An explicit retire is likewise used only after the session releases the slot.
WlRetire ==
  /\ owner = "wl" /\ session # "running"
  /\ (phase = "integrated" \/ (phase = "needsAction" /\ inTarget))
  /\ phase' = "retired"
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

NextTaskTakesSlot ==
  /\ owner = "wl" /\ phase = "retired"
  /\ phase' = "reused"
  /\ UNCHANGED <<switched, owner, target, inTarget, inDefault, session, issue, legacy, pushMoved, bindingRemoved>>

\* Some other task later integrates the feature branch into the default branch
FeatureReachesDefault ==
  /\ target = "feature" /\ inTarget /\ ~inDefault
  /\ inDefault' = TRUE
  /\ UNCHANGED <<switched, owner, phase, target, inTarget, session, issue, legacy, pushMoved, bindingRemoved>>

\* Target integration alone does not allow Issue closure.
CloseIssue ==
  /\ issue = "open" /\ inDefault
  /\ phase \in {"integrated", "retired", "reused"}
  /\ issue' = "closed"
  /\ UNCHANGED <<switched, owner, phase, target, inTarget, inDefault, session, legacy, pushMoved, bindingRemoved>>

Next ==
  \/ RulesSwitch \/ LegacyBegin \/ LegacyFinish \/ WlBegin
  \/ LegacyWorkReachesDefault \/ LegacyStranded
  \/ MovePushDecision \/ RemoveWorkspaceBinding \/ LegacyUninstall
  \/ Launch \/ SessionEnds
  \/ WlFinish \/ PreflightPush \/ PreflightStops
  \/ TopicAndValidationPass \/ TopicOrValidationFails
  \/ TargetPush \/ TargetCheckPasses \/ TargetCheckFails
  \/ WlRetire \/ NextTaskTakesSlot \/ FeatureReachesDefault \/ CloseIssue

Spec == Init /\ [][Next]_vars

\* Rules
IssueClosesOnlyInDefault == issue = "closed" => inDefault
NoSessionInReusedSlot == ~(phase = "reused" /\ session = "running")
LegacyOutlivesDependents ==
  (LegacyTaskActive \/ ~pushMoved \/ ~bindingRemoved) => legacy = "installed"
SessionCanStart == session # "blocked"
NoLegacyWorkLostAtRemoval == (legacy = "removed" /\ owner = "legacy") => inDefault
====
