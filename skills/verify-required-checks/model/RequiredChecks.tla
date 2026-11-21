---- MODULE RequiredChecks ----
CONSTANTS ReadSuites, TrustOtherApp

VARIABLES stage, topic, target, retries
vars == <<stage, topic, target, retries>>

Init ==
    /\ stage = "prepared"
    /\ topic = "unread"
    /\ target = "unread"
    /\ retries = 0

\* "missing": no matching run while the app has an unfinished suite or no failed suite without runs.
\* "unstarted": no matching run, every suite of the app completed, and one failed without check runs.
Replies == {"missing", "unstarted", "other app passed", "latest pending", "failed", "unfinalized", "passed"}
IsSuccess(reply) == reply = "passed"
Stops(reply) == reply \in {"failed", "unfinalized"} \/ (ReadSuites /\ reply = "unstarted")
SelectSuccess(reply) == IsSuccess(reply) \/ (TrustOtherApp /\ reply = "other app passed")

PollTopic ==
    /\ stage = "prepared"
    /\ \E reply \in Replies:
        /\ topic' = reply
        /\ stage' = CASE SelectSuccess(reply) -> "topic checked"
                      [] Stops(reply) -> "topic failed"
                      [] OTHER -> "prepared"
    /\ UNCHANGED <<target, retries>>

PushTarget ==
    /\ stage = "topic checked"
    /\ stage' = "target pushed"
    /\ UNCHANGED <<topic, target, retries>>

PollTarget ==
    /\ stage = "target pushed"
    /\ \E reply \in Replies:
        /\ target' = reply
        /\ stage' = CASE SelectSuccess(reply) -> "integrated"
                      [] Stops(reply) -> "target failed"
                      [] OTHER -> "target pushed"
    /\ UNCHANGED <<topic, retries>>

Retry ==
    /\ stage = "topic failed"
    /\ retries = 0
    /\ stage' = "prepared"
    /\ topic' = "unread"
    /\ retries' = 1
    /\ UNCHANGED target

Done ==
    /\ stage \in {"integrated", "target failed"}
        \/ (stage = "topic failed" /\ retries = 1)
    /\ UNCHANGED vars

Next == PollTopic \/ PushTarget \/ PollTarget \/ Retry \/ Done

OnlyRequiredChecksAdvance ==
    /\ stage \in {"topic checked", "target pushed", "target failed", "integrated"} => IsSuccess(topic)
    /\ stage = "integrated" => IsSuccess(target)

WorkflowFailureStopsNextPoll ==
    /\ topic = "unstarted" => stage # "prepared"
    /\ target = "unstarted" => stage # "target pushed"
====
