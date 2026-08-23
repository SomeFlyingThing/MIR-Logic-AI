fn transaction() -> Result<(), ()> { Err(()) }
fn log_transaction_failure() {}
fn commit_transaction() {}
fn run() { match transaction() { Ok(()) => commit_transaction(), Err(()) => { log_transaction_failure(); commit_transaction(); } } }
fn main() { run(); }
