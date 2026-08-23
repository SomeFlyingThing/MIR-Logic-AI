struct Resource;
enum ResourceError { Closed }
fn open_resource(open: bool) -> Result<Resource, ResourceError> { if open { Ok(Resource) } else { Err(ResourceError::Closed) } }
fn fallback_resource() -> Resource { Resource }
fn log_resource_error() {}
fn use_resource(_: Resource) {}
fn run(open: bool) { match open_resource(open) { Ok(resource) => use_resource(resource), Err(ResourceError::Closed) => { log_resource_error(); use_resource(fallback_resource()); } } }
fn main() { run(false); }
