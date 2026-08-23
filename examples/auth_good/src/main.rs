#[derive(Clone)] struct User;
enum AuthError { WrongPassword }
fn authenticate(password: &str) -> Result<User, AuthError> { if password == "correct" { Ok(User) } else { Err(AuthError::WrongPassword) } }
fn create_session(_: User) {}
fn report_auth_failure() {}
fn login(password: &str) { match authenticate(password) { Ok(user) => create_session(user), Err(AuthError::WrongPassword) => report_auth_failure() } }
fn main() { login("bad"); }
