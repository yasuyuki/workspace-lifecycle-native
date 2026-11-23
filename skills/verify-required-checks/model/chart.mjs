export const title = "Required checks during wl finish";

export function state({ stage, topic, target }) {
  if ((stage === "prepared" && topic === "unstarted") || (stage === "target pushed" && target === "unstarted")) {
    return "Still waiting after workflow could not start";
  }
  return {
    prepared: "Waiting for topic checks",
    "topic checked": "Topic checks passed",
    "topic failed": "Needs action before target push",
    "push refused": "Needs action with the remote's reason",
    "target pushed": "Waiting for checks on pushed target",
    "target failed": "Needs action after target push",
    integrated: "Integrated",
  }[stage];
}

export function step(action, from, to) {
  const unstarted = (action === "PollTopic" ? to.topic : to.target) === "unstarted";
  return {
    PollTopic: unstarted ? "workflow cannot start"
      : to.stage === "topic checked" && to.topic !== "passed" ? "a different app reports success"
      : to.stage === "topic checked" ? "required topic check passes" : "required topic check fails",
    PushTarget: to.push === "accepted" ? "push prepared commit to target"
      : to.push === "refused" ? "the remote refuses the push [target unmoved]"
      : "the target moved first, so merge and validate again",
    PollTarget: unstarted ? "workflow cannot start"
      : to.stage === "integrated" ? "required target check passes" : "required target check fails",
    Retry: "finish again after fixing the cause",
    Done: "finish completes",
  }[action];
}
