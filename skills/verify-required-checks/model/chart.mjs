export const title = "Required checks during wl finish";

export function state({ stage, ignoredWorkflowFailure }) {
  if (ignoredWorkflowFailure && stage === "prepared") {
    return "Still waiting after workflow failure";
  }
  return {
    prepared: "Waiting for topic checks",
    "topic checked": "Topic checks passed",
    "topic failed": "Needs action before target push",
    "target pushed": "Waiting for checks on pushed target",
    "target failed": "Needs action after target push",
    integrated: "Integrated",
  }[stage];
}

export function step(action, from, to) {
  return {
    PollTopic: to.stage === "topic checked" && to.topic !== "passed"
      ? "a different app reports success"
      : to.stage === "topic checked" ? "required topic check passes" : "required topic check fails",
    PollEmptyFailedSuite: "workflow fails without creating check runs",
    PushTarget: "push prepared commit to target",
    PollTarget: to.stage === "integrated" ? "required target check passes" : "required target check fails",
    Retry: "finish a corrected topic commit",
    Done: "finish completes",
  }[action];
}
