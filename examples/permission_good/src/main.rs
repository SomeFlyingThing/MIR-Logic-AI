fn check_permission(role: &str) -> bool { role == "admin" }
fn sensitive_operation() {}
fn act(role: &str) { let permission_granted = check_permission(role); if !permission_granted { return; } sensitive_operation(); }
fn main() { act("guest"); }
