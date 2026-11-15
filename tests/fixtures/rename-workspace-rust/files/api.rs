use crate::util::foo;
use crate::util::{foo as bar};
pub fn call() -> i32 { foo(1) + bar(2) + crate::util::foo(3) }
