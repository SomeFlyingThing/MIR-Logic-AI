enum StateError { Invalid }
fn validate_state(valid: bool) -> Result<(), StateError> { if valid { Ok(()) } else { Err(StateError::Invalid) } }
fn transition_to_active() {}
fn advance(valid: bool) { match validate_state(valid) { Ok(()) => transition_to_active(), Err(StateError::Invalid) => return } }
fn main() { advance(false); }
