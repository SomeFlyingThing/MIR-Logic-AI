struct Resource;
enum ResourceError { Closed }
fn open_resource(open: bool) -> Result<Resource, ResourceError> { if open { Ok(Resource) } else { Err(ResourceError::Closed) } }
fn use_resource(_: Resource) {}
fn run(open: bool) { match open_resource(open) { Ok(resource) => use_resource(resource), Err(ResourceError::Closed) => return } }
fn main() { run(false); }
