fn transaction() -> Result<(), ()> { Err(()) }
fn commit_transaction() {}
fn run() { match transaction() { Ok(()) => commit_transaction(), Err(()) => return } }
fn main() { run(); }
