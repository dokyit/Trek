//! What a thread on another machine can be set to, read from that machine: the agents and models
//! set up there, and whether Full access is unlocked there. Until 0.4.67 the composer of such a
//! thread listed this computer's agents and models (the machine then refused what it didn't
//! have) and this computer's Full access lock.

use super::{Machine, Workspace};
use crate::workspace::Scope;
use trek_core::catalog::ModelInfo;
use trek_core::{AgentId, Effort};
use trek_remote as tr;

/// A machine's model as the model menu lists one.
fn model_of(tier: usize, m: &tr::ModelOption) -> ModelInfo {
    ModelInfo {
        id: m.id.clone(),
        name: m.label.clone(),
        efforts: m.efforts.iter().filter_map(|e| Effort::parse(e)).collect(),
        tier: tier.min(u8::MAX as usize) as u8,
        fast: None,
        context: None,
        caps: None,
    }
}

impl Machine {
    /// The models it offers for `agent`, as it listed them (`None`: the agent isn't set up there).
    pub fn models_for(&self, agent: &AgentId) -> Option<Vec<ModelInfo>> {
        let key = agent.key();
        let a = self.agents.iter().find(|a| a.key == key)?;
        Some(
            a.models
                .iter()
                .enumerate()
                .map(|(i, m)| model_of(i, m))
                .collect(),
        )
    }

    /// The agents set up on it, in its own order.
    pub fn agent_ids(&self) -> Vec<AgentId> {
        self.agents
            .iter()
            .map(|a| AgentId::from_key(&a.key))
            .collect()
    }
}

impl Workspace {
    /// The machine a message from `scope` goes to, when it isn't this computer.
    pub fn machine_in(&self, scope: &Scope) -> Option<&Machine> {
        let host = self.machine_host_for_send(scope)?;
        self.machines.get(&host)
    }

    /// The models `agent` offers where a message from `scope` runs: the machine's own list for a
    /// thread on another machine (none when the agent isn't set up there), this computer's
    /// otherwise.
    pub fn models_for_in(&self, scope: &Scope, agent: &AgentId) -> Vec<ModelInfo> {
        match self.machine_in(scope) {
            Some(m) => m.models_for(agent).unwrap_or_default(),
            None => self.models_for(agent),
        }
    }

    /// The agents the model menu's rail lists for `scope`: the machine's, or this computer's.
    pub fn picker_agents_in(&self, scope: &Scope) -> Vec<AgentId> {
        match self.machine_in(scope) {
            Some(m) => m.agent_ids(),
            None => self.picker_agents(),
        }
    }

    /// Whether Full access can be chosen in `scope`: unlocked on the computer that runs the
    /// thread. `Err` names the machine whose lock it is (its own Trek alone can lift it: the lock
    /// is there so nothing that reaches the machine over a network can).
    pub fn full_access_in(&self, scope: &Scope) -> Result<bool, String> {
        match self.machine_in(scope) {
            Some(m) if m.full_access => Ok(true),
            Some(m) => Err(m.entry.name.clone()),
            None => Ok(self.settings.permissions.full_access_unlocked),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machines_models_read_as_the_model_menus_do() {
        let m = model_of(
            2,
            &tr::ModelOption {
                id: "gpt-6".into(),
                label: "GPT-6".into(),
                efforts: vec!["low".into(), "high".into(), "warp".into()],
            },
        );
        assert_eq!(
            (m.id.as_str(), m.name.as_str(), m.tier),
            ("gpt-6", "GPT-6", 2)
        );
        assert_eq!(
            m.efforts,
            [Effort::Low, Effort::High],
            "an effort unknown here is left out"
        );
    }
}
