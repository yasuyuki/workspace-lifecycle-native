---- MODULE RequiredChecks ----
CONSTANTS ObserveFailedSuite, TrustOtherApp

VARIABLES stage, topic, target, retries, ignoredWorkflowFailure
vars == <<stage, topic, target, retries, ignoredWorkflowFailure>>

Init ==
    /\ stage = "prepared"
    /\ topic = "unread"
    /\ target = "unread"
    /\ retries = 0
    /\ ignoredWorkflowFailure = FALSE

Replies == {"missing", "other app passed", "latest pending", "failed", "unfinalized", "passed"}
IsSuccess(reply) == reply = "passed"
Stops(reply) == reply \in {"failed", "unfinalized"}
SelectSuccess(reply) == IsSuccess(reply) \/ (TrustOtherApp /\ reply = "other app passed")

PollTopic ==
    /\ stage = "prepared"
    /\ ~ignoredWorkflowFailure
    /\ \E reply \in Replies:
        /\ topic' = reply
        /\ stage' = CASE SelectSuccess(reply) -> "topic checked"
                      [] Stops(reply) -> "topic failed"
                      [] OTHER -> "prepared"
    /\ UNCHANGED <<target, retries, ignoredWorkflowFailure>>

PollEmptyFailedSuite ==
    /\ ObserveFailedSuite
    /\ stage = "prepared"
    /\ topic' = "missing"
    /\ ignoredWorkflowFailure' = TRUE
    /\ UNCHANGED <<stage, target, retries>>

PushTarget ==
    /\ stage = "topic checked"
    /\ stage' = "target pushed"
    /\ UNCHANGED <<topic, target, retries, ignoredWorkflowFailure>>

PollTarget ==
    /\ stage = "target pushed"
    /\ \E reply \in Replies:
        /\ target' = reply
        /\ stage' = CASE SelectSuccess(reply) -> "integrated"
                      [] Stops(reply) -> "target failed"
                      [] OTHER -> "target pushed"
    /\ UNCHANGED <<topic, retries, ignoredWorkflowFailure>>

Retry ==
    /\ stage = "topic failed"
    /\ retries = 0
    /\ stage' = "prepared"
    /\ topic' = "unread"
    /\ retries' = 1
    /\ UNCHANGED <<target, ignoredWorkflowFailure>>

Done ==
    /\ stage \in {"integrated", "target failed"}
        \/ (stage = "topic failed" /\ retries = 1)
        \/ ignoredWorkflowFailure
    /\ UNCHANGED vars

Next == PollTopic \/ PollEmptyFailedSuite \/ PushTarget \/ PollTarget \/ Retry \/ Done

OnlyRequiredChecksAdvance ==
    /\ stage \in {"topic checked", "target pushed", "target failed", "integrated"} => IsSuccess(topic)
    /\ stage = "integrated" => IsSuccess(target)

WorkflowFailureStopsNextPoll == ~ignoredWorkflowFailure
====
