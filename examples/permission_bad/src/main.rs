fn check_permission(role: &str) -> bool { role == "admin" }
fn audit_permission_denied() {}
fn sensitive_operation() {}
fn act(role: &str) { let permission_granted = check_permission(role); if !permission_granted { audit_permission_denied(); } sensitive_operation(); }
fn main() { act("guest"); }
