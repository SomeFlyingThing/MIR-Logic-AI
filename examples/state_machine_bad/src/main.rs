enum StateError { Invalid }
fn validate_state(valid: bool) -> Result<(), StateError> { if valid { Ok(()) } else { Err(StateError::Invalid) } }
fn log_invalid_state() {}
fn transition_to_active() {}
fn advance(valid: bool) { match validate_state(valid) { Ok(()) => transition_to_active(), Err(StateError::Invalid) => { log_invalid_state(); transition_to_active(); } } }
fn main() { advance(false); }
