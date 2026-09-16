//! Read-only agent awareness. Never infer readiness from terminal output.
use crate::api::{Agent, AgentState, AgentSummary};

pub fn from_command(command: &str) -> Option<Agent> {
    let name = command.rsplit('/').next().unwrap_or(command);
    // Exact executable names only: shells, node, python and substring matches
    // cannot reliably identify the program running inside them.
    if !matches!(
        name,
        "codex"
            | "claude"
            | "opencode"
            | "aider"
            | "gemini"
            | "amp"
            | "droid"
            | "hermes"
            | "kiro"
            | "copilot"
    ) {
        return None;
    }
    Some(Agent {
        name: name.into(),
        state: AgentState::Unknown,
        source: "command".into(),
    })
}

pub fn summarize<'a>(agents: impl Iterator<Item = &'a Agent>) -> Option<AgentSummary> {
    let priority = |state| match state {
        AgentState::Blocked => 5,
        AgentState::Working => 4,
        AgentState::Done => 3,
        AgentState::Unknown => 2,
        AgentState::Idle => 1,
    };
    let mut summary: Option<AgentSummary> = None;
    for agent in agents {
        let value = summary.get_or_insert(AgentSummary {
            state: agent.state,
            count: 0,
        });
        value.count += 1;
        if priority(agent.state) > priority(value.state) {
            value.state = agent.state;
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_identity_is_conservative_and_never_claims_readiness() {
        assert_eq!(
            from_command("/usr/bin/codex").unwrap().state,
            AgentState::Unknown
        );
        for command in ["sh", "node", "python", "my-codex", "codex-helper", "agent"] {
            assert!(from_command(command).is_none());
        }
    }
    #[test]
    fn summary_prioritizes_attention_and_clears_when_agents_exit() {
        let agents: Vec<_> = [
            AgentState::Idle,
            AgentState::Done,
            AgentState::Working,
            AgentState::Blocked,
        ]
        .into_iter()
        .map(|state| Agent {
            name: "fixture".into(),
            state,
            source: "herdr".into(),
        })
        .collect();
        for count in 1..=4 {
            let summary = summarize(agents[..count].iter()).unwrap();
            assert_eq!(summary.count, count);
            assert_eq!(summary.state, agents[count - 1].state);
        }
        assert!(summarize([].iter()).is_none());
        assert_eq!(
            serde_json::from_str::<AgentState>("\"future_state\"").unwrap(),
            AgentState::Unknown
        );
    }
}
