use base::UUID;

/// Identifies this agent's session and circuit to the simulator.
#[derive(Clone, Debug, PartialEq)]
pub struct UseCircuitCode {
    pub circuit_code: CircuitCode,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CircuitCode {
    pub code: u32,
    pub session_id: UUID,
    pub agent_id: UUID,
}
