use crate::util::qux;
use crate::util::{qux as bar};
pub fn call() -> i32 { qux(1) + bar(2) + crate::util::qux(3) }
