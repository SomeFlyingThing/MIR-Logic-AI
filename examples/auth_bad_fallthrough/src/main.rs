fn authenticate(password: &str) -> bool { password == "correct" }
fn report_auth_failure() {}
fn create_session() {}
fn login(password: &str) { let authenticated = authenticate(password); if !authenticated { report_auth_failure(); } create_session(); }
fn main() { login("bad"); }
