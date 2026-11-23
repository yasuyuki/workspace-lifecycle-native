---- MODULE RequiredChecks ----
CONSTANTS ReadSuites, TrustOtherApp, StopOnRefusal

VARIABLES stage, topic, target, retries, push
vars == <<stage, topic, target, retries, push>>

Init ==
    /\ stage = "prepared"
    /\ topic = "unread"
    /\ target = "unread"
    /\ retries = 0
    /\ push = "none"

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
    /\ UNCHANGED <<target, retries, push>>

\* "accepted": the remote takes the push.
\* "refused": the remote refuses it, such as branch protection (GH006), and the target has not moved.
\* "target moved": another push reached the target first, so the push is not a fast-forward.
\* A refusal or a moved target that goes back to "prepared" merges and validates again.
PushTarget ==
    /\ stage = "topic checked"
    /\ \E outcome \in {"accepted", "refused", "target moved"}:
        /\ push' = outcome
        /\ stage' = CASE outcome = "accepted" -> "target pushed"
                      [] outcome = "refused" /\ StopOnRefusal -> "push refused"
                      [] OTHER -> "prepared"
        /\ topic' = IF stage' = "prepared" THEN "unread" ELSE topic
    /\ UNCHANGED <<target, retries>>

PollTarget ==
    /\ stage = "target pushed"
    /\ \E reply \in Replies:
        /\ target' = reply
        /\ stage' = CASE SelectSuccess(reply) -> "integrated"
                      [] Stops(reply) -> "target failed"
                      [] OTHER -> "target pushed"
    /\ UNCHANGED <<topic, retries, push>>

Retry ==
    /\ stage \in {"topic failed", "push refused"}
    /\ retries = 0
    /\ stage' = "prepared"
    /\ topic' = "unread"
    /\ retries' = 1
    /\ push' = "none"
    /\ UNCHANGED target

Done ==
    /\ stage \in {"integrated", "target failed"}
        \/ (stage \in {"topic failed", "push refused"} /\ retries = 1)
    /\ UNCHANGED vars

Next == PollTopic \/ PushTarget \/ PollTarget \/ Retry \/ Done

OnlyRequiredChecksAdvance ==
    /\ stage \in {"topic checked", "target pushed", "target failed", "integrated"} => IsSuccess(topic)
    /\ stage = "integrated" => IsSuccess(target)

WorkflowFailureStopsNextPoll ==
    /\ topic = "unstarted" => stage # "prepared"
    /\ target = "unstarted" => stage # "target pushed"

RefusedPushStops ==
    push = "refused" => stage = "push refused"
====
