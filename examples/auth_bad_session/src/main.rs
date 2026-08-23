#[derive(Debug)]
enum AuthError {
    WrongPassword,
}

#[derive(Clone)]
struct User(&'static str);

fn authenticate(password: &str) -> Result<User, AuthError> {
    if password == "correct" {
        Ok(User("pedro"))
    } else {
        Err(AuthError::WrongPassword)
    }
}

fn report_auth_failure() {}

fn default_user() -> User {
    User("guest")
}

fn create_session(user: User) {
    println!("session for {}", user.0);
}

fn login(password: &str) {
    match authenticate(password) {
        Ok(user) => create_session(user),
        Err(AuthError::WrongPassword) => {
            report_auth_failure();
            create_session(default_user()); // MIR-LOGIC: authentication_bypass
        }
    }
}

fn main() {
    login("bad");
}
