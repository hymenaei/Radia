use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct UseCircuitCode {
    pub circuit_code: CircuitCode,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CircuitCode {
    pub code: u32,
    pub session_id: Uuid,
    pub agent_id: Uuid,
}
